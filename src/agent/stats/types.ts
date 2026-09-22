/**
 * 统计与上下文分析核心类型定义
 * 参考 ZCode Context Breakdown 与 Telemetry 体系
 */

export type ContextSource =
  | 'messages'
  | 'system_prompt'
  | 'skills'
  | 'tools'
  | 'completion'

export interface ContextBreakdownItem {
  source: ContextSource
  label: string
  color: string
  chars: number
  estimatedTokens: number
  percent: number
}

export interface ContextUsageSummary {
  /** 当前上下文占用的 Token 数（优先基于真实返回的 promptTokens，或估算） */
  usedTokens: number
  /** 当前模型支持的最大上下文窗口大小（如 128,000） */
  maxTokens: number
  /** 上下文使用率（0 ~ 1） */
  percent: number
  /** 格式化后的简短概括（如 "2.7k/128k (2.1%)"） */
  formattedSummary: string
  /** 缓存命中率（0 ~ 1，未命中或无缓存时为 null） */
  cacheHitRate: number | null
  /** 缓存命中的 Token 数 */
  cachedTokens: number
  /** 上下文各组成部分分解列表（按占比从大到小排序） */
  breakdown: ContextBreakdownItem[]
}
