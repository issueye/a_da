/**
 * 基于 jiti 的 TypeScript 扩展加载器
 * 统一管理项目工作区 (.ada/extensions) 与全局用户目录 (~/.a-da/extensions) 的扩展插件，
 * 支持动态加载、插件扫描、启用/停用、新建模板以及删除插件。
 */

import { existsSync, mkdirSync, readdirSync, rmSync, statSync, unlinkSync, writeFileSync, type Dirent } from 'node:fs'
import { basename, join } from 'node:path'
import { createJiti } from 'jiti'
import { readDisabledPlugins, saveDisabledPlugins } from '../config'
import type { AgentEvent, AgentTool } from '../core/types'
import { getAppHome } from '../home'
import { defaultToolRegistry } from './registry'
import { defaultSkillManager, type SkillSummary } from '../skills'
import { defaultPromptManager, type PromptItem } from '../prompts'
import { BUILTIN_PLUGINS } from './builtin-plugins'

/**
 * 传递给扩展插件的完整上下文 API
 */
export interface ExtensionContext {
  /** 当前项目工作区根目录 */
  workspace: string
  /** 向系统调试日志面板写入自定义追踪埋点 */
  trace: (message: string) => void
  /** 注册自定义 Agent 工具 */
  registerTool: (tool: AgentTool) => void
  /** 订阅 Agent 生命周期事件点位 (agent_start, turn_end, tool_execution_* 等) */
  onEvent: (listener: (event: AgentEvent) => void) => () => void
}

export type ExtensionFunction = (
  ctx: ExtensionContext
) => void | Promise<void> | AgentTool | ((workspace: string) => AgentTool)

export interface ExtensionModule {
  tool?: AgentTool | ((workspace: string) => AgentTool)
  tools?: Array<AgentTool | ((workspace: string) => AgentTool)>
  default?:
    | AgentTool
    | ((workspace: string) => AgentTool)
    | ExtensionFunction
    | Array<AgentTool | ((workspace: string) => AgentTool)>
}

export interface PluginToolInfo {
  name: string
  description: string
  parameters?: Record<string, unknown>
  isWrite: boolean
}

export interface PluginItem {
  id: string
  name: string
  fileName: string
  filePath: string
  scope: 'builtin' | 'workspace' | 'global'
  enabled: boolean
  tools: PluginToolInfo[]
  /** 插件包内包含的技能列表（将 SKILL 归纳到插件系统中） */
  skills: SkillSummary[]
  /** 插件包内包含的提示词列表（将提示词归纳到插件系统中） */
  prompts: PromptItem[]
  /** 是否为复合插件包目录（包含 skills/、prompts/ 或独立子目录） */
  isPackage?: boolean
  error?: string
  sizeBytes: number
  updatedAt: number
}

export class ExtensionLoader {
  private jitiInstance = createJiti(import.meta.url)
  private eventListeners = new Set<(event: AgentEvent) => void>()
  private traceHandler?: (msg: string) => void

  /**
   * 绑定宿主环境的打点与事件分发管道
   */
  bindHost(trace: (msg: string) => void) {
    this.traceHandler = trace
  }

  /**
   * 将 Agent 运行时产生的生命周期事件派发给所有扩展脚本
   */
  dispatchAgentEvent(event: AgentEvent) {
    for (const listener of this.eventListeners) {
      try {
        listener(event)
      } catch (err) {
        console.error('[Extension Event Error]', err)
      }
    }
  }

  /**
   * 从指定模块中解析并提取导出的工具列表
   */
  private async extractToolsFromModule(
    mod: ExtensionModule,
    context: ExtensionContext
  ): Promise<AgentTool[]> {
    const tools: AgentTool[] = []

    if (typeof mod.default === 'function') {
      const fnResult = await (mod.default as any)(context)
      if (fnResult && typeof fnResult === 'object' && 'name' in fnResult) {
        tools.push(fnResult as AgentTool)
      }
    }

    const candidates = [
      mod.tool,
      ...(mod.tools || []),
      ...(Array.isArray(mod.default) ? mod.default : []),
    ].filter(Boolean)

    for (const candidate of candidates) {
      let toolInstance: AgentTool | undefined
      if (typeof candidate === 'function') {
        toolInstance = (candidate as any)(context.workspace)
      } else if (candidate && typeof candidate === 'object' && 'name' in candidate) {
        toolInstance = candidate as AgentTool
      }
      if (toolInstance) {
        tools.push(toolInstance)
      }
    }

    return tools
  }

  /**
   * 扫描工作区与全局目录的所有扩展插件元数据（包含单文件插件与复合插件包）
   */
  async scanPlugins(workspace: string): Promise<PluginItem[]> {
    const disabledList = new Set(await readDisabledPlugins())
    const projectExtDir = join(workspace, '.ada', 'extensions')
    const globalExtDir = join(getAppHome(), 'extensions')
    const allSkills = await defaultSkillManager.scanSkills(workspace)
    const allPrompts = await defaultPromptManager.scanPrompts(workspace)

    const items: PluginItem[] = []

    // 0. 系统官方内置插件 (Built-in Plugins)
    for (const bp of BUILTIN_PLUGINS) {
      const id = `builtin:${bp.id}`
      const enabled = !disabledList.has(id)
      const matchingSkills = allSkills.filter(
        (s) => s.scope === 'plugin' && (s.pluginId === id || s.pluginName === bp.name)
      )
      const matchingPrompts = allPrompts.filter(
        (p) => p.scope === 'plugin' && (p.pluginId === id || p.pluginName === bp.name)
      )

      items.push({
        id,
        name: bp.name,
        fileName: `${bp.id} (内置)`,
        filePath: `(builtin):${bp.id}`,
        scope: 'builtin',
        enabled,
        tools: bp.tools.map((t) => {
          const toolInst = typeof t === 'function' ? t(workspace) : t
          return {
            name: toolInst.name,
            description: toolInst.description,
            parameters: toolInst.parameters as Record<string, unknown> | undefined,
            isWrite: defaultToolRegistry.isWriteTool(toolInst.name),
          }
        }),
        skills: matchingSkills,
        prompts: matchingPrompts,
        isPackage: true,
        sizeBytes: 0,
        updatedAt: Date.now(),
      })
    }

    const scanDir = async (dirPath: string, scope: 'workspace' | 'global') => {
      if (!existsSync(dirPath)) return
      let entries: Dirent[] = []
      try {
        entries = readdirSync(dirPath, { withFileTypes: true })
      } catch {
        return
      }

      for (const entry of entries) {
        if (entry.name.startsWith('.') || entry.name === 'node_modules') continue
        const fullPath = join(dirPath, entry.name)

        if (entry.isFile() && (entry.name.endsWith('.ts') || entry.name.endsWith('.js'))) {
          // 单文件插件
          const id = `${scope}:${entry.name}`
          const name = basename(entry.name).replace(/\.[^.]+$/, '')
          let sizeBytes = 0
          let updatedAt = Date.now()

          try {
            const st = statSync(fullPath)
            sizeBytes = st.size
            updatedAt = st.mtimeMs
          } catch {}

          const enabled = !disabledList.has(id)
          const matchingSkills = allSkills.filter(
            (s) => s.scope === 'plugin' && (s.pluginId === id || s.pluginName === name)
          )
          const matchingPrompts = allPrompts.filter(
            (p) => p.scope === 'plugin' && (p.pluginId === id || p.pluginName === name)
          )

          const item: PluginItem = {
            id,
            name,
            fileName: entry.name,
            filePath: fullPath,
            scope,
            enabled,
            tools: [],
            skills: matchingSkills,
            prompts: matchingPrompts,
            isPackage: false,
            sizeBytes,
            updatedAt,
          }

          try {
            const registeredTools: AgentTool[] = []
            const mockContext: ExtensionContext = {
              workspace,
              trace: () => {},
              registerTool: (t) => registeredTools.push(t),
              onEvent: () => () => {},
            }

            const mod = (await this.jitiInstance.import(fullPath)) as ExtensionModule
            const extracted = await this.extractToolsFromModule(mod, mockContext)
            const allTools = [...registeredTools, ...extracted]

            item.tools = allTools.map((t) => ({
              name: t.name,
              description: t.description,
              parameters: t.parameters as Record<string, unknown> | undefined,
              isWrite: defaultToolRegistry.isWriteTool(t.name),
            }))
          } catch (err) {
            item.error = (err as Error).message
          }

          items.push(item)
        } else if (entry.isDirectory()) {
          // 复合能力插件包目录（支持同时包含 tools、skills 与 prompts）
          const id = `${scope}:${entry.name}`
          const name = entry.name
          let sizeBytes = 0
          let updatedAt = Date.now()

          try {
            const st = statSync(fullPath)
            sizeBytes = st.size
            updatedAt = st.mtimeMs
          } catch {}

          const enabled = !disabledList.has(id)
          const matchingSkills = allSkills.filter(
            (s) => s.scope === 'plugin' && (s.pluginId === id || s.pluginName === name)
          )
          const matchingPrompts = allPrompts.filter(
            (p) => p.scope === 'plugin' && (p.pluginId === id || p.pluginName === name)
          )

          const candidateFiles = [
            join(fullPath, 'index.ts'),
            join(fullPath, 'index.js'),
            join(fullPath, 'tools.ts'),
            join(fullPath, 'tool.ts'),
            join(fullPath, `${name}.ts`),
          ]
          const scriptEntry = candidateFiles.find((f) => existsSync(f))

          const item: PluginItem = {
            id,
            name,
            fileName: entry.name,
            filePath: scriptEntry || fullPath,
            scope,
            enabled,
            tools: [],
            skills: matchingSkills,
            prompts: matchingPrompts,
            isPackage: true,
            sizeBytes,
            updatedAt,
          }

          if (scriptEntry) {
            try {
              const registeredTools: AgentTool[] = []
              const mockContext: ExtensionContext = {
                workspace,
                trace: () => {},
                registerTool: (t) => registeredTools.push(t),
                onEvent: () => () => {},
              }

              const mod = (await this.jitiInstance.import(scriptEntry)) as ExtensionModule
              const extracted = await this.extractToolsFromModule(mod, mockContext)
              const allTools = [...registeredTools, ...extracted]

              item.tools = allTools.map((t) => ({
                name: t.name,
                description: t.description,
                parameters: t.parameters as Record<string, unknown> | undefined,
                isWrite: defaultToolRegistry.isWriteTool(t.name),
              }))
            } catch (err) {
              item.error = (err as Error).message
            }
          }

          if (
            item.tools.length > 0 ||
            item.skills.length > 0 ||
            item.prompts.length > 0 ||
            existsSync(join(fullPath, 'skills')) ||
            existsSync(join(fullPath, 'prompts')) ||
            scriptEntry
          ) {
            items.push(item)
          }
        }
      }
    }

    await scanDir(projectExtDir, 'workspace')
    await scanDir(globalExtDir, 'global')

    return items
  }

  /**
   * 从指定目录动态扫描并加载 TypeScript/JavaScript 扩展模块并注册工具（支持单文件与目录包）
   */
  async loadExtensionsFromDir(
    dirPath: string,
    workspace: string,
    disabledSet: Set<string> = new Set(),
    scope: 'workspace' | 'global' = 'workspace'
  ): Promise<string[]> {
    if (!existsSync(dirPath)) return []

    const loadedNames: string[] = []
    let entries: Dirent[] = []
    try {
      entries = readdirSync(dirPath, { withFileTypes: true })
    } catch {
      return []
    }

    const context: ExtensionContext = {
      workspace,
      trace: (msg: string) => {
        if (this.traceHandler) this.traceHandler(`[扩展] ${msg}`)
      },
      registerTool: (tool: AgentTool) => {
        defaultToolRegistry.register(tool)
        loadedNames.push(tool.name)
      },
      onEvent: (listener: (event: AgentEvent) => void) => {
        this.eventListeners.add(listener)
        return () => this.eventListeners.delete(listener)
      },
    }

    for (const entry of entries) {
      if (entry.name.startsWith('.') || entry.name === 'node_modules') continue
      const id = `${scope}:${entry.name}`
      if (disabledSet.has(id)) continue

      let scriptToLoad: string | null = null
      if (entry.isFile() && (entry.name.endsWith('.ts') || entry.name.endsWith('.js'))) {
        scriptToLoad = join(dirPath, entry.name)
      } else if (entry.isDirectory()) {
        const fullDir = join(dirPath, entry.name)
        const candidates = [
          join(fullDir, 'index.ts'),
          join(fullDir, 'index.js'),
          join(fullDir, 'tools.ts'),
          join(fullDir, 'tool.ts'),
          join(fullDir, `${entry.name}.ts`),
        ]
        scriptToLoad = candidates.find((f) => existsSync(f)) || null
      }

      if (!scriptToLoad) continue

      try {
        const mod = (await this.jitiInstance.import(scriptToLoad)) as ExtensionModule
        const extracted = await this.extractToolsFromModule(mod, context)
        for (const t of extracted) {
          defaultToolRegistry.register(t)
          loadedNames.push(t.name)
        }
      } catch (err) {
        console.warn(`[ExtensionLoader] 加载扩展失败 ${entry.name}:`, (err as Error).message)
      }
    }

    return loadedNames
  }

  /**
   * 自动重新加载工作区及全局未禁用的扩展插件
   */
  async autoLoadExtensions(workspace: string): Promise<string[]> {
    defaultToolRegistry.clearCustomTools()
    const disabledList = await readDisabledPlugins()
    const disabledSet = new Set(disabledList)

    const loadedNames: string[] = []

    // 0. 加载启用的官方内置插件工具
    for (const bp of BUILTIN_PLUGINS) {
      const id = `builtin:${bp.id}`
      if (disabledSet.has(id)) continue
      for (const t of bp.tools) {
        const toolInst = typeof t === 'function' ? t(workspace) : t
        defaultToolRegistry.register(toolInst)
        loadedNames.push(toolInst.name)
      }
    }

    const projectExtDir = join(workspace, '.ada', 'extensions')
    const globalExtDir = join(getAppHome(), 'extensions')

    const projectLoaded = await this.loadExtensionsFromDir(
      projectExtDir,
      workspace,
      disabledSet,
      'workspace'
    )
    const globalLoaded = await this.loadExtensionsFromDir(
      globalExtDir,
      workspace,
      disabledSet,
      'global'
    )

    return [...loadedNames, ...projectLoaded, ...globalLoaded]
  }

  /**
   * 切换插件的启用/禁用状态并持久化
   */
  async togglePlugin(pluginId: string, enabled: boolean, workspace: string): Promise<void> {
    const disabled = await readDisabledPlugins()
    const nextSet = new Set(disabled)
    if (enabled) {
      nextSet.delete(pluginId)
    } else {
      nextSet.add(pluginId)
    }
    await saveDisabledPlugins(Array.from(nextSet))
    await this.autoLoadExtensions(workspace)
  }

  /**
   * 快速创建一个扩展插件模板
   */
  async createPluginTemplate(
    workspace: string,
    scope: 'workspace' | 'global',
    rawName: string,
    customCode?: string
  ): Promise<string> {
    const cleanName = rawName.trim().replace(/\.[^.]+$/, '').replace(/[^a-zA-Z0-9_-]/g, '_')
    const fileName = `${cleanName || 'my_custom_tool'}.ts`
    const targetDir =
      scope === 'workspace'
        ? join(workspace, '.ada', 'extensions')
        : join(getAppHome(), 'extensions')

    mkdirSync(targetDir, { recursive: true })
    const fullPath = join(targetDir, fileName)

    const toolIdentifier = cleanName.replace(/-/g, '_')
    const code = customCode?.trim() || `/**
 * 扩展工具: ${cleanName}
 *
 * 可以在这里编写自定义逻辑，Agent 会在需要时调用此工具。
 */
export default function (context: any) {
  context.registerTool({
    name: '${toolIdentifier}',
    description: '这是自定义扩展工具 ${cleanName} 的功能描述',
    parameters: {
      type: 'object',
      properties: {
        query: {
          type: 'string',
          description: '输入查询参数',
        },
      },
      required: ['query'],
    },
    async execute(callId: string, args: { query: string }) {
      // 可以在这里执行网络请求、文件操作或系统调用
      return {
        output: \`扩展 [${cleanName}] 成功处理输入: \${args.query}\`,
        ok: true,
      }
    },
  })
}
`
    writeFileSync(fullPath, code, 'utf8')
    await this.autoLoadExtensions(workspace)
    return fullPath
  }

  /**
   * 删除指定的插件文件并重新加载
   */
  async deletePlugin(filePath: string, workspace: string): Promise<boolean> {
    try {
      if (existsSync(filePath)) {
        const st = statSync(filePath)
        if (st.isDirectory()) {
          rmSync(filePath, { recursive: true, force: true })
        } else {
          unlinkSync(filePath)
        }
        await this.autoLoadExtensions(workspace)
        return true
      }
    } catch (err) {
      console.warn(`[ExtensionLoader] 删除插件失败:`, err)
    }
    return false
  }
}

export const defaultExtensionLoader = new ExtensionLoader()
