/**
 * 基于 jiti 的 TypeScript 扩展加载器
 * 统一管理项目工作区 (.ada/extensions) 与全局用户目录 (~/.a-da/extensions) 的扩展插件，
 * 支持动态加载、插件扫描、启用/停用、新建模板以及删除插件。
 */

import { existsSync, mkdirSync, readdirSync, statSync, unlinkSync, writeFileSync } from 'node:fs'
import { basename, join } from 'node:path'
import { createJiti } from 'jiti'
import { readDisabledPlugins, saveDisabledPlugins } from '../config'
import type { AgentEvent, AgentTool } from '../core/types'
import { getAppHome } from '../home'
import { defaultToolRegistry } from './registry'

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
  scope: 'workspace' | 'global'
  enabled: boolean
  tools: PluginToolInfo[]
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
   * 扫描工作区与全局目录的所有扩展插件元数据
   */
  async scanPlugins(workspace: string): Promise<PluginItem[]> {
    const disabledList = new Set(await readDisabledPlugins())
    const projectExtDir = join(workspace, '.ada', 'extensions')
    const globalExtDir = join(getAppHome(), 'extensions')

    const items: PluginItem[] = []

    const scanDir = async (dirPath: string, scope: 'workspace' | 'global') => {
      if (!existsSync(dirPath)) return
      let files: string[] = []
      try {
        files = readdirSync(dirPath)
      } catch {
        return
      }

      for (const file of files) {
        if (!file.endsWith('.ts') && !file.endsWith('.js')) continue
        const fullPath = join(dirPath, file)
        const id = `${scope}:${file}`
        const name = basename(file).replace(/\.[^.]+$/, '')
        let sizeBytes = 0
        let updatedAt = Date.now()

        try {
          const st = statSync(fullPath)
          sizeBytes = st.size
          updatedAt = st.mtimeMs
        } catch {}

        const enabled = !disabledList.has(id)
        const item: PluginItem = {
          id,
          name,
          fileName: file,
          filePath: fullPath,
          scope,
          enabled,
          tools: [],
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
      }
    }

    await scanDir(projectExtDir, 'workspace')
    await scanDir(globalExtDir, 'global')

    return items
  }

  /**
   * 从指定目录动态扫描并加载 TypeScript/JavaScript 扩展模块并注册工具
   */
  async loadExtensionsFromDir(
    dirPath: string,
    workspace: string,
    disabledSet: Set<string> = new Set(),
    scope: 'workspace' | 'global' = 'workspace'
  ): Promise<string[]> {
    if (!existsSync(dirPath)) return []

    const loadedNames: string[] = []
    let files: string[] = []
    try {
      files = readdirSync(dirPath)
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

    for (const file of files) {
      if (!file.endsWith('.ts') && !file.endsWith('.js')) continue
      const id = `${scope}:${file}`
      if (disabledSet.has(id)) continue

      const fullPath = join(dirPath, file)
      try {
        const mod = (await this.jitiInstance.import(fullPath)) as ExtensionModule
        const extracted = await this.extractToolsFromModule(mod, context)
        for (const t of extracted) {
          defaultToolRegistry.register(t)
          loadedNames.push(t.name)
        }
      } catch (err) {
        console.warn(`[ExtensionLoader] 加载扩展失败 ${file}:`, (err as Error).message)
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

    return [...projectLoaded, ...globalLoaded]
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
        unlinkSync(filePath)
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
