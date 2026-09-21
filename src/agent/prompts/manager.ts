/**
 * 提示词管理器
 * 负责扫描、加载、创建、更新、删除以及合成当前工作区与全局的提示词。
 */

import { existsSync, mkdirSync } from 'node:fs'
import { readdir, readFile, unlink, writeFile } from 'node:fs/promises'
import { basename, extname, join } from 'node:path'
import { getAppHome } from '../home'
import { BUILTIN_PROMPTS } from './builtins'
import type { CreatePromptOptions, PromptItem, PromptScope } from './types'

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
   * 支持 YAML Frontmatter 或自然标题解析。
   */
  parseMarkdownPrompt(raw: string, defaultId: string, scope: PromptScope, filePath?: string): PromptItem {
    let name = defaultId
    let description = ''
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
          else if (key === 'isSystem') isSystem = val === 'true'
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

    return {
      id: `${scope}_${defaultId}`,
      name: name || defaultId,
      description: description || '自定义提示词',
      content,
      scope,
      enabled,
      isSystem,
      filePath,
      updatedAt: Date.now(),
    }
  }

  /** 将 PromptItem 序列化为规范带有 Frontmatter 的 Markdown 文本 */
  serializeToMarkdown(item: {
    name: string
    description?: string
    isSystem?: boolean
    enabled?: boolean
    content: string
  }): string {
    const yaml = [
      '---',
      `name: "${(item.name || '').replace(/"/g, '\\"')}"`,
      `description: "${(item.description || '').replace(/"/g, '\\"')}"`,
      `isSystem: ${item.isSystem ? 'true' : 'false'}`,
      `enabled: ${item.enabled !== false ? 'true' : 'false'}`,
      '---',
      '',
      item.content.trim(),
      '',
    ].join('\n')
    return yaml
  }

  /**
   * 扫描并汇总所有可用提示词（内置 + 工作区 + 全局）
   */
  async scanPrompts(workspace: string): Promise<PromptItem[]> {
    const state = await this.loadState()
    const results: PromptItem[] = []

    // 1. 内置提示词（应用用户持久化的启停状态）
    for (const b of BUILTIN_PROMPTS) {
      const overrideEnabled = state.builtinEnabled[b.id]
      results.push({
        ...b,
        enabled: overrideEnabled !== undefined ? overrideEnabled : b.enabled,
      })
    }

    // 2. 工作区提示词
    const wsDir = this.getWorkspaceDir(workspace)
    if (existsSync(wsDir)) {
      try {
        const files = await readdir(wsDir)
        for (const file of files) {
          if (extname(file).toLowerCase() === '.md') {
            const filePath = join(wsDir, file)
            try {
              const text = await readFile(filePath, 'utf8')
              const id = basename(file, extname(file))
              results.push(this.parseMarkdownPrompt(text, id, 'workspace', filePath))
            } catch {}
          }
        }
      } catch {}
    }

    // 3. 全局提示词
    const globalDir = this.getGlobalDir()
    if (existsSync(globalDir)) {
      try {
        const files = await readdir(globalDir)
        for (const file of files) {
          if (extname(file).toLowerCase() === '.md') {
            const filePath = join(globalDir, file)
            try {
              const text = await readFile(filePath, 'utf8')
              const id = basename(file, extname(file))
              results.push(this.parseMarkdownPrompt(text, id, 'global', filePath))
            } catch {}
          }
        }
      } catch {}
    }

    return results
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
    if (id.startsWith('builtin-')) {
      const state = await this.loadState()
      state.builtinEnabled[id] = enabled
      await this.saveState(state)
      return true
    }

    const all = await this.scanPrompts(workspace)
    const target = all.find((p) => p.id === id)
    if (!target || !target.filePath) return false

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

  /**
   * 合成当前已启用的系统提示词（供 Agent 会话循环消费）
   */
  async getCompositeSystemPrompt(workspace: string): Promise<string> {
    const all = await this.scanPrompts(workspace)
    const enabledSystemPrompts = all.filter((p) => p.enabled && p.isSystem && p.content.trim())

    if (enabledSystemPrompts.length === 0) {
      return ''
    }

    const sections = enabledSystemPrompts.map((p) => {
      return `【系统规范/角色预设：${p.name}】\n${p.content.trim()}`
    })

    return sections.join('\n\n---\n\n')
  }
}

export const defaultPromptManager = new PromptManager()
