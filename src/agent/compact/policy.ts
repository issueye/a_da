/**
 * 上下文压缩触发策略与阈值判定
 * 参考 ZCode 的 policy.ts 设计
 */

import type { AgentMessage } from '../core/types'
import type { AutoCompactDecision, CompactPolicyConfig } from './types'

export const DEFAULT_COMPACT_CONTEXT_WINDOW = 128_000
export const DEFAULT_OUTPUT_RESERVE_TOKENS = 20_000
export const DEFAULT_BUFFER_TOKENS = 15_000
export const DEFAULT_THRESHOLD_PERCENT = 80

/**
 * 获取指定模型的上下文窗口 Token 上限（优先使用用户配置的上限，其次使用预设，默认 128k）
 */
export function getModelContextWindow(modelName?: string, configuredLimit?: number): number {
  if (configuredLimit && configuredLimit > 0) {
    return configuredLimit
  }
  if (!modelName) return 128_000
  const m = modelName.toLowerCase()
  if (m.includes('gemini') || m.includes('qwen-long')) return 1_000_000
  if (m.includes('claude')) return 200_000
  if (m.includes('32k')) return 32_000
  if (m.includes('16k')) return 16_000
  if (m.includes('8k')) return 8_000
  if (m.includes('64k')) return 64_000
  if (m.includes('200k')) return 200_000
  if (m.includes('128k')) return 128_000
  return 128_000
}

/**
 * 粗略估算消息序列的 Token 体积（当无 API 返回的精确 usage 时兜底使用）
 * 依据中英文混合场景平均 3.5 字符 ≈ 1 Token
 */
export function estimateMessageTokens(messages: readonly AgentMessage[]): number {
  let charCount = 0

  for (const m of messages) {
    if (typeof m.content === 'string') {
      charCount += m.content.length
    }
    if (m.role === 'assistant') {
      if (m.thinking) {
        charCount += m.thinking.length
      }
      if (m.toolCalls && m.toolCalls.length > 0) {
        for (const tc of m.toolCalls) {
          charCount += tc.name.length + (tc.rawArguments ? tc.rawArguments.length : 20)
        }
      }
    } else if (m.role === 'toolResult') {
      charCount += m.toolName.length + (m.content ? m.content.length : 0)
    }
  }

  return Math.ceil(charCount / 3.5)
}

/**
 * 计算扣减单次回答预留窗口后的有效上下文预算
 */
export function getEffectiveContextWindowSize(config: CompactPolicyConfig = {}): number {
  const contextWindow = config.contextWindow && config.contextWindow > 0
    ? config.contextWindow
    : DEFAULT_COMPACT_CONTEXT_WINDOW
  const reserve = config.outputReserveTokens !== undefined
    ? config.outputReserveTokens
    : DEFAULT_OUTPUT_RESERVE_TOKENS
  return Math.max(0, contextWindow - reserve)
}

/**
 * 计算触发自动压缩的 Token 阈值
 */
export function getAutoCompactThreshold(config: CompactPolicyConfig = {}): number {
  const contextWindow = config.contextWindow && config.contextWindow > 0
    ? config.contextWindow
    : DEFAULT_COMPACT_CONTEXT_WINDOW
  const effective = getEffectiveContextWindowSize(config)
  const buffer = config.bufferTokens !== undefined ? config.bufferTokens : DEFAULT_BUFFER_TOKENS
  const percent = config.thresholdPercent !== undefined ? config.thresholdPercent : DEFAULT_THRESHOLD_PERCENT

  const bufferThreshold = Math.max(0, effective - buffer)
  const percentThreshold = Math.round(contextWindow * (percent / 100))

  return Math.min(bufferThreshold, percentThreshold)
}

/**
 * 判定当前会话是否需要自动触发上下文压缩
 */
export function shouldAutoCompact(input: {
  messages: readonly AgentMessage[]
  currentTokens?: number
  config?: CompactPolicyConfig
}): AutoCompactDecision {
  const config = input.config ?? {}
  if (config.enabled === false) {
    return {
      shouldCompact: false,
      currentTokens: input.currentTokens ?? 0,
      threshold: 0,
      contextWindow: 0,
      effectiveContextWindow: 0,
      reason: 'disabled',
    }
  }

  const userMessages = input.messages.filter((m) => m.role === 'user')
  const assistantMessages = input.messages.filter((m) => m.role === 'assistant')

  // 若历史对话过短（少于 2 轮完整用户交互或少于 4 条总消息），无压缩必要与空间
  if (userMessages.length < 2 || assistantMessages.length < 1 || input.messages.length < 4) {
    return {
      shouldCompact: false,
      currentTokens: input.currentTokens ?? 0,
      threshold: 0,
      contextWindow: config.contextWindow ?? DEFAULT_COMPACT_CONTEXT_WINDOW,
      effectiveContextWindow: getEffectiveContextWindowSize(config),
      reason: 'not_enough_messages',
    }
  }

  const contextWindow = config.contextWindow && config.contextWindow > 0
    ? config.contextWindow
    : DEFAULT_COMPACT_CONTEXT_WINDOW
  const effectiveContextWindow = getEffectiveContextWindowSize(config)
  const threshold = getAutoCompactThreshold(config)

  const currentTokens = input.currentTokens !== undefined && input.currentTokens > 0
    ? input.currentTokens
    : estimateMessageTokens(input.messages)

  const shouldCompact = currentTokens >= threshold

  return {
    shouldCompact,
    currentTokens,
    threshold,
    contextWindow,
    effectiveContextWindow,
    reason: shouldCompact ? 'above_threshold' : 'below_threshold',
  }
}
