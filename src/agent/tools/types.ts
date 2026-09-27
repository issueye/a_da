/**
 * 工具系统类型定义
 * 参考 @earendil-works/pi-coding-agent 工具规范
 */

import type { AgentTool, AgentToolResult } from '../core/types'

export type { AgentTool, AgentToolResult }

export interface ToolContext {
  workspace: string
}

export interface ReadToolArgs {
  path: string
  offset?: number
  limit?: number
}

export interface WriteToolArgs {
  path: string
  content: string
}

export interface EditReplacement {
  old_string?: string
  new_string?: string
  oldText?: string
  newText?: string
}

export interface EditToolArgs {
  path: string
  old_string?: string
  new_string?: string
  edits?: EditReplacement[]
}

export interface BashToolArgs {
  command: string
  cwd?: string
  timeout?: number
}

export interface SearchToolArgs {
  pattern: string
  glob?: string
  /** 限定搜索的子目录（相对工作区） */
  path?: string
  /** 把 pattern 当纯文本而不是正则 */
  literal?: boolean
  /** 区分大小写（默认不区分） */
  case_sensitive?: boolean
  /** 每个匹配附带的上下文行数（0-3） */
  context?: number
}

export interface ListToolArgs {
  path?: string
  depth?: number
}

export interface SubagentToolArgs {
  subagent_id: string
  task: string
  additional_context?: string
  async?: boolean
}

