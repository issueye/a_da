/**
 * 核心 Agent 运行时类型定义
 * 参考 @earendil-works/pi-agent-core 架构设计
 */

import type { TokenUsage } from '../ai/types'
import type { ChatCompletionMessageParam } from '../ai/stream'

export type ToolExecutionMode = 'sequential' | 'parallel'
export type QueueMode = 'all' | 'one-at-a-time'

/** 一轮 runAgentLoop 为何结束：正常收尾、撞上步数上限、还是被中止。 */
export type AgentEndReason = 'completed' | 'max_steps' | 'aborted'

export interface ToolCallBlock {
  id: string
  name: string
  arguments: Record<string, unknown>
  rawArguments: string
}

export interface TextBlock {
  type: 'text'
  text: string
}

export interface ThinkingBlock {
  type: 'thinking'
  thinking: string
}

export interface UserMessage {
  role: 'user'
  content: string
  /** 附带的图片路径、URL 或 Base64 Data URL */
  images?: string[]
  timestamp?: number
}

export interface AssistantMessage {
  role: 'assistant'
  content: string
  thinking?: string
  toolCalls?: ToolCallBlock[]
  stopReason?: 'stop' | 'tool_use' | 'error' | 'aborted'
  errorMessage?: string
  timestamp?: number
  /** 本轮/本次对话消耗的 Token 统计 */
  usage?: TokenUsage
  /** 本次对话花费的耗时（毫秒） */
  durationMs?: number
}

export interface ToolResultMessage {
  role: 'toolResult'
  toolCallId: string
  toolName: string
  content: string
  isError?: boolean
  details?: unknown
  patch?: string
  timestamp?: number
}

export type AgentMessage = UserMessage | AssistantMessage | ToolResultMessage

export interface AgentToolResult<T = unknown> {
  output: string
  ok: boolean
  details?: T
  patch?: string
  terminate?: boolean
}

export type AgentToolUpdateCallback<T = unknown> = (partialResult: AgentToolResult<T>) => void

export interface AgentTool<TArgs = any, TDetails = any> {
  name: string
  label?: string
  description: string
  parameters: Record<string, unknown>
  executionMode?: ToolExecutionMode
  execute: (
    toolCallId: string,
    params: TArgs,
    signal?: AbortSignal,
    onUpdate?: AgentToolUpdateCallback<TDetails>
  ) => Promise<AgentToolResult<TDetails>>
}

export interface BeforeToolCallContext {
  assistantMessage: AssistantMessage
  toolCall: ToolCallBlock
  args: Record<string, unknown>
}

export interface BeforeToolCallResult {
  block?: boolean
  reason?: string
  terminate?: boolean
}

export interface AfterToolCallContext {
  assistantMessage: AssistantMessage
  toolCall: ToolCallBlock
  result: AgentToolResult
  isError: boolean
}

export interface AfterToolCallResult {
  output?: string
  isError?: boolean
  details?: unknown
  terminate?: boolean
}

export interface ShouldStopAfterTurnContext {
  message: AssistantMessage
  toolResults: ToolResultMessage[]
  messages: AgentMessage[]
}

export type AgentEvent =
  | { type: 'agent_start' }
  | { type: 'agent_end'; messages: AgentMessage[]; reason: AgentEndReason }
  | { type: 'turn_start' }
  | { type: 'turn_end'; message: AssistantMessage; toolResults: ToolResultMessage[] }
  | { type: 'message_start'; message: AgentMessage }
  | {
      type: 'message_update'
      message: AssistantMessage
      delta: { text?: string; thinking?: string; toolCall?: ToolCallBlock; usage?: TokenUsage }
    }
  | { type: 'message_end'; message: AgentMessage }
  | { type: 'tool_execution_start'; toolCallId: string; toolName: string; args: Record<string, unknown> }
  | { type: 'tool_execution_update'; toolCallId: string; partialResult: AgentToolResult }
  | { type: 'tool_execution_end'; toolCallId: string; result: AgentToolResult }
  | {
      type: 'llm_request'
      model: string
      baseUrl: string
      messages: ChatCompletionMessageParam[]
      tools?: unknown[]
    }
  | {
      type: 'llm_response'
      model: string
      message: AssistantMessage
    }

export interface AgentState {
  systemPrompt: string
  tools: AgentTool[]
  messages: AgentMessage[]
  readonly isStreaming: boolean
  readonly pendingToolCalls: ReadonlySet<string>
  readonly errorMessage?: string
}

export interface AgentLoopOptions {
  systemPrompt?: string
  tools?: AgentTool[]
  maxSteps?: number
  /** 传给接口的 reasoning_effort；空值则不下发该字段。 */
  effort?: string
  toolExecution?: ToolExecutionMode
  beforeToolCall?: (context: BeforeToolCallContext, signal?: AbortSignal) => Promise<BeforeToolCallResult | undefined>
  afterToolCall?: (context: AfterToolCallContext, signal?: AbortSignal) => Promise<AfterToolCallResult | undefined>
  shouldStopAfterTurn?: (context: ShouldStopAfterTurnContext, signal?: AbortSignal) => boolean | Promise<boolean>
  getSteeringMessages?: () => Promise<AgentMessage[]>
  getFollowUpMessages?: () => Promise<AgentMessage[]>
  signal?: AbortSignal
  workspace?: string
}
