/**
 * 决策插件的三个工具。
 *
 * 工具层只做三件事：校验入参、调引擎/门禁、把结果整理成模型与卡片都能读的形状。
 * 判断逻辑全在 engine.ts / designer.ts / gate.ts，这样测试不必穿过工具层。
 */

import type { AgentTool, AgentToolResult } from '../../../core/types'
import { DEFAULT_GATE_THRESHOLD, readDecisionConfig } from './config'
import { designDecision } from './designer'
import { resolveEngine, resolveThreshold } from './engine'
import { runGate, type GateSource } from './gate'
import type { DecisionAnswer, DecisionQuestion } from './types'

// ---------------------------------------------------------------- 共用

interface QuestionInput {
  type?: unknown
  instructions?: unknown
  criteria?: unknown
}

/**
 * 校验模型给的 questions 入参。
 *
 * 与 design_decision 的 validateDesign 同一套规则（这里是模型直接手写的，
 * 那边是模型设计出来的），复用同一份判定以免两处标准漂移。
 */
function normalizeQuestions(
  raw: unknown,
): { questions: Record<string, DecisionQuestion>; dropped: string[] } {
  const questions: Record<string, DecisionQuestion> = {}
  const dropped: string[] = []
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    return { questions, dropped }
  }

  for (const [id, value] of Object.entries(raw as Record<string, unknown>)) {
    const q = (value ?? {}) as QuestionInput
    const instructions = typeof q.instructions === 'string' ? q.instructions.trim() : ''
    if (!instructions) {
      dropped.push(`${id}：缺少 instructions`)
      continue
    }

    if (q.type === 'noul') {
      questions[id] = { type: 'noul', instructions }
      continue
    }

    if (q.type === 'choice') {
      if (!q.criteria || typeof q.criteria !== 'object' || Array.isArray(q.criteria)) {
        dropped.push(`${id}：choice 的 criteria 必须是「选项 key → 说明」的对象`)
        continue
      }
      const entries = Object.entries(q.criteria as Record<string, unknown>)
      if (entries.length === 0) {
        dropped.push(`${id}：choice 的 criteria 为空`)
        continue
      }
      const criteria: Record<string, string | null> = {}
      for (const [key, desc] of entries) {
        criteria[key] = typeof desc === 'string' ? desc : null
      }
      questions[id] = { type: 'choice', instructions, criteria }
      continue
    }

    if (q.type === 'score') {
      if (!Array.isArray(q.criteria) || q.criteria.length === 0) {
        dropped.push(`${id}：score 的 criteria 必须是非空档位数组（最高档在前）`)
        continue
      }
      const levels = q.criteria.filter(
        (level): level is string => typeof level === 'string' && level.trim().length > 0,
      )
      if (levels.length === 0) {
        dropped.push(`${id}：score 的 criteria 无有效档位`)
        continue
      }
      questions[id] = { type: 'score', instructions, criteria: levels }
      continue
    }

    dropped.push(`${id}：type 必须是 choice / noul / score`)
  }

  return { questions, dropped }
}

/** 把答案渲染成模型与人都好读的文本。 */
function formatAnswers(
  answers: Record<string, DecisionAnswer>,
  engine: string,
  samples: number,
  notes: string[],
): string {
  const lines: string[] = [`## 决策结果（引擎: ${engine}，采样: ${samples}）`, '']

  for (const [id, answer] of Object.entries(answers)) {
    lines.push(`### ${id}`)
    if (answer.type === 'noul') {
      lines.push(`概率: ${answer.value}`)
    } else {
      lines.push(`结论: ${answer.value}`)
    }
    if (answer.confidence !== undefined) lines.push(`置信: ${answer.confidence}`)
    if (answer.distribution) {
      const dist = Object.entries(answer.distribution)
        .map(([key, value]) => `${key}=${value}`)
        .join(', ')
      lines.push(`分布: ${dist}`)
    }
    lines.push(`已校准: ${answer.calibrated ? '是' : '否'}`)
    lines.push('')
  }

  if (notes.length > 0) {
    lines.push('> ' + notes.join('\n> '))
  }
  return lines.join('\n')
}

// ---------------------------------------------------------------- decide

export interface DecideToolArgs {
  state: string | Record<string, unknown>
  questions: Record<string, { type: string; instructions: string; criteria?: unknown }>
  threshold?: number
}

export function createDecideTool(): AgentTool<DecideToolArgs> {
  return {
    name: 'decide',
    label: '类型化决策',
    description: [
      '对给定材料做结构化判断，返回 choice（多选一）/ noul（是-否概率）/ score（按档位评分）三种类型的结果。',
      '',
      '**什么时候用它**：需要的是「判断」而不是「生成一段话」时——严重度分级、失败归因分类、',
      '验收判定、二选一取舍、按评分标准打分。这类问题的答案应当是可比、可设阈值、可累计的，',
      '而不是埋在散文里让下一轮重新解析。',
      '',
      '**注意概率的可靠性**：返回里每条答案都带 calibrated 字段。远端 Jev 引擎（专用 System One',
      '模型）为 true；本地模型自评与启发式兜底为 false，前者的概率是多次采样的投票占比、',
      '**未经校准**，不宜直接当概率解释。engine 字段会说明实际用了哪个引擎。',
      '',
      '**没有可用引擎时会失败**，不会编造数字。需要先配置模型或 Jev 端点。',
    ].join('\n'),
    executionMode: 'sequential',
    parameters: {
      type: 'object',
      properties: {
        state: {
          type: ['string', 'object'],
          description: '被判断的材料（一段文本或结构化对象）。务必自包含：不要引用「上文」「刚才那段代码」之类模型看不见的东西。',
        },
        questions: {
          type: 'object',
          description: [
            '问题 id → 问题定义。规则：',
            '- noul：只要 instructions，**不要给 criteria**（纯是/否问题）',
            '- choice：需要 criteria 为「选项 key → 说明」的对象，如 {"a":"方案甲","b":"方案乙"}',
            '- score：需要 criteria 为非空档位数组（最高档在前），如 ["无风险","低风险","高风险"]',
          ].join('\n'),
          additionalProperties: {
            type: 'object',
            properties: {
              type: { type: 'string', enum: ['choice', 'noul', 'score'] },
              instructions: { type: 'string', description: '具体的判断指令' },
              criteria: { description: 'choice 给对象，score 给数组，noul 不传' },
            },
            required: ['type', 'instructions'],
          },
        },
        threshold: { type: 'number', description: '可选，覆盖默认判定阈值（默认 0.65）' },
      },
      required: ['state', 'questions'],
    },
    async execute(_callId, args, signal, onUpdate): Promise<AgentToolResult> {
      const { questions, dropped } = normalizeQuestions(args?.questions)
      const ids = Object.keys(questions)
      if (ids.length === 0) {
        return {
          output: [
            '没有可执行的问题定义。',
            dropped.length ? `被丢弃的条目：\n- ${dropped.join('\n- ')}` : '',
            '每个问题都要有 type（choice/noul/score）与 instructions；choice 需要 criteria 对象，score 需要 criteria 数组。',
          ]
            .filter(Boolean)
            .join('\n'),
          ok: false,
        }
      }

      if (args?.state === undefined || args?.state === null) {
        return { output: '缺少 state 参数：需要给出被判断的材料。', ok: false }
      }

      onUpdate?.({ output: `正在对 ${ids.length} 个问题进行判定...`, ok: true })

      const config = await readDecisionConfig()
      const threshold = resolveThreshold(args?.threshold, config.threshold)

      let resolved
      try {
        resolved = await resolveEngine(config)
      } catch (error) {
        return { output: (error as Error).message, ok: false }
      }

      try {
        const response = await resolved.engine.evaluate(
          { state: args.state, questions, threshold },
          signal,
        )

        const notes = [...response.notes]
        if (resolved.note) notes.unshift(resolved.note)
        if (dropped.length > 0) notes.push(`有 ${dropped.length} 个问题定义不合法已丢弃：${dropped.join('；')}`)

        // 需要人复核的判定：noul 概率落在阈值附近时提示，避免把模糊当确定
        for (const [id, answer] of Object.entries(response.answers)) {
          if (answer.type === 'noul' && typeof answer.value === 'number') {
            const distance = Math.abs(answer.value - threshold)
            if (distance < 0.15) {
              notes.push(`问题「${id}」的结论贴近阈值（${answer.value} vs ${threshold}），建议人工复核。`)
            }
          }
        }

        return {
          output: formatAnswers(response.answers, response.engine, response.samples, notes),
          ok: true,
          details: {
            answers: response.answers,
            engine: response.engine,
            model: response.model,
            samples: response.samples,
            elapsedMs: response.elapsedMs,
            threshold,
            notes,
            dropped,
          },
        }
      } catch (error) {
        return {
          output: `决策执行失败：${(error as Error).message}`,
          ok: false,
          details: { engine: resolved.engine.id },
        }
      }
    },
  }
}

// ---------------------------------------------------------------- design_decision

export interface DesignDecisionToolArgs {
  prompt: string
  max_questions?: number
}

export function createDesignDecisionTool(): AgentTool<DesignDecisionToolArgs> {
  return {
    name: 'design_decision',
    label: '设计并执行决策',
    description: [
      '给一段自由描述的需求，自动设计出合适的决策问题，并立即执行判定。',
      '',
      '**什么时候用它**：你大概知道要「判断点什么」，但一时说不清该问什么问题时。它会先让模型',
      '把需求翻译成结构化的问题（choice / noul / score），再交给决策引擎评估。比手工拼 decide 的',
      'questions 省事，适合探索性判定。',
      '',
      '如果需求本身就明确（例如「按这三个档位给严重度打分」），直接用 decide 更可控。',
    ].join('\n'),
    executionMode: 'sequential',
    parameters: {
      type: 'object',
      properties: {
        prompt: {
          type: 'string',
          description: '自由描述要判断什么，例如「判断这次重构是否引入了破坏性变更」。描述里要包含被判断的材料或指向它的线索。',
        },
        max_questions: {
          type: 'number',
          description: '可选，最多设计几个问题（上限 6，默认 6）',
        },
      },
      required: ['prompt'],
    },
    async execute(_callId, args, signal, onUpdate): Promise<AgentToolResult> {
      const prompt = String(args?.prompt ?? '').trim()
      if (!prompt) {
        return { output: '缺少 prompt 参数：需要描述要判断什么。', ok: false }
      }

      onUpdate?.({ output: '正在设计决策问题...', ok: true })

      const config = await readDecisionConfig()
      const design = await designDecision(prompt, {
        maxQuestions: args?.max_questions,
        signal,
        config: undefined,
      })

      if (!design.design) {
        return {
          output: design.error ?? '决策设计失败。',
          ok: false,
          details: design.raw ? { raw: design.raw.slice(0, 2000) } : undefined,
        }
      }

      const ids = Object.keys(design.design.questions)
      onUpdate?.({
        output: `已设计 ${ids.length} 个问题（${ids.join(', ')}），正在判定...`,
        ok: true,
      })

      let resolved
      try {
        resolved = await resolveEngine(config)
      } catch (error) {
        return { output: (error as Error).message, ok: false }
      }

      try {
        const response = await resolved.engine.evaluate(design.design, signal)
        const notes = [...response.notes]
        if (resolved.note) notes.unshift(resolved.note)

        const designed = ids
          .map((id) => {
            const q = design.design!.questions[id]!
            return `- ${id}（${q.type}）：${q.instructions}`
          })
          .join('\n')

        return {
          output: [
            '### 自动设计的决策问题',
            designed,
            '',
            formatAnswers(response.answers, response.engine, response.samples, notes),
          ].join('\n'),
          ok: true,
          details: {
            designed: design.design.questions,
            answers: response.answers,
            engine: response.engine,
            samples: response.samples,
            elapsedMs: response.elapsedMs,
            notes,
          },
        }
      } catch (error) {
        return { output: `决策执行失败：${(error as Error).message}`, ok: false }
      }
    },
  }
}

// ---------------------------------------------------------------- check_gate

export interface CheckGateToolArgs {
  criteria: string
  source?: GateSource
  file?: string
  text?: string
  threshold?: number
  fail_open?: boolean
}

export function createCheckGateTool(workspace: string): AgentTool<CheckGateToolArgs> {
  return {
    name: 'check_gate',
    label: '验收门禁',
    description: [
      '判定一份产出是否满足给定的验收标准，返回通过与否以及概率。',
      '',
      '**什么时候用它**：完成一轮改动后自检是否达到验收标准，或判断某个产出能否交付。',
      'source 指定判定材料：`diff`（默认，读 git 未提交改动）、`file`（读工作区文件）、',
      '`text`（直接用给定文本）。',
      '',
      '**注意失败方向**：没有可用决策引擎时默认判为**不通过**（fail-close），因为「门禁永远放行」',
      '比「要求人工复核」危险得多。确实需要放宽时显式传 fail_open: true，但那样结果没有判定依据。',
    ].join('\n'),
    executionMode: 'sequential',
    parameters: {
      type: 'object',
      properties: {
        criteria: {
          type: 'string',
          description: '验收标准（自然语言），例如「所有导出的函数都有类型标注且未引入 any」。',
        },
        source: {
          type: 'string',
          enum: ['diff', 'file', 'text'],
          description: '判定材料来源，默认 diff（git 未提交改动）。',
        },
        file: { type: 'string', description: 'source=file 时必填，工作区内的相对路径。' },
        text: { type: 'string', description: 'source=text 时必填，直接给出要判定的内容。' },
        threshold: { type: 'number', description: `通过阈值（默认 ${DEFAULT_GATE_THRESHOLD}）` },
        fail_open: { type: 'boolean', description: '无可用引擎时是否放行（默认 false，即 fail-close）' },
      },
      required: ['criteria'],
    },
    async execute(_callId, args, signal, onUpdate): Promise<AgentToolResult> {
      const criteria = String(args?.criteria ?? '').trim()
      if (!criteria) {
        return { output: '缺少 criteria 参数：需要给出验收标准。', ok: false }
      }

      const source: GateSource = args?.source === 'file' || args?.source === 'text' ? args.source : 'diff'
      if (source === 'file' && !args?.file?.trim()) {
        return { output: 'source=file 时必须提供 file 参数。', ok: false }
      }
      if (source === 'text' && !String(args?.text ?? '').trim()) {
        return { output: 'source=text 时必须提供 text 参数。', ok: false }
      }

      onUpdate?.({ output: `正在按验收标准判定（source=${source}）...`, ok: true })

      const config = await readDecisionConfig()

      try {
        const outcome = await runGate({
          criteria,
          source,
          file: args?.file,
          text: args?.text,
          threshold: args?.threshold,
          failOpen: args?.fail_open,
          workspace,
          signal,
          config,
        })

        const verdict = outcome.passed ? '✅ 通过' : '❌ 未通过'
        const lines = [
          `${verdict}`,
          '',
          `验收标准: ${outcome.criteria}`,
          `概率: ${outcome.probability}（阈值 ${outcome.threshold}）`,
          `引擎: ${outcome.engine}　已校准: ${outcome.calibrated ? '是' : '否'}`,
        ]
        if (outcome.note) lines.push('', `> ${outcome.note}`)

        return {
          output: lines.join('\n'),
          ok: true,
          details: outcome,
        }
      } catch (error) {
        return { output: `门禁判定失败：${(error as Error).message}`, ok: false }
      }
    },
  }
}
