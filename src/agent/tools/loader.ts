/**
 * 基于 jiti 的 TypeScript 扩展加载器
 * 参考 @earendil-works/pi-coding-agent/src/core/extensions/loader.ts
 * 允许用户在项目 .ada/extensions 或全局目录直接编写 .ts 扩展工具并热加载
 */

import { existsSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { createJiti } from 'jiti'
import type { AgentEvent, AgentTool } from '../core/types'
import { getAppHome } from '../home'
import { defaultToolRegistry } from './registry'

/**
 * 传递给扩展插件的完整上下文 API
 * 参考 @earendil-works/pi-coding-agent 的 ExtensionAPI
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

export type ExtensionFunction = (ctx: ExtensionContext) => void | Promise<void> | AgentTool | ((workspace: string) => AgentTool)

export interface ExtensionModule {
  tool?: AgentTool | ((workspace: string) => AgentTool)
  tools?: Array<AgentTool | ((workspace: string) => AgentTool)>
  default?: AgentTool | ((workspace: string) => AgentTool) | ExtensionFunction | Array<AgentTool | ((workspace: string) => AgentTool)>
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
   * 从指定目录动态扫描并加载 TypeScript/JavaScript 扩展模块
   */
  async loadExtensionsFromDir(dirPath: string, workspace: string): Promise<string[]> {
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
      const fullPath = join(dirPath, file)
      try {
        const mod = (await this.jitiInstance.import(fullPath)) as ExtensionModule

        // 1. 如果 default 导出是一个 ExtensionFunction (如 export default function(api) { api.registerTool(...) })
        if (typeof mod.default === 'function') {
          const fnResult = await (mod.default as any)(context)
          if (fnResult && typeof fnResult === 'object' && 'name' in fnResult) {
            defaultToolRegistry.register(fnResult as AgentTool)
            loadedNames.push((fnResult as AgentTool).name)
          }
        }

        // 2. 兼容对象或数组形式导出的 tools
        const candidates = [
          mod.tool,
          ...(mod.tools || []),
          ...(Array.isArray(mod.default) ? mod.default : []),
        ].filter(Boolean)

        for (const candidate of candidates) {
          let toolInstance: AgentTool | undefined
          if (typeof candidate === 'function') {
            toolInstance = (candidate as any)(workspace)
          } else if (candidate && typeof candidate === 'object' && 'name' in candidate) {
            toolInstance = candidate as AgentTool
          }

          if (toolInstance) {
            defaultToolRegistry.register(toolInstance)
            loadedNames.push(toolInstance.name)
          }
        }
      } catch (err) {
        console.warn(`[ExtensionLoader] 加载扩展失败 ${file}:`, (err as Error).message)
      }
    }

    return loadedNames
  }

  /**
   * 自动加载工作区及全局的扩展插件。
   *
   * 工作区那份是随仓库一起被克隆进来的第三方代码，所以只读模式下扩展工具
   * 一律要审批（见 ToolRegistry.isWriteTool）。
   */
  async autoLoadExtensions(workspace: string): Promise<string[]> {
    const projectExtDir = join(workspace, '.ada', 'extensions')
    const globalExtDir = join(getAppHome(), 'extensions')

    const projectLoaded = await this.loadExtensionsFromDir(projectExtDir, workspace)
    const globalLoaded = await this.loadExtensionsFromDir(globalExtDir, workspace)

    return [...projectLoaded, ...globalLoaded]
  }
}

export const defaultExtensionLoader = new ExtensionLoader()
