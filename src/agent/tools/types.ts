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
}

export interface ListToolArgs {
  path?: string
  depth?: number
}
