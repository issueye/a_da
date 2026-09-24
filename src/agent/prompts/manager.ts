/**
 * 提示词管理器
 * 负责扫描、加载、创建、更新、删除以及合成当前工作区与全局的提示词。
 */

import { existsSync, mkdirSync } from 'node:fs'
import { readdir, readFile, unlink, writeFile } from 'node:fs/promises'
import { basename, extname, join } from 'node:path'
import { getAppHome } from '../home'
import { readDisabledPlugins } from '../config'
import { BUILTIN_PROMPTS } from './builtins'
import { BUILTIN_PLUGINS } from '../tools/builtin-plugins'
import type { CreatePromptOptions, PromptItem, PromptScope } from './types'
import type { AgentMode } from '../types'

interface PromptsState {
  /** 内置提示词的启停状态重写，key 为 builtin id */
  builtinEnabled: Record<string, boolean>
}

export class PromptManager {
  private stateFilePath(): string {
    return join(getAppHome(), 'prompts_state.json')
  }

  /** 读取内置提示词的状态重写表 */
  private async loadState(): Promise<PromptsState> {
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

  /** 保存内置提示词的状态重写表 */
  private async saveState(state: PromptsState): Promise<void> {
    const dir = getAppHome()
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true })
    }
    await writeFile(this.stateFilePath(), JSON.stringify(state, null, 2), 'utf8')
  }

  /** 工作区提示词存储目录：`${workspace}/.ada/prompts` */
  getWorkspaceDir(workspace: string): string {
    return join(workspace, '.ada', 'prompts')
  }

  /** 全局提示词存储目录：`~/.a-da/prompts` */
  getGlobalDir(): string {
    return join(getAppHome(), 'prompts')
  }

  /**
   * 解析 Markdown 提示词文件内容与元数据。
   * 支持 YAML Frontmatter 或自然标题解析，兼容 pi argument-hint 规范。
   */
  parseMarkdownPrompt(
    raw: string,
    defaultId: string,
    scope: PromptScope,
    filePath?: string,
    extraMeta?: { pluginName?: string; pluginId?: string }
  ): PromptItem {
    let name = defaultId
    let description = ''
    let argumentHint: string | undefined = undefined
    let isSystem = false
    let enabled = true
    let content = raw.trim()

    // 检查是否存在 --- frontmatter ---
    const fmMatch = raw.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/)
    if (fmMatch) {
      const yaml = fmMatch[1]!
      content = fmMatch[2]!.trim()
      for (const line of yaml.split('\n')) {
        const colonIdx = line.indexOf(':')
        if (colonIdx > 0) {
          const key = line.slice(0, colonIdx).trim()
          const val = line.slice(colonIdx + 1).trim().replace(/^['"]|['"]$/g, '')
          if (key === 'name') name = val
          else if (key === 'description') description = val
          else if (
            key === 'argument-hint' ||
            key === 'argument_hint' ||
            key === 'argumentHint'
          ) {
            argumentHint = val
          } else if (key === 'isSystem') isSystem = val === 'true'
          else if (key === 'enabled') enabled = val !== 'false'
        }
      }
    } else {
      // 容错：从第一行或次行提取一级标题与引用描述
      const lines = raw.split('\n')
      for (const l of lines) {
        const trimmed = l.trim()
        if (trimmed.startsWith('# ') && name === defaultId) {
          name = trimmed.slice(2).trim()
        } else if (trimmed.startsWith('> ') && !description) {
          description = trimmed.slice(2).trim()
        }
      }
    }

    if (!description) {
      const firstLine = content.split('\n').find((line) => line.trim())
      if (firstLine) {
        description = firstLine.slice(0, 60)
        if (firstLine.length > 60) description += '...'
      }
    }

    const id = extraMeta?.pluginId ? `${extraMeta.pluginId}:${defaultId}` : `${scope}_${defaultId}`

    return {
      id,
      name: name || defaultId,
      description: description || '自定义提示词',
      argumentHint,
      content,
      scope,
      enabled,
      isSystem,
      filePath,
      pluginName: extraMeta?.pluginName,
      pluginId: extraMeta?.pluginId,
      updatedAt: Date.now(),
    }
  }

  /** 将 PromptItem 序列化为规范带有 Frontmatter 的 Markdown 文本 */
  serializeToMarkdown(item: {
    name: string
    description?: string
    argumentHint?: string
    isSystem?: boolean
    enabled?: boolean
    content: string
  }): string {
    const yamlLines = [
      '---',
      `name: "${(item.name || '').replace(/"/g, '\\"')}"`,
      `description: "${(item.description || '').replace(/"/g, '\\"')}"`,
    ]
    if (item.argumentHint) {
      yamlLines.push(`argument-hint: "${item.argumentHint.replace(/"/g, '\\"')}"`)
    }
    yamlLines.push(`isSystem: ${item.isSystem ? 'true' : 'false'}`)
    yamlLines.push(`enabled: ${item.enabled !== false ? 'true' : 'false'}`)
    yamlLines.push('---', '', item.content.trim(), '')
    return yamlLines.join('\n')
  }

  /**
   * 从单个目录扫描加载所有 .md 提示词文件
   */
  private async scanDirPrompts(
    dir: string,
    scope: PromptScope,
    out: PromptItem[],
    state: PromptsState,
    disabledPlugins?: Set<string>,
    extraMeta?: { pluginName?: string; pluginId?: string }
  ): Promise<void> {
    if (!existsSync(dir)) return
    try {
      const files = await readdir(dir)
      for (const file of files) {
        if (extname(file).toLowerCase() === '.md') {
          const filePath = join(dir, file)
          try {
            const text = await readFile(filePath, 'utf8')
            const id = basename(file, extname(file))
            const item = this.parseMarkdownPrompt(text, id, scope, filePath, extraMeta)
            const overrideEnabled = state.builtinEnabled[item.id]
            if (overrideEnabled !== undefined) {
              item.enabled = overrideEnabled
            }
            if (scope === 'plugin' && extraMeta?.pluginId && disabledPlugins?.has(extraMeta.pluginId)) {
              item.enabled = false
            }
            out.push(item)
          } catch {}
        }
      }
    } catch {}
  }

  /**
   * 扫描并汇总所有可用提示词（内置 + 工作区 + 全局 + 插件包）
   */
  async scanPrompts(workspace: string): Promise<PromptItem[]> {
    const state = await this.loadState()
    const disabledPlugins = new Set(await readDisabledPlugins())
    const results: PromptItem[] = []

    // 1. 内置提示词（应用用户持久化的启停状态）
    for (const b of BUILTIN_PROMPTS) {
      const overrideEnabled = state.builtinEnabled[b.id]
      results.push({
        ...b,
        enabled: overrideEnabled !== undefined ? overrideEnabled : b.enabled,
      })
    }

    // 2. 工作区直接提示词：`${workspace}/.ada/prompts/*.md`
    if (workspace) {
      const wsDir = this.getWorkspaceDir(workspace)
      await this.scanDirPrompts(wsDir, 'workspace', results, state)
    }

    // 3. 全局直接提示词：`~/.a-da/prompts/*.md`
    const globalDir = this.getGlobalDir()
    await this.scanDirPrompts(globalDir, 'global', results, state)

    // 4. 工作区扩展插件中的提示词目录：`${workspace}/.ada/extensions/<plugin>/prompts/*.md`
    if (workspace) {
      const wsExtDir = join(workspace, '.ada', 'extensions')
      if (existsSync(wsExtDir)) {
        try {
          const entries = await readdir(wsExtDir, { withFileTypes: true })
          for (const entry of entries) {
            if (entry.isDirectory()) {
              const pluginPromptsDir = join(wsExtDir, entry.name, 'prompts')
              if (existsSync(pluginPromptsDir)) {
                await this.scanDirPrompts(
                  pluginPromptsDir,
                  'plugin',
                  results,
                  state,
                  disabledPlugins,
                  { pluginName: entry.name, pluginId: `workspace:${entry.name}` }
                )
              }
            }
          }
        } catch {}
      }
    }

    // 5. 用户全局扩展插件中的提示词目录：`~/.a-da/extensions/<plugin>/prompts/*.md`
    const globalExtDir = join(getAppHome(), 'extensions')
    if (existsSync(globalExtDir)) {
      try {
        const entries = await readdir(globalExtDir, { withFileTypes: true })
        for (const entry of entries) {
          if (entry.isDirectory()) {
            const pluginPromptsDir = join(globalExtDir, entry.name, 'prompts')
            if (existsSync(pluginPromptsDir)) {
              await this.scanDirPrompts(
                pluginPromptsDir,
                'plugin',
                results,
                state,
                disabledPlugins,
                { pluginName: entry.name, pluginId: `global:${entry.name}` }
              )
            }
          }
        }
      } catch {}
    }

    // 6. 系统官方内置插件中的提示词模板
    for (const bp of BUILTIN_PLUGINS) {
      const pluginId = `builtin:${bp.id}`
      const isPluginDisabled = disabledPlugins.has(pluginId)
      for (const p of bp.prompts || []) {
        const id = `${pluginId}:${p.name}`
        const overrideEnabled = state.builtinEnabled[id]
        let enabled = overrideEnabled !== undefined ? overrideEnabled : !isPluginDisabled
        if (isPluginDisabled) {
          enabled = false
        }
        results.push({
          id,
          name: p.name,
          description: p.description,
          argumentHint: p.argumentHint,
          content: p.content,
          scope: 'plugin',
          enabled,
          isSystem: Boolean(p.isSystem),
          pluginName: bp.name,
          pluginId,
          updatedAt: 1720000000000,
        })
      }
    }

    return results
  }

  /**
   * 按照作用域优先级（workspace > global > plugin > builtin）查找已启用的匹配提示词模板
   */
  async findPrompt(commandName: string, workspace: string): Promise<PromptItem | undefined> {
    const all = await this.scanPrompts(workspace)
    const target = commandName.trim().toLowerCase()
    const enabledPrompts = all.filter((p) => p.enabled)

    const priorityOrder: PromptScope[] = ['workspace', 'global', 'plugin', 'builtin']
    for (const scope of priorityOrder) {
      const match = enabledPrompts.find((p) => {
        if (p.scope !== scope) return false
        const nameMatch = p.name.toLowerCase() === target
        const fileMatch = p.filePath && basename(p.filePath, extname(p.filePath)).toLowerCase() === target
        return nameMatch || fileMatch
      })
      if (match) return match
    }
    return undefined
  }

  /**
   * 创建新的提示词（生成 .md 文件）
   */
  async createPrompt(workspace: string, options: CreatePromptOptions): Promise<PromptItem> {
    const targetDir = options.scope === 'workspace' ? this.getWorkspaceDir(workspace) : this.getGlobalDir()
    if (!existsSync(targetDir)) {
      mkdirSync(targetDir, { recursive: true })
    }

    // 生成安全的文件名 slug：提取英文字符，若无则使用标准时间戳标识
    const asciiSlug = options.name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, '_')
      .replace(/^_+|_+$/g, '')
      .slice(0, 24)
    const baseSlug = asciiSlug || `prompt_${Date.now().toString(36)}`
    let filename = `${baseSlug}.md`
    let counter = 1
    while (existsSync(join(targetDir, filename))) {
      filename = `${baseSlug}_${counter++}.md`
    }

    const filePath = join(targetDir, filename)
    const markdown = this.serializeToMarkdown({
      name: options.name.trim(),
      description: options.description?.trim() || '',
      argumentHint: options.argumentHint?.trim(),
      isSystem: Boolean(options.isSystem),
      enabled: options.enabled !== false,
      content: options.content,
    })

    await writeFile(filePath, markdown, 'utf8')

    const fileId = basename(filename, '.md')
    return this.parseMarkdownPrompt(markdown, fileId, options.scope, filePath)
  }

  /**
   * 更新已有提示词的内容与元数据
   */
  async updatePrompt(item: PromptItem): Promise<boolean> {
    if (item.scope === 'builtin') {
      // 内置提示词只更新其启停与系统设定状态到全局配置
      const state = await this.loadState()
      state.builtinEnabled[item.id] = item.enabled
      await this.saveState(state)
      return true
    }

    if (!item.filePath || !existsSync(item.filePath)) {
      return false
    }

    const markdown = this.serializeToMarkdown({
      name: item.name,
      description: item.description,
      argumentHint: item.argumentHint,
      isSystem: item.isSystem,
      enabled: item.enabled,
      content: item.content,
    })

    await writeFile(item.filePath, markdown, 'utf8')
    return true
  }

  /**
   * 快速切换提示词的启用/停用状态
   */
  async togglePrompt(id: string, enabled: boolean, workspace: string): Promise<boolean> {
    if (id.startsWith('builtin-') || id.includes(':')) {
      const state = await this.loadState()
      state.builtinEnabled[id] = enabled
      await this.saveState(state)
      return true
    }

    const all = await this.scanPrompts(workspace)
    const target = all.find((p) => p.id === id)
    if (!target) return false

    if (target.scope === 'plugin') {
      const state = await this.loadState()
      state.builtinEnabled[id] = enabled
      await this.saveState(state)
      return true
    }

    if (!target.filePath) return false
    target.enabled = enabled
    return this.updatePrompt(target)
  }

  /**
   * 删除提示词文件
   */
  async deletePrompt(filePath: string): Promise<boolean> {
    try {
      if (existsSync(filePath)) {
        await unlink(filePath)
        return true
      }
      return false
    } catch {
      return false
    }
  }

  private cachedCompositePrompts = new Map<string, string>()

  /**
   * 合成当前已启用的系统提示词（供 Agent 会话循环消费）
   */
  async getCompositeSystemPrompt(workspace: string, mode: AgentMode = 'code'): Promise<string> {
    const all = await this.scanPrompts(workspace)
    const enabledSystemPrompts = all.filter((p) => p.enabled && p.isSystem && p.content.trim())

    const sections: string[] = []

    for (const p of enabledSystemPrompts) {
      sections.push(`【系统规范/角色预设：${p.name}】\n${p.content.trim()}`)
    }

    // 注入协作模式专属指导规范
    if (mode === 'plan') {
      sections.push(`【协作模式：Plan 规划模式】
当前处于只读架构规划模式。
你的目标是专注于需求分析、技术选型、架构梳理与实施计划设计。
核心准则：
1. 本模式下禁止直接修改工作区代码或执行外部命令；
2. 请调用只读分析工具（read_file, list_files, search_files, read_url_content, Skill 等）深入调研系统现状；
3. 输出条理清晰、步骤可执行的结构化方案，并引导用户切换到 Code（编码）模式执行具体修改。`)
    } else if (mode === 'create') {
      sections.push(`【协作模式：Create 创造与元开发模式】
当前处于智能体自扩展与元开发模式。
你被赋予了自我进化的超级能力：
1. 你可以使用 manage_tool 工具自主编写、调试与更新 TypeScript 扩展工具插件；
2. 你可以使用 manage_skill 工具自主创建、更新与优化专业领域技能规范（SKILL.md）；
3. 根据用户的自然语言诉求，规划并生成最适合的自定义工具或技能，并在生成后告知用户其使用方式。`)
    } else {
      sections.push(`【协作模式：Code 编码模式 (Vibe Coding)】
当前处于全能敏捷编码模式。
遵循 Vibe Coding 核心哲学：极速切入、原子改动、测试驱动、保持代码整洁现代，高质量交付用户所需的功能与修改。`)
    }

    // 自动接入已启用的专业技能（Skills）元数据摘要段，引导大模型按需调用 Skill 工具
    try {
      const { defaultSkillManager } = await import('../skills')
      const skillsCtx = await defaultSkillManager.buildSkillsPrompt(workspace)
      if (skillsCtx.prompt) {
        sections.push(skillsCtx.prompt)
      }
    } catch {}

    const result = sections.join('\n\n---\n\n')
    this.cachedCompositePrompts.set(`${workspace}:${mode}`, result)
    return result
  }

  /**
   * 同步获取当前缓存的合成系统提示词（用于 UI 遥测指标实时计算与 Token 分解）
   */
  getCompositeSystemPromptSync(workspace?: string, mode: AgentMode = 'code'): string {
    const ws = workspace || process.cwd()
    const key = `${ws}:${mode}`
    if (this.cachedCompositePrompts.has(key)) {
      return this.cachedCompositePrompts.get(key)!
    }
    return `【系统规范/角色预设：全能开发助手】\n遵循代码安全与最佳工程实践。\n\n---\n\n【协作模式：${mode}】遵循敏捷开发与系统准则。`
  }
}

export const defaultPromptManager = new PromptManager()
