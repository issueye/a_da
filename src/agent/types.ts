/**
 * 界面层的数据形状：一个会话持有什么、每行卡片渲染什么。
 *
 * 发给模型的消息模型在 `core/types.ts`（AgentMessage）。会话原样保存那一份，
 * 所以 thread.messages 永远等于真正发出去过的历史，不再有第二套转换。
 */

import type { TokenUsage } from './ai/types'
import type { AgentMessage } from './core/types'

export type ToolStatus = 'awaiting' | 'running' | 'done' | 'error' | 'denied'

export type Item =
  | { kind: 'user'; id: string; at: number; text: string; images?: string[]; queued?: boolean }
  | {
      /** 模型的思考链（reasoning_content）。默认折叠，只留一行时长。 */
      kind: 'thinking'
      id: string
      /** 第一次收到思考增量的时刻，用来算「持续了几秒」。 */
      at: number
      text: string
      /** 这段思考结束的时刻：思考完就不再增长。 */
      endedAt?: number
    }
  | {
      kind: 'assistant'
      id: string
      at: number
      text: string
      streaming?: boolean
      /** 本次对话消耗的 Token 统计 */
      usage?: TokenUsage
      /** 本次对话花费的时间（毫秒） */
      durationMs?: number
    }
  | {
      kind: 'tool'
      id: string
      at: number
      callId: string
      name: string
      args: Record<string, unknown>
      rawArgs: string
      status: ToolStatus
      output?: string
      patch?: string
    }
  | { kind: 'notice'; id: string; at: number; text: string; level: 'info' | 'error' }

export interface ThreadStats {
  totalPromptTokens: number
  totalCompletionTokens: number
  totalTokens: number
  totalThinkingTokens: number
  totalCachedTokens: number
  totalDurationMs: number
  turnsCount: number
}

/** 汇总计算指定会话的所有已完成轮次的 Token 与耗时 */
export function computeThreadStats(thread: Thread, _model?: string): ThreadStats {
  let totalPromptTokens = 0
  let totalCompletionTokens = 0
  let totalTokens = 0
  let totalThinkingTokens = 0
  let totalCachedTokens = 0
  let totalDurationMs = 0
  let turnsCount = 0

  for (const item of thread.items) {
    if (item.kind === 'assistant' && !item.streaming) {
      turnsCount++
      if (item.durationMs) {
        totalDurationMs += item.durationMs
      }
      if (item.usage && (item.usage.totalTokens > 0 || item.usage.promptTokens > 0 || item.usage.completionTokens > 0)) {
        totalPromptTokens += item.usage.promptTokens || 0
        totalCompletionTokens += item.usage.completionTokens || 0
        totalTokens += item.usage.totalTokens || (item.usage.promptTokens + item.usage.completionTokens)
        if (item.usage.thinkingTokens) {
          totalThinkingTokens += item.usage.thinkingTokens
        }
        if (item.usage.cachedTokens) {
          totalCachedTokens += item.usage.cachedTokens
        }
      }
    }
  }

  return {
    totalPromptTokens,
    totalCompletionTokens,
    totalTokens,
    totalThinkingTokens,
    totalCachedTokens,
    totalDurationMs,
    turnsCount,
  }
}

export interface Thread {
  id: string
  title: string
  createdAt: number
  /** The project this conversation works in. Every tool call is scoped to it. */
  workspace: string
  items: Item[]
  /** Everything the model has been told in this thread, in the core message model. */
  messages: AgentMessage[]
  parentId?: string
  subagentId?: string
  isSubagent?: boolean
}

export interface DebugEntry {
  id: number
  at: number
  kind: 'request' | 'delta' | 'tools' | 'tool' | 'error' | 'info'
  text: string
}

export interface ToolOutcome {
  output: string
  ok: boolean
  patch?: string
}
