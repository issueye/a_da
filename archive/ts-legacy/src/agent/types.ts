/**
 * 界面层的数据形状：一个会话持有什么、每行卡片渲染什么。
 *
 * 发给模型的消息模型在 `core/types.ts`（AgentMessage）。会话原样保存那一份，
 * 所以 thread.messages 永远等于真正发出去过的历史，不再有第二套转换。
 */

import type { TokenUsage } from './ai/types'
import type { AgentMessage } from './core/types'

export type ToolStatus = 'awaiting' | 'running' | 'done' | 'error' | 'denied'

/**
 * 一次「智能体向用户提问」（`ask_user` 工具）的问题与答案。
 *
 * 它挂在**工具卡片的 `details.question`** 上，而不是新增一种 Item：问题本来就属于
 * 那次调用，挂在卡片上让问题、答案与工具结果一起留在会话历史里，也不必改
 * `Item` 联合与所有渲染分支。
 */
export interface AgentQuestion {
  question: string
  /** 可选的固定选项；给了就渲染成按钮 */
  choices?: Array<{ id: string; label: string; description?: string }>
  /** 是否允许自由输入。没给选项时强制为真（否则用户无从作答） */
  allowText?: boolean
  status: 'pending' | 'answered' | 'aborted'
  askedAt: number
  answer?: { answeredBy: 'user' | 'aborted'; choice?: string; text?: string }
}

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
      /** 整轮对话总耗时（毫秒，包含思考、工具执行与流式生成） */
      turnDurationMs?: number
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
      details?: Record<string, any>
      threadId?: string
      /** 执行前快照的检查点 id（仅 write_file / edit_file）；有它才能「撤销此次改动」 */
      checkpointId?: string
      /** 该检查点已被回滚 */
      reverted?: boolean
    }
  | { kind: 'notice'; id: string; at: number; text: string; level: 'info' | 'error' }  | {
      kind: 'compact'
      id: string
      at: number
      summary: string
      preTokens: number
      postTokens: number
      savedTokens: number
      turnsSummarized: number
      customInstructions?: string
      prunedItems?: Item[]
    }

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

export type AgentMode = 'code' | 'plan' | 'create'

export interface Thread {
  id: string
  title: string
  createdAt: number
  /** The project this conversation works in. Every tool call is scoped to it. */
  workspace: string
  items: Item[]
  /** Everything the model has been told in this thread, in the core message model. */
  messages: AgentMessage[]
  mode?: AgentMode
  parentId?: string
  subagentId?: string
  isSubagent?: boolean
  lastSystemPrompt?: string
  lastSystemPromptChars?: number
  lastToolSpecsChars?: number
  /**
   * 插件自己的会话级数据，按插件 id 分键。
   *
   * **核心永不读取它**（设计文档 §6.7.3）：一旦核心去解释它，插件数据就变成了隐式
   * 契约，插件作者再也没法自由改自己的结构。它随会话持久化、随会话删除。
   */
  pluginData?: Record<string, unknown>
}

export interface DebugEntry {
  id: number
  at: number
  kind: 'request' | 'response' | 'delta' | 'tools' | 'tool' | 'error' | 'info'
  text: string
  /** 结构化的完整请求或响应负载（如 messages 列表、回复对象等） */
  payload?: unknown
  /** 格式化后的完整 JSON 字符串，便于直接展示与复制 */
  raw?: string
  /** 涉及的大模型名称 */
  model?: string
  /** 接口调用耗时（毫秒） */
  durationMs?: number
}

export interface ToolOutcome {
  output: string
  ok: boolean
  patch?: string
}
