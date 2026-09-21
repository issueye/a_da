/**
 * 工具注册中心
 * 参考 @earendil-works/pi-coding-agent 统一工具管理
 */

import type { AgentTool } from '../core/types'
import { createBashTool } from './builtins/bash'
import { createEditTool } from './builtins/edit'
import { createListTool } from './builtins/list'
import { createReadTool } from './builtins/read'
import { createSearchTool } from './builtins/search'
import { createWriteTool } from './builtins/write'
import { createTodoTool } from './builtins/todo'

export class ToolRegistry {
  private customTools = new Map<string, AgentTool>()

  /** 明确只读的内置工具。名字不在这里的一律按「会改动工作区」处理。 */
  private static readonly READ_ONLY = new Set(['list_files', 'read_file', 'search_files', 'todo'])

  /**
   * 注册自定义/扩展工具
   */
  register(tool: AgentTool): void {
    this.customTools.set(tool.name, tool)
  }

  /**
   * 取消注册
   */
  unregister(name: string): void {
    this.customTools.delete(name)
  }

  /**
   * 创建适用于指定工作区的所有工具列表 (包含内置基础工具 + 已注册扩展工具)
   */
  getToolsForWorkspace(workspace: string): AgentTool[] {
    const builtins: AgentTool[] = [
      createListTool(workspace),
      createReadTool(workspace),
      createSearchTool(workspace),
      createWriteTool(workspace),
      createEditTool(workspace),
      createBashTool(workspace),
      createTodoTool(),
    ]

    const all = [...builtins]
    for (const custom of this.customTools.values()) {
      all.push(custom)
    }

    return all
  }

  /**
   * 是否为产生写副作用的工具。
   *
   * 失败安全：只有列在白名单里的只读工具算安全，其余（含扩展注册的工具）都要
   * 在「只读」模式下走审批。扩展是工作区里的第三方代码，不能默认它无害。
   */
  isWriteTool(name: string): boolean {
    return !ToolRegistry.READ_ONLY.has(name)
  }
}

export const defaultToolRegistry = new ToolRegistry()
