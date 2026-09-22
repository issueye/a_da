/**
 * 子智能体管理器 (SubagentManager)
 * 负责内置子智能体的注册、启停状态维护，以及项目级与全局自定义子智能体的扫描与持久化。
 */

import { existsSync, mkdirSync } from 'node:fs'
import { readdir, readFile, unlink, writeFile } from 'node:fs/promises'
import { basename, extname, join } from 'node:path'
import { getAppHome } from '../home'
import { BUILTIN_SUBAGENTS } from './builtins'
import type { SubagentMode, SubagentProfile, SubagentScope } from './types'

interface SubagentsState {
  /** 内置子智能体的启停状态重写，key 为 builtin id */
  builtinEnabled: Record<string, boolean>
}

export class SubagentManager {
  private listeners = new Set<() => void>()

  /** 订阅子智能体列表变动事件 */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  private notify(): void {
    for (const listener of this.listeners) {
      try {
        listener()
      } catch (err) {
        console.error('[SubagentManager listener error]', err)
      }
    }
  }

  private stateFilePath(): string {
    return join(getAppHome(), 'subagents_state.json')
  }

  /** 读取内置智能体的状态重写表 */
  private async loadState(): Promise<SubagentsState> {
    const path = this.stateFilePath()
    if (!existsSync(path)) {
      return { builtinEnabled: {} }
    }
    try {
      const raw = await readFile(path, 'utf8')
      const parsed = JSON.parse(raw)
      return {
        builtinEnabled: parsed?.builtinEnabled ?? {},
      }
    } catch {
      return { builtinEnabled: {} }
    }
  }

  /** 保存内置智能体的状态重写表 */
  private async saveState(state: SubagentsState): Promise<void> {
    const dir = getAppHome()
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true })
    }
    await writeFile(this.stateFilePath(), JSON.stringify(state, null, 2), 'utf8')
    this.notify()
  }

  /** 工作区子智能体存储目录：`${workspace}/.ada/subagents` */
  getWorkspaceDir(workspace: string): string {
    return join(workspace, '.ada', 'subagents')
  }

  /** 全局子智能体存储目录：`~/.a-da/subagents` */
  getGlobalDir(): string {
    return join(getAppHome(), 'subagents')
  }

  /**
   * 解析 JSON 格式的子智能体定义
   */
  private parseJsonSubagent(raw: string, defaultId: string, scope: SubagentScope): SubagentProfile | null {
    try {
      const data = JSON.parse(raw)
      const cleanId = String(data.id || defaultId).replace(/^(workspace|global)_/, '')
      return {
        id: `${scope}_${cleanId}`,
        name: data.name || defaultId,
        description: data.description || '',
        systemPrompt: data.systemPrompt || '',
        allowedTools: Array.isArray(data.allowedTools) ? data.allowedTools : ['list_files', 'read_file', 'search_files'],
        disallowedTools: Array.isArray(data.disallowedTools) ? data.disallowedTools : undefined,
        mode: (data.mode === 'readwrite' ? 'readwrite' : 'readonly') as SubagentMode,
        color: data.color,
        background: typeof data.background === 'boolean' ? data.background : undefined,
        maxSteps: typeof data.maxSteps === 'number' ? data.maxSteps : undefined,
        modelOverride: data.modelOverride,
        enabled: data.enabled !== false,
        scope,
        icon: data.icon || (data.mode === 'readwrite' ? 'bug' : 'search'),
        updatedAt: typeof data.updatedAt === 'number' ? data.updatedAt : Date.now(),
      }
    } catch (e) {
      console.warn(`[SubagentManager] Failed to parse JSON subagent ${defaultId}:`, e)
      return null
    }
  }

  /**
   * 解析 Markdown 格式的子智能体定义 (带 YAML Frontmatter)
   */
  private parseMarkdownSubagent(raw: string, defaultId: string, scope: SubagentScope): SubagentProfile {
    let name = defaultId
    let description = ''
    let mode: SubagentMode = 'readonly'
    let allowedTools = ['list_files', 'read_file', 'search_files']
    let disallowedTools: string[] | undefined = undefined
    let color: any = undefined
    let background: boolean | undefined = undefined
    let enabled = true
    let maxSteps: number | undefined = undefined
    let icon = 'search'
    let systemPrompt = raw.trim()

    const fmMatch = raw.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/)
    if (fmMatch) {
      const yaml = fmMatch[1]!
      systemPrompt = fmMatch[2]!.trim()
      for (const line of yaml.split('\n')) {
        const colonIdx = line.indexOf(':')
        if (colonIdx > 0) {
          const key = line.slice(0, colonIdx).trim()
          const val = line.slice(colonIdx + 1).trim().replace(/^['"]|['"]$/g, '')
          if (key === 'name') name = val
          else if (key === 'description') description = val
          else if (key === 'mode') mode = val === 'readwrite' ? 'readwrite' : 'readonly'
          else if (key === 'enabled') enabled = val !== 'false'
          else if (key === 'maxSteps') maxSteps = parseInt(val, 10) || undefined
          else if (key === 'icon') icon = val
          else if (key === 'color') color = val
          else if (key === 'background') background = val === 'true'
          else if (key === 'tools' || key === 'allowedTools') {
            allowedTools = val
              .split(',')
              .map((t) => t.trim())
              .filter(Boolean)
          } else if (key === 'disallowedTools') {
            disallowedTools = val
              .split(',')
              .map((t) => t.trim())
              .filter(Boolean)
          }
        }
      }
    }

    return {
      id: `${scope}_${defaultId}`,
      name,
      description,
      systemPrompt,
      allowedTools,
      disallowedTools,
      mode,
      color,
      background,
      maxSteps,
      enabled,
      scope,
      icon,
      updatedAt: Date.now(),
    }
  }

  private async scanDirectory(dir: string, scope: SubagentScope): Promise<SubagentProfile[]> {
    if (!existsSync(dir)) return []
    try {
      const files = await readdir(dir)
      const results: SubagentProfile[] = []
      for (const file of files) {
        const ext = extname(file).toLowerCase()
        const idName = basename(file, ext)
        const fullPath = join(dir, file)
        try {
          const raw = await readFile(fullPath, 'utf8')
          if (ext === '.json') {
            const profile = this.parseJsonSubagent(raw, idName, scope)
            if (profile) results.push(profile)
          } else if (ext === '.md') {
            results.push(this.parseMarkdownSubagent(raw, idName, scope))
          }
        } catch (e) {
          console.warn(`[SubagentManager] Failed to read subagent file ${fullPath}:`, e)
        }
      }
      return results
    } catch {
      return []
    }
  }

  private cachedProfiles = new Map<string, SubagentProfile[]>()

  /**
   * 同步获取可用子智能体（优先使用缓存，无缓存时回退至内置列表）
   */
  getSubagentsSync(workspace?: string): SubagentProfile[] {
    const key = workspace ?? '__global__'
    return this.cachedProfiles.get(key) ?? BUILTIN_SUBAGENTS
  }

  /**
   * 获取所有可用子智能体 (包含内置、当前工作区和全局配置)
   */
  async getSubagents(workspace?: string): Promise<SubagentProfile[]> {
    const state = await this.loadState()

    // 1. 处理内置预装智能体并应用启停状态重写
    const builtins: SubagentProfile[] = BUILTIN_SUBAGENTS.map((b) => ({
      ...b,
      enabled: state.builtinEnabled[b.id] !== undefined ? state.builtinEnabled[b.id] : b.enabled,
    }))

    // 2. 加载全局配置
    const globals = await this.scanDirectory(this.getGlobalDir(), 'global')

    // 3. 加载当前工作区配置
    let workspaces: SubagentProfile[] = []
    if (workspace) {
      workspaces = await this.scanDirectory(this.getWorkspaceDir(workspace), 'workspace')
    }

    const result = [...builtins, ...workspaces, ...globals]
    this.cachedProfiles.set(workspace ?? '__global__', result)
    return result
  }

  /**
   * 获取所有启用的子智能体
   */
  async getEnabledSubagents(workspace?: string): Promise<SubagentProfile[]> {
    const all = await this.getSubagents(workspace)
    return all.filter((item) => item.enabled)
  }

  /**
   * 根据 ID 查找指定子智能体
   */
  async getById(id: string, workspace?: string): Promise<SubagentProfile | undefined> {
    const all = await this.getSubagents(workspace)
    const cleanId = id.replace(/^(workspace|global)_/, '')
    return all.find(
      (item) =>
        item.id === id ||
        item.id === cleanId ||
        item.id === `workspace_${cleanId}` ||
        item.id === `global_${cleanId}`
    )
  }

  /**
   * 切换子智能体的启用/禁用状态
   */
  async toggleSubagent(id: string, enabled: boolean, workspace?: string): Promise<void> {
    const builtin = BUILTIN_SUBAGENTS.find((b) => b.id === id)
    if (builtin) {
      const state = await this.loadState()
      state.builtinEnabled[id] = enabled
      await this.saveState(state)
      return
    }

    // 自定义智能体：更新文件中的 enabled 字段
    const target = await this.getById(id, workspace)
    if (!target) return

    const dir = target.scope === 'workspace' && workspace ? this.getWorkspaceDir(workspace) : this.getGlobalDir()
    const jsonPath = join(dir, `${id.replace(/^(workspace|global)_/, '')}.json`)
    if (existsSync(jsonPath)) {
      try {
        const raw = await readFile(jsonPath, 'utf8')
        const data = JSON.parse(raw)
        data.enabled = enabled
        data.updatedAt = Date.now()
        await writeFile(jsonPath, JSON.stringify(data, null, 2), 'utf8')
        this.notify()
      } catch (e) {
        console.error('[SubagentManager] Failed to update subagent status:', e)
      }
    }
  }

  /**
   * 创建或保存自定义子智能体
   */
  async saveSubagent(
    profile: SubagentProfile,
    workspace?: string
  ): Promise<void> {
    const dir = profile.scope === 'workspace' && workspace ? this.getWorkspaceDir(workspace) : this.getGlobalDir()
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true })
    }

    const cleanId = profile.id.replace(/^(workspace|global)_/, '')
    const targetPath = join(dir, `${cleanId}.json`)
    const toWrite = {
      ...profile,
      id: cleanId,
      updatedAt: Date.now(),
    }
    await writeFile(targetPath, JSON.stringify(toWrite, null, 2), 'utf8')
    this.notify()
  }

  /**
   * 删除自定义子智能体
   */
  async deleteSubagent(id: string, workspace?: string): Promise<boolean> {
    const target = await this.getById(id, workspace)
    if (!target || target.scope === 'builtin') {
      return false
    }

    const dir = target.scope === 'workspace' && workspace ? this.getWorkspaceDir(workspace) : this.getGlobalDir()
    const cleanId = id.replace(/^(workspace|global)_/, '')
    const candidates = [join(dir, `${cleanId}.json`), join(dir, `${cleanId}.md`)]

    let deleted = false
    for (const p of candidates) {
      if (existsSync(p)) {
        try {
          await unlink(p)
          deleted = true
        } catch {}
      }
    }

    if (deleted) {
      this.notify()
    }
    return deleted
  }
}

export const defaultSubagentManager = new SubagentManager()

/**
 * 依据可用子智能体列表生成格式化提示词（供工具描述或系统提示词动态注入）
 */
export function formatProfilesPrompt(profiles: SubagentProfile[]): string {
  const active = profiles.filter((p) => p.enabled)
  if (active.length === 0) return ''

  return [
    '## 可委派子智能体列表 (Available Subagents):',
    ...active.map((p) => {
      const tools = p.allowedTools.includes('*') ? '所有工具' : p.allowedTools.join(', ')
      const disallowStr = p.disallowedTools && p.disallowedTools.length > 0 ? ` (排除: ${p.disallowedTools.join(', ')})` : ''
      const modeStr = p.mode === 'readonly' ? '只读安全' : '读写'
      return `- **${p.id}** (${p.name}, ${modeStr}): ${p.description} [可用工具: ${tools}${disallowStr}]`
    }),
    '',
    '## 委派准则与最佳实践 (Delegation Guidelines):',
    '- **自包含任务 (Self-contained task)**：子智能体在全新的独立上下文中启动，任务要求与参考信息必须完整，无法直接读取主会话未传递的隐式上下文。',
    '- **只取结论，不取大文本 (Keep the conclusion)**：子智能体完成后会自动给出精炼的高信息密度总结，主智能体负责将核心结论转述给用户，避免大量过程文本挤占上下文。',
    '- **并发委派 (Concurrent Delegation)**：若有多个独立、互不依赖的探索或验证子任务，可并发调用 invoke_subagent，系统将自动进行后台并发处理。',
    '- **双向交互与纠偏 (Steering)**：对正在运行或已完成的子智能体，可通过 send_subagent_message 发送补充要求或实时转向纠偏。',
  ].join('\n')
}
