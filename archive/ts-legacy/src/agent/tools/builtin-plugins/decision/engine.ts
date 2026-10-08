/**
 * 决策引擎：把「类型化问题」翻译成「结构化答案」。
 *
 * 三个实现，按 jev → local → heuristic 依次回退：
 *
 * - `JevEngine`：调 Jev 兼容端点（`/systemOne`）。专用 System One 模型，概率是校准的。
 * - `LocalEngine`：用 a_da 已配置的模型自评。**聊天模型自报的概率系统性偏乐观**
 *   （动辄给 0.9），所以这里不采信单次自报值，而是多样本采样 + 投票占比。
 * - `HeuristicEngine`：确定性规则，永远可用，但刻意产出中性值并标注「请勿据此决策」。
 *
 * 一条贯穿三个实现的铁律：**绝不捏造确定性**。拿不到真实判断时宁可失败。
 */

import { readLlmConfig, type ProviderConfig } from '../../../config'
import { streamModelChat, type ChatCompletionMessageParam } from '../../../ai/stream'
import {
  clampState,
  DEFAULT_DECISION_BUDGET_MS,
  DEFAULT_DECISION_THRESHOLD,
  DEFAULT_SAMPLE_TIMEOUT_MS,
  DEFAULT_SAMPLES,
  readDecisionConfig,
  type DecisionConfig,
} from './config'
import type {
  ChoiceQuestion,
  DecisionAnswer,
  DecisionEngine,
  DecisionQuestion,
  DecisionRequest,
  DecisionResponse,
  EngineId,
  NoulQuestion,
  ScoreQuestion,
} from './types'

// ---------------------------------------------------------------- 小工具

function stateToText(state: string | Record<string, unknown>): string {
  if (typeof state === 'string') return state
  try {
    return JSON.stringify(state, null, 2)
  } catch {
    return String(state)
  }
}

/** 问题类型校验：调用方（模型）可能给错，这里挡住。 */
export function isQuestionType(value: unknown): value is DecisionQuestion['type'] {
  return value === 'choice' || value === 'noul' || value === 'score'
}

function choiceKeys(question: ChoiceQuestion): string[] {
  return Object.keys(question.criteria ?? {})
}

/**
 * 找出若干候选词里最早出现的那个位置。
 *
 * CJK 不能用 `\b`——那是「单词边界」，对汉字不成立（汉字是非单词字符，
 * `\b是\b` 永远匹配不到）。所以中文走朴素子串查找，并允许用 `skip` 排除歧义命中。
 */
function firstCjkIndex(text: string, tokens: string[], skip?: (index: number) => boolean): number {
  let best = -1
  for (const token of tokens) {
    let from = 0
    for (;;) {
      const index = text.indexOf(token, from)
      if (index === -1) break
      if (!skip || !skip(index)) {
        if (best === -1 || index < best) best = index
        break
      }
      from = index + 1
    }
  }
  return best
}

/** 中文否定词。刻意不含裸「否」之外的歧义用法，见 skip 逻辑。 */
const CJK_NEGATIONS = ['不成立', '不正确', '不符合', '不是', '否']
const CJK_AFFIRMATIONS = ['成立', '正确', '符合', '是']

/** 把 noul 的模型输出解析成 yes/no。容忍多种写法。 */
export function parseNoulAnswer(raw: string): 'yes' | 'no' | null {
  const text = raw.toLowerCase()

  // 1. 优先找 JSON 里的 answer 字段（提示词就是要它这么回）
  const jsonMatch = text.match(/"answer"\s*:\s*"(yes|no|true|false)"/)
  if (jsonMatch) {
    const token = jsonMatch[1]!
    return token === 'yes' || token === 'true' ? 'yes' : 'no'
  }

  // 2. 英文用单词边界（这里 `\b` 是对的：`\bno\b` 不会命中 not / nothing）
  const enYes = text.search(/\b(yes|true)\b/)
  const enNo = text.search(/\b(no|false)\b/)

  // 3. 中文用子串查找。歧义都来自「是否」这个疑问词——它后面的部分往往是**复述问题**
  //    而不是表态（「是否成立？」里的「成立」）。所以把每个「是否」起到下一个标点
  //    （或结尾）为止视作问句片段，落在片段内的肯定/否定词一律不算数。
  //    这样「是否成立？我认为成立」仍能取到后半句的真表态。
  const questionSpans: Array<[number, number]> = []
  {
    let from = 0
    for (;;) {
      const at = text.indexOf('是否', from)
      if (at === -1) break
      const rest = text.slice(at)
      const stop = rest.search(/[？?。．.!！,，、;；:：\n]/)
      const end = stop === -1 ? text.length : at + stop
      questionSpans.push([at, end])
      from = at + 2
    }
  }
  const inQuestionSpan = (index: number): boolean =>
    questionSpans.some(([start, end]) => index >= start && index < end)

  const zhNo = firstCjkIndex(text, CJK_NEGATIONS, inQuestionSpan)
  const zhYes = firstCjkIndex(text, CJK_AFFIRMATIONS, inQuestionSpan)

  const candidates: Array<{ kind: 'yes' | 'no'; at: number }> = []
  if (enYes !== -1) candidates.push({ kind: 'yes', at: enYes })
  if (enNo !== -1) candidates.push({ kind: 'no', at: enNo })
  if (zhYes !== -1) candidates.push({ kind: 'yes', at: zhYes })
  if (zhNo !== -1) candidates.push({ kind: 'no', at: zhNo })

  if (candidates.length === 0) return null
  candidates.sort((a, b) => a.at - b.at)
  return candidates[0]!.kind
}

/** 从模型输出里挑出选项 key 或档位名。 */
export function parseChoiceKey(raw: string, candidates: string[]): string | null {
  const text = raw.toLowerCase()
  const jsonMatch = text.match(/"(?:answer|choice|value)"\s*:\s*"([^"]+)"/)
  const haystack = jsonMatch ? jsonMatch[1]! : text
  // 长 key 优先，避免 "a" 命中 "abc" 里的 a
  const sorted = [...candidates].sort((a, b) => b.length - a.length)
  for (const key of sorted) {
    if (haystack.includes(key.toLowerCase())) return key
  }
  return null
}

/** 带超时的 promise 包装：超时返回 null，不抛。 */
function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T | null> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(null), ms)
    promise.then(
      (value) => {
        clearTimeout(timer)
        resolve(value)
      },
      () => {
        clearTimeout(timer)
        resolve(null)
      },
    )
  })
}

// ---------------------------------------------------------------- 启发式引擎

/**
 * 启发式兜底：**不做任何真实判断**。
 *
 * 刻意产出中性值（choice 取第一个 key、noul 给 0.5、score 取中间档），
 * 并标明 confidence=0 —— 目的只是让工具可用且不误导，而不是猜一个方向。
 */
export class HeuristicEngine implements DecisionEngine {
  readonly id: EngineId = 'heuristic'

  async isAvailable(): Promise<boolean> {
    return true
  }

  async evaluate(request: DecisionRequest): Promise<DecisionResponse> {
    const started = Date.now()
    const answers: Record<string, DecisionAnswer> = {}

    for (const [id, question] of Object.entries(request.questions)) {
      if (question.type === 'choice') {
        const keys = choiceKeys(question)
        const first = keys[0] ?? ''
        answers[id] = {
          type: 'choice',
          value: first,
          confidence: 0,
          distribution: Object.fromEntries(keys.map((k) => [k, keys.length ? 1 / keys.length : 0])),
          calibrated: false,
        }
      } else if (question.type === 'noul') {
        answers[id] = { type: 'noul', value: 0.5, confidence: 0, calibrated: false }
      } else {
        const levels = question.criteria ?? []
        const middle = levels.length ? levels[Math.floor((levels.length - 1) / 2)]! : ''
        answers[id] = {
          type: 'score',
          value: middle,
          confidence: 0,
          distribution: Object.fromEntries(levels.map((l) => [l, levels.length ? 1 / levels.length : 0])),
          calibrated: false,
        }
      }
    }

    return {
      answers,
      engine: 'heuristic',
      elapsedMs: Date.now() - started,
      samples: 1,
      notes: [
        '无可用决策引擎（既未配置 Jev 端点，也未配置模型），以下为占位结果：数值刻意保持中性，',
        'confidence 为 0。**请勿据此做决策**，需要真实判断请先配置模型或 Jev 端点。',
      ],
    }
  }
}

// ---------------------------------------------------------------- 本地引擎

/** 单次采样的原始输出（模型返回的文本）。 */
export interface SampleResult {
  text: string
}

/**
 * 用模型自身做一次性判断的采样器。
 *
 * 抽成可替换的函数是为了测试：真实实现走 streamModelChat，测试里注入假响应，
 * 从而把「投票数学」与「网络调用」分开验证。
 */
export type SampleRunner = (
  config: ProviderConfig,
  prompt: string,
  systemPrompt: string,
  signal?: AbortSignal,
) => Promise<SampleResult | null>

const defaultSampleRunner: SampleRunner = async (config, prompt, systemPrompt, signal) => {
  const messages: ChatCompletionMessageParam[] = [{ role: 'user', content: prompt }]
  let text = ''
  for await (const chunk of streamModelChat(config, messages, {
    systemPrompt,
    // 没有工具，纯粹要一段结构化回答
    tools: undefined,
    // 采样需要随机性，投票才有意义
    temperature: 0.7,
    signal,
  })) {
    if (chunk.type === 'text' && chunk.text) text += chunk.text
    if (chunk.type === 'error') return null
  }
  return text.trim() ? { text } : null
}

const LOCAL_SYSTEM_PROMPT = [
  'You are a calibration-focused judge.',
  'Answer the question about the provided state.',
  'Reply with ONLY a JSON object, no prose and no code fence.',
  'Be honest about uncertainty: if the state does not support a confident judgment, do not fake confidence.',
].join('\n')

/**
 * 本地模型自评引擎。
 *
 * 关键设计：**不采信单次自报概率**。聊天模型报的概率普遍偏乐观，直接用会制造虚假确定性。
 * 因此对每个问题采样 N 次，用**投票占比**作为主值（经验上比自报数值稳定），
 * 自报均值只作为参考写进 notes。所有结果 calibrated=false。
 */
export class LocalEngine implements DecisionEngine {
  readonly id: EngineId = 'local'

  constructor(
    private readonly options: {
      samples?: number
      sampleTimeoutMs?: number
      budgetMs?: number
      /** 测试注入点 */
      runner?: SampleRunner
      /** 测试注入点：绕过 readLlmConfig */
      config?: ProviderConfig
    } = {},
  ) {}

  private cachedRunner?: SampleRunner

  async isAvailable(): Promise<boolean> {
    if (this.options.config) return true
    const config = await readLlmConfig()
    return config !== null
  }

  private getRunner(): SampleRunner {
    if (!this.cachedRunner) {
      this.cachedRunner = this.options.runner ?? defaultSampleRunner
    }
    return this.cachedRunner
  }

  private async resolveConfig(): Promise<ProviderConfig | null> {
    if (this.options.config) return this.options.config
    const config = await readLlmConfig()
    if (!config) return null
    return {
      baseUrl: config.baseUrl,
      apiKey: config.apiKey,
      model: config.model,
      contextWindow: config.contextWindow,
      supportsImages: config.supportsImages,
    }
  }

  async evaluate(request: DecisionRequest, signal?: AbortSignal): Promise<DecisionResponse> {
    const started = Date.now()
    const config = await this.resolveConfig()
    if (!config) {
      throw new Error('未配置模型，无法使用本地决策引擎。')
    }

    const samples = Math.max(1, this.options.samples ?? DEFAULT_SAMPLES)
    const sampleTimeout = this.options.sampleTimeoutMs ?? DEFAULT_SAMPLE_TIMEOUT_MS
    const budget = this.options.budgetMs ?? DEFAULT_DECISION_BUDGET_MS

    const { text: stateText, truncated } = clampState(stateToText(request.state))
    const notes: string[] = []
    if (truncated) notes.push('state 过长已截断，判断可能未覆盖全部内容。')

    const answers: Record<string, DecisionAnswer> = {}
    let failedSamples = 0

    for (const [id, question] of Object.entries(request.questions)) {
      // 每个问题独立跑 N 次采样；同一问题的多次采样并发
      const prompt = this.buildPrompt(stateText, id, question)
      const runs: Array<Promise<SampleResult | null>> = []
      for (let i = 0; i < samples; i++) {
        const remaining = budget - (Date.now() - started)
        if (remaining <= 0) {
          runs.push(Promise.resolve(null))
          continue
        }
        runs.push(withTimeout(this.getRunner()(config, prompt, LOCAL_SYSTEM_PROMPT, signal), Math.min(sampleTimeout, remaining)))
      }
      const results = await Promise.all(runs)
      const texts = results.filter((r): r is SampleResult => r !== null).map((r) => r.text)
      if (texts.length === 0) {
        failedSamples += 1
        // 拿不到任何样本：如实标成低置信的默认值，而不是编一个概率
        answers[id] = this.emptyAnswer(question)
        continue
      }
      if (texts.length < samples) failedSamples += 1
      answers[id] = this.aggregate(question, texts)
    }

    if (failedSamples > 0) {
      notes.push(`有 ${failedSamples} 个问题的部分采样未返回（超时或出错），结果基于已到的样本。`)
    }
    notes.push(
      `本地自评：概率来自 ${samples} 次采样的投票占比，**未经校准**，不宜直接当概率解释。`,
    )

    return {
      answers,
      engine: 'local',
      model: config.model,
      elapsedMs: Date.now() - started,
      samples,
      notes,
    }
  }

  /** 拿不到样本时的答案：中性 + confidence 0，并标注未校准。 */
  private emptyAnswer(question: DecisionQuestion): DecisionAnswer {
    if (question.type === 'choice') {
      return { type: 'choice', value: choiceKeys(question)[0] ?? '', confidence: 0, calibrated: false }
    }
    if (question.type === 'noul') {
      return { type: 'noul', value: 0.5, confidence: 0, calibrated: false }
    }
    const levels = question.criteria ?? []
    return {
      type: 'score',
      value: levels.length ? levels[Math.floor((levels.length - 1) / 2)]! : '',
      confidence: 0,
      calibrated: false,
    }
  }

  private buildPrompt(
    stateText: string,
    id: string,
    question: DecisionQuestion,
  ): string {
    const lines = [`## State`, stateText, '', `## Question (id: ${id})`, question.instructions]
    if (question.type === 'choice') {
      lines.push('', 'Choose exactly one of these options (reply with the option key):')
      for (const [key, desc] of Object.entries(question.criteria ?? {})) {
        lines.push(`- "${key}": ${desc ?? '(no description)'}`)
      }
      lines.push('', 'Reply as JSON: {"answer": "<option key>", "probability": <0-1>}')
    } else if (question.type === 'noul') {
      lines.push(
        '',
        'This is a yes/no question.',
        'Reply as JSON: {"answer": "yes" | "no", "probability": <your probability that the answer is "yes", 0-1>}',
      )
    } else {
      lines.push('', 'Score against these levels (highest first):')
      question.criteria.forEach((level, index) => lines.push(`${index + 1}. ${level}`))
      lines.push('', 'Reply as JSON: {"answer": "<level name>", "probability": <0-1>}')
    }
    return lines.join('\n')
  }

  /**
   * 汇总 N 次采样。
   *
   * noul 用「yes 票数 / 总票数」——投票占比在经验上比自报概率均值稳得多；
   * choice 用众数，平票按 criteria 的 key 顺序取第一个（确定性打破平局）；
   * score 用加权期望档位，比众数更能反映倾向。
   */
  private aggregate(question: DecisionQuestion, texts: string[]): DecisionAnswer {
    if (question.type === 'noul') {
      let yes = 0
      let total = 0
      const selfReported: number[] = []
      for (const text of texts) {
        const parsed = parseNoulAnswer(text)
        if (!parsed) continue
        total += 1
        if (parsed === 'yes') yes += 1
        const prob = text.match(/"probability"\s*:\s*([0-9.]+)/)
        if (prob) {
          const value = Number(prob[1])
          if (Number.isFinite(value) && value >= 0 && value <= 1) selfReported.push(value)
        }
      }
      if (total === 0) return this.emptyAnswer(question)
      const voteRatio = yes / total
      const mean = selfReported.length
        ? selfReported.reduce((a, b) => a + b, 0) / selfReported.length
        : undefined
      const answer: DecisionAnswer = {
        type: 'noul',
        value: Number(voteRatio.toFixed(4)),
        distribution: {
          yes: Number(voteRatio.toFixed(4)),
          no: Number((1 - voteRatio).toFixed(4)),
        },
        calibrated: false,
      }
      if (mean !== undefined) answer.confidence = Number(mean.toFixed(4))
      return answer
    }

    if (question.type === 'choice') {
      const keys = choiceKeys(question)
      const votes: Record<string, number> = Object.fromEntries(keys.map((k) => [k, 0]))
      for (const text of texts) {
        const picked = parseChoiceKey(text, keys)
        if (picked) votes[picked] = (votes[picked] ?? 0) + 1
      }
      const total = Object.values(votes).reduce((a, b) => a + b, 0)
      if (total === 0) return this.emptyAnswer(question)
      // 平票时取 criteria 里靠前的那个：确定性，避免同一输入两次答案不同
      let winner = keys[0] ?? ''
      for (const key of keys) {
        if ((votes[key] ?? 0) > (votes[winner] ?? 0)) winner = key
      }
      const distribution: Record<string, number> = {}
      for (const key of keys) distribution[key] = Number(((votes[key] ?? 0) / total).toFixed(4))
      return {
        type: 'choice',
        value: winner,
        confidence: Number(((votes[winner] ?? 0) / total).toFixed(4)),
        distribution,
        calibrated: false,
      }
    }

    const levels = question.criteria ?? []
    const votes: Record<string, number> = Object.fromEntries(levels.map((l) => [l, 0]))
    for (const text of texts) {
      const picked = parseChoiceKey(text, levels)
      if (picked) votes[picked] = (votes[picked] ?? 0) + 1
    }
    const total = Object.values(votes).reduce((a, b) => a + b, 0)
    if (total === 0 || levels.length === 0) return this.emptyAnswer(question)

    // 加权期望：levels[0] 最高分，依次递减
    let weighted = 0
    for (const [index, level] of levels.entries()) {
      const score = levels.length - index
      weighted += (votes[level]! / total) * score
    }
    // 期望分四舍五入到最近的档位
    const expectedIndex = Math.min(
      levels.length - 1,
      Math.max(0, Math.round(levels.length - weighted)),
    )
    const distribution: Record<string, number> = {}
    for (const level of levels) distribution[level] = Number(((votes[level] ?? 0) / total).toFixed(4))
    return {
      type: 'score',
      value: levels[expectedIndex]!,
      confidence: Number(((votes[levels[expectedIndex]!] ?? 0) / total).toFixed(4)),
      distribution,
      calibrated: false,
    }
  }
}

// ---------------------------------------------------------------- Jev 引擎

/** Jev 兼容端点的响应形状（只取需要的字段）。 */
interface JevRawAnswer {
  choice?: string
  value?: string | number
  noul?: number
  probability?: number
  score?: number
  confidence?: number
  distribution?: Record<string, number>
}

export class JevEngine implements DecisionEngine {
  readonly id: EngineId = 'jev'

  constructor(private readonly options: { baseUrl: string; apiKey: string }) {}

  async isAvailable(): Promise<boolean> {
    return Boolean(this.options.baseUrl)
  }

  /** 把内部问题定义翻成 Jev 的 systemOne 请求体。 */
  private toJevQuestions(request: DecisionRequest): Record<string, unknown> {
    const questions: Record<string, unknown> = {}
    for (const [id, question] of Object.entries(request.questions)) {
      if (question.type === 'choice') {
        questions[id] = { type: 'choice', instructions: question.instructions, criteria: question.criteria }
      } else if (question.type === 'noul') {
        questions[id] = { type: 'noul', instructions: question.instructions }
      } else {
        questions[id] = { type: 'score', instructions: question.instructions, criteria: question.criteria }
      }
    }
    return questions
  }

  async evaluate(request: DecisionRequest, signal?: AbortSignal): Promise<DecisionResponse> {
    const started = Date.now()
    const url = `${this.options.baseUrl.replace(/\/+$/, '')}/systemOne`
    const { text: stateText, truncated } = clampState(stateToText(request.state))

    const body = {
      state: { text: stateText },
      questions: this.toJevQuestions(request),
    }

    const headers: Record<string, string> = { 'content-type': 'application/json' }
    if (this.options.apiKey) headers.authorization = `Bearer ${this.options.apiKey}`

    const response = await fetch(url, {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
      signal,
    })

    if (!response.ok) {
      const detail = await response.text().catch(() => '')
      throw new Error(
        `Jev 端点返回 ${response.status}${detail ? `：${detail.slice(0, 200)}` : ''}`,
      )
    }

    const payload = (await response.json()) as {
      answers?: Record<string, JevRawAnswer>
      model?: string
    }
    const rawAnswers = payload.answers ?? {}
    const answers: Record<string, DecisionAnswer> = {}

    for (const [id, question] of Object.entries(request.questions)) {
      const raw = rawAnswers[id]
      if (!raw) {
        answers[id] = {
          type: question.type,
          value: question.type === 'noul' ? 0.5 : '',
          confidence: 0,
          calibrated: false,
        }
        continue
      }
      if (question.type === 'choice') {
        answers[id] = {
          type: 'choice',
          value: String(raw.choice ?? raw.value ?? ''),
          confidence: raw.confidence,
          distribution: raw.distribution,
          // 专用 System One 模型，概率是校准的
          calibrated: true,
        }
      } else if (question.type === 'noul') {
        answers[id] = {
          type: 'noul',
          value: Number(raw.noul ?? raw.probability ?? raw.value ?? 0.5),
          calibrated: true,
        }
      } else {
        const numeric = Number(raw.score ?? raw.value ?? 0)
        const levels = (question as ScoreQuestion).criteria ?? []
        // Jev 的 score 可能是数值（档位序）或档位名，两种都认
        const value = typeof raw.value === 'string' && levels.includes(raw.value)
          ? raw.value
          : levels[Math.min(levels.length - 1, Math.max(0, numeric - 1))] ?? String(numeric)
        answers[id] = {
          type: 'score',
          value,
          confidence: raw.confidence,
          distribution: raw.distribution,
          calibrated: true,
        }
      }
    }

    const notes: string[] = []
    if (truncated) notes.push('state 过长已截断，判断可能未覆盖全部内容。')

    return {
      answers,
      engine: 'jev',
      model: payload.model ?? 'jev',
      elapsedMs: Date.now() - started,
      samples: 1,
      notes,
    }
  }
}

// ---------------------------------------------------------------- 引擎解析

export interface ResolvedEngine {
  engine: DecisionEngine
  /** 用户偏好与实际落点的说明，写进工具返回的 notes */
  note?: string
}

/**
 * 按偏好与可用性解析出实际引擎。
 *
 * `auto` 走 jev → local → heuristic；显式指定则只认那一个（不可用即报错，
 * 而不是悄悄降级——用户明确要求某个引擎时，静默换掉比失败更糟）。
 */
export async function resolveEngine(
  config?: DecisionConfig,
  overrides: { engine?: DecisionEngine } = {},
): Promise<ResolvedEngine> {
  const resolved = config ?? (await readDecisionConfig())

  if (overrides.engine) return { engine: overrides.engine }

  const jev = new JevEngine({ baseUrl: resolved.baseUrl, apiKey: resolved.apiKey })
  const local = new LocalEngine({ samples: resolved.samples, sampleTimeoutMs: resolved.sampleTimeoutMs })
  const heuristic = new HeuristicEngine()

  if (resolved.engine === 'heuristic') return { engine: heuristic }
  if (resolved.engine === 'jev') {
    if (!(await jev.isAvailable())) {
      throw new Error('已指定使用 Jev 引擎，但未配置端点（A_DA_DECISION_BASE_URL / PI_JEV_BASE_URL）。')
    }
    return { engine: jev }
  }
  if (resolved.engine === 'local') {
    if (!(await local.isAvailable())) {
      throw new Error('已指定使用本地引擎，但未配置模型。请在设置里填写供应商。')
    }
    return { engine: local }
  }

  // auto
  if (await jev.isAvailable()) return { engine: jev }
  if (await local.isAvailable()) {
    return { engine: local, note: '未配置 Jev 端点，已回退至本地模型自评（概率未经校准）。' }
  }
  return {
    engine: heuristic,
    note: '既未配置 Jev 端点也未配置模型，已回退至确定性占位结果。',
  }
}

/** 取单题阈值：题级覆盖 > 调用级 > 默认。 */
export function resolveThreshold(
  requestThreshold: number | undefined,
  configThreshold: number | undefined,
): number {
  if (typeof requestThreshold === 'number' && requestThreshold > 0) return requestThreshold
  if (typeof configThreshold === 'number' && configThreshold > 0) return configThreshold
  return DEFAULT_DECISION_THRESHOLD
}
