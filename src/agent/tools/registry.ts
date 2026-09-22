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
import { createSubagentTool, createCheckSubagentTool, createSendSubagentMessageTool } from './builtins/subagent'

export class ToolRegistry {
  private customTools = new Map<string, AgentTool>()

  /** 明确只读的内置工具。名字不在这里的一律按「会改动工作区」处理。 */
  private static readonly READ_ONLY = new Set(['list_files', 'read_file', 'search_files', 'todo', 'invoke_subagent', 'check_subagent', 'send_subagent_message'])

  /**
   * 注册自定义/扩展工具
   */
  register(tool: AgentTool): void {
    this.customTools.set(tool.name, tool)
  }

  /**
   * 取消注册指定工具
   */
  unregister(name: string): void {
    this.customTools.delete(name)
  }

  /**
   * 清空所有自定义/扩展工具
   */
  clearCustomTools(): void {
    this.customTools.clear()
  }

  /**
   * 获取当前所有已注册的自定义工具
   */
  getCustomTools(): AgentTool[] {
    return Array.from(this.customTools.values())
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
      createSubagentTool(workspace),
      createCheckSubagentTool(),
      createSendSubagentMessageTool(),
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

export interface BuiltinToolInfo {
  name: string
  label: string
  description: string
  isReadOnly: boolean
}

export const BUILTIN_TOOLS_METADATA: BuiltinToolInfo[] = [
  { name: 'list_files', label: '列出文件', description: '遍历并列出指定目录下的文件与子目录结构', isReadOnly: true },
  { name: 'read_file', label: '读取文件', description: '安全读取工作区内的代码或文本文件内容', isReadOnly: true },
  { name: 'search_files', label: '搜索文件', description: '在工作区文件中快速全局搜索指定文本或模式', isReadOnly: true },
  { name: 'todo', label: '任务清单', description: '管理多步骤编码任务的进度与状态', isReadOnly: true },
  { name: 'invoke_subagent', label: '委派子智能体', description: '委派专项任务给隔离运行的专用子智能体', isReadOnly: true },
  { name: 'check_subagent', label: '查询子智能体', description: '查询异步子智能体的运行状态与总结报告', isReadOnly: true },
  { name: 'send_subagent_message', label: '智能体通讯', description: '向子智能体发送消息以动态纠偏或唤醒续跑', isReadOnly: true },
  { name: 'write_file', label: '写入文件', description: '在工作区创建新文件或覆盖已有文件', isReadOnly: false },
  { name: 'edit_file', label: '编辑文件', description: '通过精准替换文本修改已有代码文件', isReadOnly: false },
  { name: 'run_command', label: '执行命令', description: '在项目工作区根目录下执行终端命令', isReadOnly: false },
]
