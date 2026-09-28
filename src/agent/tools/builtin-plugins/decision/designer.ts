/**
 * 决策设计器：自由提示词 → 决策 schema。
 *
 * 对齐 pi-jev 的 `/jev test <prompt>`：让会话模型自己设计「该问什么问题」，
 * 再把设计结果交给引擎评估。
 *
 * 这里的重点是**严格校验模型输出**——它是不受信任的。pi-jev 的 validateDesign
 * 写得相当扎实，本文件基本照搬其规则（choice 的 criteria 必须是非空对象、
 * score 必须是非空数组、noul 不得带 criteria）。校验不过宁可整体失败，
 * 也不放一个半残的 schema 进去。
 */

import { readLlmConfig, type ProviderConfig } from '../../../config'
import { streamModelChat, type ChatCompletionMessageParam } from '../../../ai/stream'
import type { DecisionQuestion, DecisionRequest } from './types'

/** 单次设计最多产出几个问题。 */
export const MAX_DESIGNED_QUESTIONS = 6

export const DESIGN_SYSTEM_PROMPT = [
  'You design System One evaluations for a decision engine.',
  "Given a user's request, reply with ONLY a JSON object (no prose, no code fence):",
  '{"state": <string or object holding the material to judge>, "questions": {"<snake_case_id>": {"type": "noul"|"choice"|"score", "instructions": "<the judgment>", "criteria": <type-specific>}}}',
  'Rules:',
  `- Use 1 to ${MAX_DESIGNED_QUESTIONS} questions, each independent and answerable from "state" alone.`,
  '- "noul" is a yes/no probability question and must NOT include "criteria".',
  '- "choice" requires "criteria" as an object mapping option keys to descriptions.',
  '- "score" requires "criteria" as an array of rubric levels, highest first.',
  '- "state" must contain concrete, self-contained content: never reference external context.',
].join('\n')

/**
 * 从可能带 ``` 围栏或前后散文的模型输出里抽出第一个 JSON 对象。
 * 模型很少严格只回 JSON，所以这一步不能省。
 */
export function extractJson(text: string): unknown {
  const cleaned = text.replace(/^\s*```(?:json)?/i, '').replace(/```\s*$/, '')
  const start = cleaned.indexOf('{')
  const end = cleaned.lastIndexOf('}')
  if (start === -1 || end <= start) return null
  try {
    return JSON.parse(cleaned.slice(start, end + 1))
  } catch {
    return null
  }
}

/**
 * 校验不可信的设计输出，产出可用的决策请求。
 *
 * 逐条丢弃不合法的问题；一个都没剩下则整体失败（返回 null），
 * 由调用方给用户一个能理解的错误，而不是拿空 schema 去评估。
 */
export function validateDesign(raw: unknown, maxQuestions = MAX_DESIGNED_QUESTIONS): DecisionRequest | null {
  if (!raw || typeof raw !== 'object') return null
  const candidate = raw as { state?: unknown; questions?: unknown }

  if (candidate.state === undefined || candidate.state === null) return null
  const state =
    typeof candidate.state === 'string' || typeof candidate.state === 'object'
      ? (candidate.state as string | Record<string, unknown>)
      : null
  if (state === null) return null
  if (!candidate.questions || typeof candidate.questions !== 'object') return null

  const questions: Record<string, DecisionQuestion> = {}
  for (const [id, value] of Object.entries(candidate.questions as Record<string, unknown>)) {
    if (Object.keys(questions).length >= maxQuestions) break
    if (!value || typeof value !== 'object') continue
    const q = value as { type?: unknown; instructions?: unknown; criteria?: unknown }
    if (typeof q.instructions !== 'string' || !q.instructions.trim()) continue

    if (q.type === 'noul') {
      // noul 是纯是/否问题，带 criteria 说明模型理解错了，丢弃
      if (q.criteria !== undefined) continue
      questions[id] = { type: 'noul', instructions: q.instructions }
      continue
    }

    if (q.type === 'choice') {
      // 必须是「非空对象」；数组是 score 的形态，给错就丢
      if (!q.criteria || typeof q.criteria !== 'object' || Array.isArray(q.criteria)) continue
      const entries = Object.entries(q.criteria as Record<string, unknown>)
      if (entries.length === 0) continue
      const criteria: Record<string, string | null> = {}
      for (const [key, desc] of entries) {
        criteria[key] = typeof desc === 'string' ? desc : null
      }
      questions[id] = { type: 'choice', instructions: q.instructions, criteria }
      continue
    }

    if (q.type === 'score') {
      // 必须是非空字符串数组
      if (!Array.isArray(q.criteria) || q.criteria.length === 0) continue
      const levels = q.criteria.filter((level): level is string => typeof level === 'string' && level.trim().length > 0)
      if (levels.length === 0) continue
      questions[id] = { type: 'score', instructions: q.instructions, criteria: levels }
      continue
    }
    // 未知类型：丢弃
  }

  if (Object.keys(questions).length === 0) return null
  return { state, questions }
}

/** 让模型为一段自由提示词设计决策 schema。抽成函数便于测试注入。 */
export type DesignRunner = (
  config: ProviderConfig,
  prompt: string,
  signal?: AbortSignal,
) => Promise<string | null>

const defaultDesignRunner: DesignRunner = async (config, prompt, signal) => {
  const messages: ChatCompletionMessageParam[] = [{ role: 'user', content: prompt }]
  let text = ''
  for await (const chunk of streamModelChat(config, messages, {
    systemPrompt: DESIGN_SYSTEM_PROMPT,
    // 设计阶段不要工具，只要一段 JSON
    tools: undefined,
    temperature: 0,
    signal,
  })) {
    if (chunk.type === 'text' && chunk.text) text += chunk.text
    if (chunk.type === 'error') return null
  }
  return text.trim() || null
}

export interface DesignOutcome {
  /** 校验通过的设计；失败为 null */
  design: DecisionRequest | null
  /** 失败原因（面向用户） */
  error?: string
  /** 原始模型输出，便于排查 */
  raw?: string
}

/**
 * 设计一次决策。
 *
 * 两步分离（设计 → 校验）是刻意的：模型输出不可信，校验必须独立成一步，
 * 这样测试可以直接喂各种畸形 JSON 验证防御逻辑，不必真的调模型。
 */
export async function designDecision(
  prompt: string,
  options: {
    maxQuestions?: number
    signal?: AbortSignal
    runner?: DesignRunner
    config?: ProviderConfig
  } = {},
): Promise<DesignOutcome> {
  const config = options.config ?? (await readLlmConfig())
  if (!config) {
    return { design: null, error: '未配置模型，无法设计决策。请先在设置里填写模型供应商。' }
  }

  const runner = options.runner ?? defaultDesignRunner
  const raw = await runner(
    {
      baseUrl: config.baseUrl,
      apiKey: config.apiKey,
      model: config.model,
      contextWindow: config.contextWindow,
      supportsImages: config.supportsImages,
    },
    prompt,
    options.signal,
  )

  if (!raw) {
    return { design: null, error: '模型未返回任何内容，无法设计决策。' }
  }

  const design = validateDesign(extractJson(raw), options.maxQuestions ?? MAX_DESIGNED_QUESTIONS)
  if (!design) {
    return {
      design: null,
      error: '模型未返回可用的决策 schema（问题定义不合法或为空）。请换个说法再试。',
      raw,
    }
  }
  return { design, raw }
}
