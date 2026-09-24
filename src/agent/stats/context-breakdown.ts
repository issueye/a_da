/**
 * 上下文分解与统计分析引擎
 * 参考 ZCode `buildContextUsageBreakdownSegments` 与 `ChatContextUsage`
 */

import type { Item } from '../types'
import type { ContextBreakdownItem, ContextSource, ContextUsageSummary } from './types'

export const CONTEXT_SOURCE_CONFIG: Record<
  ContextSource,
  { label: string; color: string; order: number }
> = {
  messages: { label: '正常对话消息', color: '#3b82f6', order: 0 },
  system_prompt: { label: '系统提示词', color: '#8b5cf6', order: 1 },
  skills: { label: '技能规范', color: '#10b981', order: 2 },
  tools: { label: '工具', color: '#f59e0b', order: 3 },
  completion: { label: '本次回复', color: '#ec4899', order: 4 },
}

export interface BreakdownInput {
  items: Item[]
  systemPrompt?: string
  skillsPrompt?: string
  systemChars?: number
  toolSpecsChars?: number
  toolsChars?: number
  realPromptTokens?: number
  realCompletionTokens?: number
  realCachedTokens?: number
  contextLimit: number
}

function formatTokensNumber(value: number): string {
  if (value >= 1_000_000) {
    const m = value / 1_000_000
    return `${m % 1 === 0 ? m.toFixed(0) : m.toFixed(1)}m`
  }
  if (value >= 1_000) {
    const k = value / 1_000
    return `${k % 1 === 0 ? k.toFixed(0) : k.toFixed(1)}k`
  }
  return `${value}`
}

/**
 * 计算上下文构成细分与统计
 */
export function computeContextBreakdown(input: BreakdownInput): ContextUsageSummary {
  const {
    items,
    systemPrompt = '',
    skillsPrompt = '',
    toolSpecsChars = 0,
    realPromptTokens,
    realCompletionTokens,
    realCachedTokens = 0,
    contextLimit,
  } = input

  // 1. 分别统计正常对话消息、工具执行、本次回复的字符数
  let messagesChars = 0
  let completionChars = 0
  let toolExecutionChars = 0

  for (let i = 0; i < items.length; i++) {
    const it = items[i]
    if (it.kind === 'user') {
      const imgOverhead = (it.images?.length ?? 0) * 1000
      messagesChars += (it.text?.length ?? 0) + imgOverhead + 20
    } else if (it.kind === 'tool') {
      // 工具执行项：调用参数、输出结果、修改补丁均归属工具
      toolExecutionChars +=
        (it.output?.length ?? 0) + (it.patch?.length ?? 0) + (it.rawArgs?.length ?? 0) + 30
    } else if (it.kind === 'thinking') {
      messagesChars += (it.text?.length ?? 0) + 15
    } else if (it.kind === 'assistant') {
      if (i === items.length - 1 && (it.streaming || realCompletionTokens !== undefined)) {
        completionChars += it.text?.length ?? 0
      } else {
        messagesChars += (it.text?.length ?? 0) + 20
      }
    }
  }

  // 工具字符总计：工具定义结构体字符数 + 历史工具调用执行字符数
  const toolsChars =
    input.toolsChars !== undefined ? input.toolsChars : toolSpecsChars + toolExecutionChars

  // 系统提示词与技能规范字符数
  const systemChars =
    input.systemChars !== undefined ? input.systemChars : systemPrompt.length
  const skillsChars = skillsPrompt.length

  // 总请求输入字符数 (Prompt Chars)
  const inputChars = messagesChars + systemChars + skillsChars + toolsChars
  const totalChars = Math.max(1, inputChars + completionChars)

  // 2. 确定真实的 Token 用量
  const promptTokens =
    realPromptTokens && realPromptTokens > 0
      ? realPromptTokens
      : Math.max(1, Math.round(inputChars / 2.2))

  const completionTokens =
    realCompletionTokens && realCompletionTokens > 0
      ? realCompletionTokens
      : Math.max(0, Math.round(completionChars / 2.2))

  const usedTokens = promptTokens + completionTokens
  const maxTokens = Math.max(1, contextLimit)
  const percent = Math.min(1, Math.max(0, usedTokens / maxTokens))

  // 3. 将 Token 按照各组成部分字符占比精准切分
  const candidateParts: Array<{ source: ContextSource; chars: number }> = [
    { source: 'messages', chars: messagesChars },
    { source: 'system_prompt', chars: systemChars },
    { source: 'skills', chars: skillsChars },
    { source: 'tools', chars: toolsChars },
  ]
  const promptParts: Array<{ source: ContextSource; chars: number }> = candidateParts.filter((p) => p.chars > 0)

  if (promptParts.length === 0 && promptTokens > 0) {
    promptParts.push({ source: 'messages', chars: 1 })
  }

  const breakdown: ContextBreakdownItem[] = []

  // 若存在输入部分，精准拆解 promptTokens
  if (promptParts.length > 0) {
    const totalPromptChars = promptParts.reduce((sum, p) => sum + p.chars, 0)
    let allocatedTokens = 0

    for (let idx = 0; idx < promptParts.length; idx++) {
      const part = promptParts[idx]!
      let partTokens: number
      // 最后一项通过减法保证 promptTokens 总和精确一致，不产生舍入误差
      if (idx === promptParts.length - 1) {
        partTokens = Math.max(0, promptTokens - allocatedTokens)
      } else {
        partTokens = Math.max(1, Math.round(promptTokens * (part.chars / totalPromptChars)))
        allocatedTokens += partTokens
      }

      const config = CONTEXT_SOURCE_CONFIG[part.source]
      breakdown.push({
        source: part.source,
        label: config.label,
        color: config.color,
        chars: part.chars,
        estimatedTokens: partTokens,
        percent: usedTokens > 0 ? partTokens / usedTokens : part.chars / totalChars,
      })
    }
  }

  // 本次回复补全项（若有）
  if (completionTokens > 0 || completionChars > 0) {
    const config = CONTEXT_SOURCE_CONFIG.completion
    breakdown.push({
      source: 'completion',
      label: config.label,
      color: config.color,
      chars: completionChars,
      estimatedTokens: completionTokens,
      percent: usedTokens > 0 ? completionTokens / usedTokens : completionChars / totalChars,
    })
  }

  // 按照字符数/Token 数量从大到小排序
  breakdown.sort(
    (a, b) =>
      b.chars - a.chars ||
      CONTEXT_SOURCE_CONFIG[a.source].order - CONTEXT_SOURCE_CONFIG[b.source].order,
  )

  // 4. 缓存命中率计算
  const cacheHitRate =
    realCachedTokens > 0 && promptTokens > 0
      ? Math.min(1, Math.max(0, realCachedTokens / promptTokens))
      : null

  // 5. 格式化概括文本（对齐 ZCode formatContextUsageSummary）
  const formattedPercent = (percent * 100).toFixed(percent < 0.01 ? 1 : 0)
  const formattedSummary = `${formatTokensNumber(usedTokens)}/${formatTokensNumber(maxTokens)} (${formattedPercent}%)`

  return {
    usedTokens,
    maxTokens,
    percent,
    formattedSummary,
    cacheHitRate,
    cachedTokens: realCachedTokens,
    breakdown,
  }
}
