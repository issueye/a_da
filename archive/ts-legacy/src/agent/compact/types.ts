/**
 * 会话摘要与上下文压缩核心类型定义
 * 参考 E:\code\github\ZCode\apps\zcode-cli\packages\core\src\compact\
 */

import type { AgentMessage } from '../core/types'
import type { Item } from '../types'

export type CompactTrigger = 'manual' | 'auto'

export interface CompactPolicyConfig {
  /** 是否启用自动压缩，默认 true */
  enabled?: boolean
  /** 模型上下文窗口总 Token 上限 */
  contextWindow?: number
  /** 为单次正常回答输出预留的 Token 数（默认 20,000） */
  outputReserveTokens?: number
  /** 触发自动压缩的安全缓冲区大小（默认 15,000） */
  bufferTokens?: number
  /** 自动压缩触发百分比（默认 80%） */
  thresholdPercent?: number
}

export interface CompactSelection {
  /** 待送入模型进行结构化总结的前期消息 */
  messagesToSummarize: AgentMessage[]
  /** 原样保留在上下文中的最近一轮或多轮消息 */
  preservedMessages: AgentMessage[]
  /** 对应被折叠/收缩的前期界面卡片 */
  prunedItems: Item[]
  /** 原样保留在界面上的最近卡片 */
  preservedItems: Item[]
  /** 本次压缩涵盖的历史回合数 */
  turnsSummarized: number
}

export interface CompactResult {
  /** 9 大结构化总结内容（已剥离 <analysis> 思考标签） */
  summary: string
  /** 模型原始返回的完整文本（含 <analysis> 与 <summary>） */
  rawResponse: string
  /** 压缩前有效估算/实际 Token 数 */
  preTokens: number
  /** 压缩后上下文 Token 数 */
  postTokens: number
  /** 节约的 Token 数量 */
  savedTokens: number
  /** 压缩的回合数 */
  turnsSummarized: number
  /** 用户附加的自定义总结偏好要求 */
  customInstructions?: string
  /** 原样保留的消息 */
  preservedMessages: AgentMessage[]
  /** 原样保留的界面卡片 */
  preservedItems: Item[]
  /** 被收缩归档的界面卡片 */
  prunedItems: Item[]
}

export interface AutoCompactDecision {
  shouldCompact: boolean
  currentTokens: number
  threshold: number
  contextWindow: number
  effectiveContextWindow: number
  reason: 'disabled' | 'not_enough_messages' | 'below_threshold' | 'above_threshold'
}
