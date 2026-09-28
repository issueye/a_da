/**
 * 决策插件的配置解析。
 *
 * 优先级顺序与 a_da 既有约定一致：环境变量 > `config.json`。此外额外兼容 pi-jev
 * 的变量名（`PI_JEV_BASE_URL` / `TYPESAFE_BASE_URL` / `TYPESAFE_API_KEY`），
 * 让已经配好 Jev 端点的人不必重配一遍。
 *
 * 密钥不写进 config.json 的话，可以放 `~/.a-da/secrets/decision_api_key`
 * （对齐 pi-jev 的 `~/.pi/agent/secrets/typesafe_api_key`）。
 */

import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { readSavedConfig } from '../../../config'
import { getAppHome } from '../../../home'
import type { EngineId } from './types'

/** 单一阈值常量：所有路径都读它，避免各处理解不一致。 */
export const DEFAULT_DECISION_THRESHOLD = 0.65

/** 门禁默认阈值：比通用决策更严（pi-jev 的 gate 也用 0.7）。 */
export const DEFAULT_GATE_THRESHOLD = 0.7

/** 本地引擎默认采样次数。1 次最省，但会失去投票稳定性。 */
export const DEFAULT_SAMPLES = 3

/** 单次采样的超时（毫秒）。 */
export const DEFAULT_SAMPLE_TIMEOUT_MS = 20_000

/** 整批决策的时间预算（毫秒），超了就用已有样本。 */
export const DEFAULT_DECISION_BUDGET_MS = 60_000

/** 送进引擎的 state 上限：避免把整个文件/仓库塞进请求。 */
export const MAX_STATE_CHARS = 24_000

export type EnginePreference = 'auto' | EngineId

export interface DecisionConfig {
  /** 引擎偏好；auto 走 jev → local → heuristic 回退 */
  engine: EnginePreference
  /** Jev 兼容端点 */
  baseUrl: string
  apiKey: string
  threshold: number
  samples: number
  sampleTimeoutMs: number
}

function envValue(...names: string[]): string {
  for (const name of names) {
    const raw = process.env[name]
    if (raw && raw.trim()) return raw.trim()
  }
  return ''
}

/** 从 `~/.a-da/secrets/decision_api_key` 读密钥（存在且非空才认）。 */
function readSecretFile(): string {
  try {
    const path = join(getAppHome(), 'secrets', 'decision_api_key')
    if (!existsSync(path)) return ''
    const content = readFileSync(path, 'utf8').trim()
    return content
  } catch {
    return ''
  }
}

function asEnginePreference(raw: unknown): EnginePreference | undefined {
  if (typeof raw !== 'string') return undefined
  const value = raw.trim().toLowerCase()
  if (value === 'auto' || value === 'jev' || value === 'local' || value === 'heuristic') {
    return value as EnginePreference
  }
  return undefined
}

function asPositiveNumber(raw: unknown): number | undefined {
  const value = typeof raw === 'number' ? raw : Number(raw)
  if (!Number.isFinite(value) || value <= 0) return undefined
  return value
}

/**
 * 解析决策配置。
 *
 * 环境变量整体优先于文件；文件里的 `decision` 块缺失时各项回落到默认值。
 * 密钥的来源顺序：`A_DA_DECISION_API_KEY` → `TYPESAFE_API_KEY` → secrets 文件。
 */
export async function readDecisionConfig(): Promise<DecisionConfig> {
  const saved = await readSavedConfig()
  const block = (saved.decision ?? {}) as Record<string, unknown>

  const engine =
    asEnginePreference(envValue('A_DA_DECISION_ENGINE')) ??
    asEnginePreference(block.engine) ??
    'auto'

  const baseUrl =
    envValue('A_DA_DECISION_BASE_URL', 'PI_JEV_BASE_URL', 'TYPESAFE_BASE_URL') ||
    (typeof block.baseUrl === 'string' ? block.baseUrl.trim() : '')
  const apiKey =
    envValue('A_DA_DECISION_API_KEY', 'TYPESAFE_API_KEY') ||
    (typeof block.apiKey === 'string' ? block.apiKey.trim() : '') ||
    readSecretFile()

  const threshold = asPositiveNumber(block.threshold) ?? DEFAULT_DECISION_THRESHOLD
  const samples = Math.max(1, Math.min(9, Math.round(asPositiveNumber(block.samples) ?? DEFAULT_SAMPLES)))
  const sampleTimeoutMs =
    asPositiveNumber(block.sampleTimeoutMs) ?? DEFAULT_SAMPLE_TIMEOUT_MS

  return { engine, baseUrl, apiKey, threshold, samples, sampleTimeoutMs }
}

/** 截断 state 到上限，返回内容与是否发生截断。 */
export function clampState(raw: string): { text: string; truncated: boolean } {
  if (raw.length <= MAX_STATE_CHARS) return { text: raw, truncated: false }
  return {
    text: `${raw.slice(0, MAX_STATE_CHARS)}\n\n...(内容过长，已截断至 ${MAX_STATE_CHARS} 字符)`,
    truncated: true,
  }
}
