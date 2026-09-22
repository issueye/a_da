/**
 * SKILL 系统核心管理器
 * 负责技能发现、解析、启停配置、提示词生成与模板管理
 * 参考 ZCode SkillsService 设计
 */

import { existsSync, readdirSync } from 'node:fs'
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { basename, dirname, join } from 'node:path'
import { getAppHome } from '../home'
import { expandSkillVariables, parseSkillMarkdown } from './parser'
import { BUILTIN_SKILLS } from './builtins'
import type { SkillDiagnostic, SkillsPromptContext, SkillSummary } from './types'

export const SKILL_FILE_NAME = 'SKILL.md'

interface SkillsState {
  enabledState: Record<string, boolean>
}

export class SkillManager {
  private listeners = new Set<() => void>()
  private cache: Map<string, SkillSummary[]> = new Map()

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
        console.error('[SkillManager listener error]', err)
      }
    }
  }

  private stateFilePath(): string {
    return join(getAppHome(), 'skills_state.json')
  }

  private async loadState(): Promise<SkillsState> {
    const path = this.stateFilePath()
    if (!existsSync(path)) {
      return { enabledState: {} }
    }
    try {
      const raw = await readFile(path, 'utf8')
      const parsed = JSON.parse(raw)
      return { enabledState: parsed?.enabledState ?? {} }
    } catch {
      return { enabledState: {} }
    }
  }

  private async saveState(state: SkillsState): Promise<void> {
    const path = this.stateFilePath()
    await mkdir(dirname(path), { recursive: true })
    await writeFile(path, JSON.stringify(state, null, 2), 'utf8')
  }

  /** 获取技能根目录列表 */
  private getSkillRoots(workspaceRoot?: string): Array<{ path: string; scope: 'workspace' | 'global' | 'plugin'; pluginName?: string }> {
    const roots: Array<{ path: string; scope: 'workspace' | 'global' | 'plugin'; pluginName?: string }> = []

    if (workspaceRoot) {
      // 1. 工作区 .ada/skills
      roots.push({ path: join(workspaceRoot, '.ada', 'skills'), scope: 'workspace' })
      // 兼容 .agents/skills
      roots.push({ path: join(workspaceRoot, '.agents', 'skills'), scope: 'workspace' })

      // 2. 工作区插件中的 skills/ 目录: .ada/extensions/<plugin>/skills
      const wsExtDir = join(workspaceRoot, '.ada', 'extensions')
      if (existsSync(wsExtDir)) {
        try {
          const entries = readdirSync(wsExtDir, { withFileTypes: true })
          for (const entry of entries) {
            if (entry.isDirectory()) {
              const pluginSkillDir = join(wsExtDir, entry.name, 'skills')
              if (existsSync(pluginSkillDir)) {
                roots.push({ path: pluginSkillDir, scope: 'plugin', pluginName: entry.name })
              }
            }
          }
        } catch {}
      }
    }

    // 3. 用户全局 ~/.ada/skills
    const appHome = getAppHome()
    roots.push({ path: join(appHome, 'skills'), scope: 'global' })

    // 4. 全局插件目录下的 skills: ~/.ada/extensions/<plugin>/skills
    const globalExtDir = join(appHome, 'extensions')
    if (existsSync(globalExtDir)) {
      try {
        const entries = readdirSync(globalExtDir, { withFileTypes: true })
        for (const entry of entries) {
          if (entry.isDirectory()) {
            const pluginSkillDir = join(globalExtDir, entry.name, 'skills')
            if (existsSync(pluginSkillDir)) {
              roots.push({ path: pluginSkillDir, scope: 'plugin', pluginName: entry.name })
            }
          }
        }
      } catch {}
    }

    return roots
  }

  /** 递归发现指定目录下的所有 SKILL.md 文件 */
  private findSkillFiles(dir: string, depth = 0): string[] {
    if (depth > 3 || !existsSync(dir)) return []
    const results: string[] = []

    try {
      const entries = readdirSync(dir, { withFileTypes: true })
      for (const entry of entries) {
        const fullPath = join(dir, entry.name)
        if (entry.isFile() && entry.name.toLowerCase() === SKILL_FILE_NAME.toLowerCase()) {
          results.push(fullPath)
        } else if (entry.isDirectory() && !entry.name.startsWith('.') && entry.name !== 'node_modules') {
          results.push(...this.findSkillFiles(fullPath, depth + 1))
        }
      }
    } catch {}

    return results
  }

  /** 扫描并发现所有可用技能 */
  async scanSkills(workspaceRoot?: string): Promise<SkillSummary[]> {
    const state = await this.loadState()
    const roots = this.getSkillRoots(workspaceRoot)
    const skills: SkillSummary[] = []
    const seenNames = new Set<string>()

    for (const root of roots) {
      if (!existsSync(root.path)) continue
      const skillFiles = this.findSkillFiles(root.path)

      for (const filePath of skillFiles) {
        try {
          const raw = await readFile(filePath, 'utf8')
          const parsed = parseSkillMarkdown(raw, filePath)
          const baseDir = dirname(filePath)
          const name = parsed.metadata.name.trim()

          // 作用域优先级去重：工作区 > 全局 > 插件
          const dedupeKey = name.toLowerCase()
          if (seenNames.has(dedupeKey)) continue
          seenNames.add(dedupeKey)

          const id = `${root.scope}:${name}`
          const enabled = state.enabledState[id] ?? true

          skills.push({
            id,
            name,
            description: parsed.metadata.description,
            body: parsed.body,
            path: filePath,
            baseDirectory: baseDir,
            scope: root.scope,
            enabled,
            pluginName: root.pluginName,
            metadata: parsed.metadata,
          })
        } catch (err) {
          console.warn(`[SkillManager] 读取技能文件失败 ${filePath}:`, err)
        }
      }
    }

    // 5. 注入系统内置预设技能（若未被工作区或全局同名覆盖）
    for (const builtin of BUILTIN_SKILLS) {
      const dedupeKey = builtin.name.toLowerCase()
      if (seenNames.has(dedupeKey)) continue
      seenNames.add(dedupeKey)

      const parsed = parseSkillMarkdown(builtin.content, `(builtin):${builtin.name}`)
      const id = `builtin:${builtin.name}`
      const enabled = state.enabledState[id] ?? true
      skills.push({
        id,
        name: builtin.name,
        description: parsed.metadata.description || builtin.description,
        body: parsed.body,
        path: `(builtin):${builtin.name}`,
        baseDirectory: '',
        scope: 'builtin',
        enabled,
        metadata: parsed.metadata,
      })
    }

    // 排序：按名称字母升序
    skills.sort((a, b) => a.name.localeCompare(b.name))

    const cacheKey = workspaceRoot || 'global'
    this.cache.set(cacheKey, skills)
    return skills
  }

  /** 获取所有启用的技能 */
  async getEnabledSkills(workspaceRoot?: string): Promise<SkillSummary[]> {
    const all = await this.scanSkills(workspaceRoot)
    return all.filter((s) => s.enabled)
  }

  /** 切换技能启用状态 */
  async toggleSkill(id: string, enabled: boolean): Promise<void> {
    const state = await this.loadState()
    state.enabledState[id] = enabled
    await this.saveState(state)
    this.cache.clear()
    this.notify()
  }

  /** 根据名称查找已启用的技能 */
  async findSkillByName(name: string, workspaceRoot?: string): Promise<SkillSummary | undefined> {
    const skills = await this.getEnabledSkills(workspaceRoot)
    const target = name.trim().toLowerCase()
    return skills.find((s) => s.name.toLowerCase() === target)
  }

  /** 加载技能展开后的执行正文（供 Skill 工具或 Prompt 使用） */
  async loadSkillContent(
    name: string,
    workspaceRoot?: string
  ): Promise<{ name: string; content: string; baseDirectory: string; path: string } | null> {
    const skill = await this.findSkillByName(name, workspaceRoot)
    if (!skill) return null

    const content = expandSkillVariables(skill.body, skill.baseDirectory)
    return {
      name: skill.name,
      content,
      baseDirectory: skill.baseDirectory,
      path: skill.path,
    }
  }

  /**
   * 生成给大模型系统提示词的可用技能描述段
   * 遵循 ZCode 与 Claude Code 工业标准
   */
  async buildSkillsPrompt(workspaceRoot?: string): Promise<SkillsPromptContext> {
    const enabled = await this.getEnabledSkills(workspaceRoot)
    if (enabled.length === 0) {
      return { prompt: '', activatedSkillNames: [] }
    }

    const lines: string[] = [
      '### 可用技能库 (Available Skills)',
      '当用户提出相应领域任务，或通过 `/<skill-name>` 提及某项技能时，请调用 `Skill` 工具加载其操作规范：',
      '',
    ]

    for (const skill of enabled) {
      const desc = skill.metadata?.whenToUse
        ? `${skill.description}（适用场景：${skill.metadata.whenToUse}）`
        : skill.description
      lines.push(`- **${skill.name}**: ${desc} (来源: ${skill.scope === 'workspace' ? '工作区' : skill.scope === 'plugin' ? `插件[${skill.pluginName}]` : '全局'})`)
    }

    lines.push('')
    lines.push('【重要】：请在开始执行对应专业任务前先通过 `Skill` 工具加载该技能获取详细步骤。')

    return {
      prompt: lines.join('\n'),
      activatedSkillNames: enabled.map((s) => s.name),
    }
  }

  /** 创建新技能模板 */
  async createSkillTemplate(params: {
    name: string
    description: string
    scope: 'workspace' | 'global'
    workspaceRoot?: string
    body?: string
  }): Promise<string> {
    const { name, description, scope, workspaceRoot, body } = params
    const safeName = name.trim().toLowerCase().replace(/[^a-z0-9_-]/g, '-')

    let targetDir: string
    if (scope === 'workspace') {
      if (!workspaceRoot) throw new Error('未指定工作区路径')
      targetDir = join(workspaceRoot, '.ada', 'skills', safeName)
    } else {
      targetDir = join(getAppHome(), 'skills', safeName)
    }

    await mkdir(targetDir, { recursive: true })
    const filePath = join(targetDir, SKILL_FILE_NAME)

    const defaultBody = body || [
      '## 目标与能力概述',
      `本技能专注于 ${description}。`,
      '',
      '## 执行步骤规范',
      '1. 首先分析当前上下文与输入参数；',
      '2. 执行必要的检查与代码检索；',
      '3. 按照标准输出规范生成结果。',
    ].join('\n')

    const templateContent = [
      '---',
      `name: ${safeName}`,
      `description: ${description.trim()}`,
      `version: 0.1.0`,
      '---',
      '',
      `# 技能规范：${safeName}`,
      '',
      defaultBody,
      '',
    ].join('\n')

    await writeFile(filePath, templateContent, 'utf8')
    this.cache.clear()
    this.notify()
    return filePath
  }

  /** 删除本地技能 */
  async deleteSkill(id: string, workspaceRoot?: string): Promise<void> {
    const skills = await this.scanSkills(workspaceRoot)
    const skill = skills.find((s) => s.id === id || s.name === id)
    if (!skill) throw new Error(`未找到技能: ${id}`)
    if (skill.scope === 'builtin') throw new Error('无法删除系统内置预装技能，支持按需停用')
    if (skill.scope === 'plugin') throw new Error('无法删除插件内建的技能，请在插件管理中卸载或停用对应插件')

    await rm(skill.baseDirectory, { recursive: true, force: true })
    this.cache.clear()
    this.notify()
  }

  /** 更新已有技能的元数据或指令正文 */
  async updateSkill(
    id: string,
    updates: { description?: string; body?: string; whenToUse?: string },
    workspaceRoot?: string
  ): Promise<string> {
    const skills = await this.scanSkills(workspaceRoot)
    const skill = skills.find((s) => s.id === id || s.name === id)
    if (!skill) throw new Error(`未找到技能: ${id}`)
    if (skill.scope === 'builtin') {
      throw new Error('无法直接修改系统内置预装技能，请在当前工作区新建同名技能覆盖')
    }
    if (skill.scope === 'plugin') {
      throw new Error('无法直接修改插件内建技能，请在当前工作区新建同名技能覆盖')
    }

    const currentRaw = await readFile(skill.path, 'utf8')
    const parsed = parseSkillMarkdown(currentRaw, skill.path)
    if (updates.description !== undefined) parsed.metadata.description = updates.description.trim()
    if (updates.whenToUse !== undefined) parsed.metadata.whenToUse = updates.whenToUse.trim()
    const nextBody = updates.body !== undefined ? updates.body.trim() : parsed.body

    const frontmatterLines = ['---', `name: ${parsed.metadata.name}`]
    if (parsed.metadata.description) frontmatterLines.push(`description: ${parsed.metadata.description}`)
    if (parsed.metadata.whenToUse) frontmatterLines.push(`whenToUse: ${parsed.metadata.whenToUse}`)
    if (parsed.metadata.version) frontmatterLines.push(`version: ${parsed.metadata.version}`)
    frontmatterLines.push('---', '', nextBody, '')

    await writeFile(skill.path, frontmatterLines.join('\n'), 'utf8')
    this.cache.clear()
    this.notify()
    return skill.path
  }
}

export const defaultSkillManager = new SkillManager()
