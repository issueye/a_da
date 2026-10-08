/**
 * 前端核心类型定义 —— re-export 自 crates/agent-proto/client-ts，消除多份手写 DTO 漂移（M3-T3）
 */

export * from '../../../crates/agent-proto/client-ts'

// ── 以下为纯前端视图展示辅助类型（不在后端线协议中传输） ──

export interface TodoStep {
  id?: string
  title: string
  status: 'pending' | 'in_progress' | 'completed'
}

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

export type ToastLevel = 'info' | 'success' | 'warn' | 'error'

export interface ToastItem {
  id: string
  level: ToastLevel
  message: string
  detail?: string
  durationMs?: number
  action?: {
    label: string
    onClick: () => void
  }
}
