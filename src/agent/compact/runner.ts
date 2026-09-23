/**
 * 会话压缩执行引擎
 * 负责切分待总结历史与保留轮次，发起纯文本模型推理，生成 9 大结构化总结并重组上下文
 */

import type { ProviderConfig } from '../config'
import type { AgentMessage } from '../core/types'
import { convertMessagesToLlm } from '../core/agent-loop'
import { streamModelChat } from '../ai/stream'
import type { Item, Thread } from '../types'
import { buildCompactPrompt, buildCompactSummaryMessage, formatCompactSummary } from './prompt'
import { estimateMessageTokens } from './policy'
import type { CompactResult, CompactSelection } from './types'

/**
 * 将当前会话切分为「前期待压缩历史」与「近期原样保留回合」
 * 默认保留最近 1 个完整交互回合（从最近一个 User 消息开始及其后续所有 Assistant/Tool），
 * 确保模型对刚刚讨论的话题拥有无损的短期记忆。
 */
export function selectCompactSelection(messages: readonly AgentMessage[], items: readonly Item[]): CompactSelection {
  // 查找最后一个用户消息在 messages 数组中的索引
  let lastUserMsgIdx = -1
  let userCount = 0
  for (let i = 0; i < messages.length; i++) {
    if (messages[i]?.role === 'user') {
      userCount++
      lastUserMsgIdx = i
    }
  }

  // 历史过短无法切分
  if (userCount < 2 || lastUserMsgIdx <= 0) {
    return {
      messagesToSummarize: [],
      preservedMessages: [...messages],
      prunedItems: [],
      preservedItems: [...items],
      turnsSummarized: 0,
    }
  }

  const messagesToSummarize = messages.slice(0, lastUserMsgIdx)
  const preservedMessages = messages.slice(lastUserMsgIdx)

  // 查找对应保留回合在 items 界面卡片中的起始位置
  // 找到最后一个 user item
  let lastUserItemIdx = -1
  let userItemCount = 0
  for (let i = 0; i < items.length; i++) {
    if (items[i]?.kind === 'user') {
      userItemCount++
      lastUserItemIdx = i
    }
  }

  const pruneCutIdx = lastUserItemIdx > 0 ? lastUserItemIdx : 0
  const prunedItems = items.slice(0, pruneCutIdx)
  const preservedItems = items.slice(pruneCutIdx)

  return {
    messagesToSummarize: [...messagesToSummarize],
    preservedMessages: [...preservedMessages],
    prunedItems: [...prunedItems],
    preservedItems: [...preservedItems],
    turnsSummarized: userCount - 1,
  }
}

export interface ExecuteCompactionOptions {
  customInstructions?: string
  systemPrompt?: string
  signal?: AbortSignal
  onProgress?: (deltaText: string) => void
}

/**
 * 执行一次会话上下文压缩
 */
export async function executeCompaction(
  thread: Thread,
  config: ProviderConfig,
  options: ExecuteCompactionOptions = {},
): Promise<CompactResult> {
  const selection = selectCompactSelection(thread.messages, thread.items)

  if (selection.messagesToSummarize.length === 0) {
    throw new Error('会话历史过短，需要至少 2 轮对话才能执行上下文压缩。')
  }

  // 压缩前有效 Token 估算或取最后一次已知使用量
  const preTokens = estimateMessageTokens(thread.messages)

  const compactPrompt = buildCompactPrompt(options.customInstructions)

  // 构建用于生成摘要的消息链：前置历史 + 纯文本提示词
  const requestMessages: AgentMessage[] = [
    ...selection.messagesToSummarize,
    {
      role: 'user',
      content: compactPrompt,
      timestamp: Date.now(),
    },
  ]

  const llmMessages = convertMessagesToLlm(options.systemPrompt ?? '', requestMessages, {
    supportsImages: config.supportsImages,
    workspace: thread.workspace,
  })

  let rawResponse = ''

  // 严格无工具调用（tools: undefined）
  for await (const chunk of streamModelChat(config, llmMessages, {
    tools: undefined,
    effort: 'high',
    signal: options.signal,
  })) {
    if (chunk.type === 'text' && chunk.text) {
      rawResponse += chunk.text
      options.onProgress?.(chunk.text)
    }
  }

  const cleanSummary = formatCompactSummary(rawResponse)
  const finalSummary = cleanSummary.trim() ? cleanSummary : rawResponse.trim()

  if (!finalSummary) {
    throw new Error('模型未返回有效的结构化摘要，压缩中止。')
  }

  // 预估压缩后的消息体积：Continuation Message + Preserved Messages
  const continuationMsgContent = buildCompactSummaryMessage(finalSummary, {
    recentMessagesPreserved: selection.preservedMessages.length > 0,
  })
  const postMessages: AgentMessage[] = [
    { role: 'user', content: continuationMsgContent, timestamp: Date.now() },
    ...selection.preservedMessages,
  ]
  const postTokens = estimateMessageTokens(postMessages)
  const savedTokens = Math.max(0, preTokens - postTokens)

  return {
    summary: finalSummary,
    rawResponse,
    preTokens,
    postTokens,
    savedTokens,
    turnsSummarized: selection.turnsSummarized,
    customInstructions: options.customInstructions,
    preservedMessages: selection.preservedMessages,
    preservedItems: selection.preservedItems,
    prunedItems: selection.prunedItems,
  }
}
