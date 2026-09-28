/**
 * 决策门禁：判定「这份产出是否满足验收标准」。
 *
 * 对齐 pi-jev 的 `jev-gate`，但有一处**刻意的相反设计**：
 * 引擎不可用时默认 **fail-close**（视为未通过），而 `decide` 是直接失败。
 *
 * 理由是不对称的代价：决策拿不到值，调用方知道「没答案」；门禁拿不到值若默认放行，
 * 就成了「验收永远通过」——那比拒绝更危险。需要放宽时由调用方显式传 fail_open。
 */

import { execFile } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { promisify } from 'node:util'
import { checkWorkspaceSandbox } from '../../workspace'
import { clampState, DEFAULT_GATE_THRESHOLD } from './config'
import { resolveEngine, type ResolvedEngine } from './engine'
import type { DecisionConfig } from './config'
import type { DecisionEngine, GateOutcome } from './types'

const execFileAsync = promisify(execFile)

export type GateSource = 'diff' | 'file' | 'text'

export interface GateOptions {
  criteria: string
  source: GateSource
  file?: string
  text?: string
  threshold?: number
  failOpen?: boolean
  workspace: string
  signal?: AbortSignal
  /** 测试注入点 */
  engine?: DecisionEngine
  config?: DecisionConfig
}

/** git diff 的输出上限，避免超大 diff 撑爆请求。 */
const MAX_DIFF_CHARS = 20_000

/**
 * 取 git diff 作为判定材料。
 *
 * 先看 `git diff HEAD`（工作区未提交改动），空则退到 `git diff --cached`（已暂存）。
 * 执行方式与 git-tools 插件一致：execFile，不走 shell。
 */
export async function readGitDiff(workspace: string): Promise<string> {
  const tryDiff = async (args: string[]): Promise<string> => {
    try {
      const { stdout } = await execFileAsync('git', args, {
        cwd: workspace,
        maxBuffer: 8 * 1024 * 1024,
        windowsHide: true,
      })
      return stdout
    } catch {
      return ''
    }
  }

  const working = await tryDiff(['diff', 'HEAD'])
  if (working.trim()) return working
  const staged = await tryDiff(['diff', '--cached'])
  if (staged.trim()) return staged
  return ''
}

/** 解析出用于判定的 state 文本。 */
export async function resolveGateState(options: GateOptions): Promise<{ text: string; note?: string }> {
  if (options.source === 'text') {
    return { text: options.text ?? '' }
  }

  if (options.source === 'file') {
    if (!options.file?.trim()) {
      throw new Error('source=file 时必须提供 file 参数。')
    }
    const full = checkWorkspaceSandbox(options.workspace, options.file.trim())
    const content = await readFile(full, 'utf8')
    return { text: content }
  }

  const diff = await readGitDiff(options.workspace)
  if (!diff.trim()) {
    // 没有改动是常见情形（例如刚跑完一轮没动文件），如实说明而不是假装通过
    return { text: '', note: '工作区没有检测到 git 改动（git diff 与 --cached 均为空）。' }
  }
  if (diff.length > MAX_DIFF_CHARS) {
    return {
      text: diff.slice(0, MAX_DIFF_CHARS),
      note: `diff 过大（${diff.length} 字符），已截断至 ${MAX_DIFF_CHARS} 字符后判定。`,
    }
  }
  return { text: diff }
}

/**
 * 跑一次门禁判定。
 *
 * 返回结构里始终带 `engine` 与 `calibrated`，让调用方知道这个「通过」有多少分量：
 * 启发式给的 passed 是没有依据的，必须让上层看得见。
 */
export async function runGate(options: GateOptions): Promise<GateOutcome> {
  const criteria = options.criteria.trim()
  if (!criteria) {
    throw new Error('缺少 criteria 参数：需要给出验收标准。')
  }

  const threshold = options.threshold && options.threshold > 0 ? options.threshold : DEFAULT_GATE_THRESHOLD
  const failOpen = Boolean(options.failOpen)

  const resolved: ResolvedEngine = await resolveEngine(options.config, {
    engine: options.engine,
  })
  const engine = resolved.engine
  const notes: string[] = []
  if (resolved.note) notes.push(resolved.note)

  // 启发式引擎不做真实判断：默认视为未通过（fail-close），除非显式放宽
  if (engine.id === 'heuristic') {
    if (!failOpen) {
      return {
        passed: false,
        probability: 0,
        threshold,
        criteria,
        engine: engine.id,
        calibrated: false,
        elapsedMs: 0,
        note: '无可用决策引擎，门禁默认不通过（fail-close）。配置模型或 Jev 端点后重试，或显式传 fail_open。',
      }
    }
    return {
      passed: true,
      probability: 1,
      threshold,
      criteria,
      engine: engine.id,
      calibrated: false,
      elapsedMs: 0,
      note: '无可用决策引擎，因 fail_open=true 而放行——**此结果无判定依据**。',
    }
  }

  const { text: stateText, note: stateNote } = await resolveGateState(options)
  if (stateNote) notes.push(stateNote)
  const { text: clamped, truncated } = clampState(stateText)
  if (truncated) notes.push('判定材料过长已截断。')

  const response = await engine.evaluate(
    {
      state: clamped,
      questions: {
        gate_passed: {
          type: 'noul',
          instructions: `Does the provided code/output satisfy this acceptance criteria: "${criteria}"?`,
        },
      },
    },
    options.signal,
  )

  const answer = response.answers.gate_passed
  const probability = typeof answer?.value === 'number' ? answer.value : 0
  const passed = probability >= threshold

  notes.push(...response.notes)

  return {
    passed,
    probability: Number(probability.toFixed(4)),
    threshold,
    criteria,
    engine: response.engine,
    calibrated: Boolean(answer?.calibrated),
    elapsedMs: response.elapsedMs,
    note: notes.length ? notes.join(' ') : undefined,
  }
}
