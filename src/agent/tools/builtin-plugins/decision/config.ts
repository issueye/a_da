/**
 * 决策插件的配置解析。
 *
 * 配置读取已统一到核心的 `readPluginConfig`（见 docs/plugin-system-design.md §5.2）：
 * 插件声明需求，核心负责优先级与环境变量映射。优先级：
 * `A_DA_PLUGIN_DECISION_<KEY>` 环境变量 > `config.json` 的 `pluginConfig.decision` > 默认值。
 *
 * 密钥放 `~/.a-da/secrets/decision_api_key`（对齐 pi-jev 的
 * `~/.pi/agent/secrets/typesafe_api_key`），不进 config.json。
 */

import { readPluginConfig, readPluginSecret } from '../../../config'
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

/** 插件 id：同时用于 `pluginConfig` 键名与环境变量前缀 `A_DA_PLUGIN_DECISION_*`。 */
export const DECISION_PLUGIN_ID = 'decision'

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
 * 环境变量整体优先于文件（由 `readPluginConfig` 实现）；文件里的 `pluginConfig.decision`
 * 块缺失时各项回落到默认值。密钥顺序：`A_DA_PLUGIN_DECISION_API_KEY` 环境变量 >
 * `pluginConfig.decision.apiKey` > secrets 文件。
 */
export async function readDecisionConfig(): Promise<DecisionConfig> {
  const block = await readPluginConfig<Record<string, unknown>>(DECISION_PLUGIN_ID)

  const engine = asEnginePreference(block.engine) ?? 'auto'

  const baseUrl = typeof block.baseUrl === 'string' ? block.baseUrl.trim() : ''
  const apiKey =
    (typeof block.apiKey === 'string' ? block.apiKey.trim() : '') ||
    readPluginSecret(DECISION_PLUGIN_ID, 'api_key')

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
