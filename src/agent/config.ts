/**
 * Where the model endpoint comes from.
 *
 * Precedence is environment first, then `~/.a-da/config.json`, so CI and shell
 * runs can override without touching the file the settings dialog writes.
 * `A_DA_CONFIG` moves that file, which is how the tests keep their hands off
 * the real one.
 */

import { readFileSync } from 'node:fs'
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { getAppHome } from './home'
import { APPEARANCES, type Appearance } from '../theme'

export interface ProviderConfig {
  baseUrl: string
  apiKey: string
  model: string
}

export interface LlmConfig extends ProviderConfig {
  /** `env` or the config file path: which one the running turn will actually use. */
  source: string
}

export interface ProviderPreset {
  id: string
  label: string
  baseUrl: string
  model: string
}

/** Any OpenAI-compatible gateway works; these are the ones with a fixed URL. */
export const PROVIDER_PRESETS: ProviderPreset[] = [
  { id: 'openai', label: 'OpenAI', baseUrl: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
  { id: 'deepseek', label: 'DeepSeek', baseUrl: 'https://api.deepseek.com/v1', model: 'deepseek-chat' },
  { id: 'dashscope', label: '阿里云百炼', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen-plus' },
  { id: 'moonshot', label: 'Moonshot', baseUrl: 'https://api.moonshot.cn/v1', model: 'kimi-k2-0905-preview' },
  { id: 'ollama', label: 'Ollama（本地）', baseUrl: 'http://127.0.0.1:11434/v1', model: 'qwen2.5-coder' },
  { id: 'custom', label: '自定义', baseUrl: '', model: '' },
]

export function configPath(): string {
  return process.env.A_DA_CONFIG || join(getAppHome(), 'config.json')
}

/**
 * What is on disk, which is not only the provider block: the file also carries
 * the appearance choice and whatever a user hand-wrote. Hence the open index
 * signature — the dialog edits three keys, and everything else must survive it.
 */
export type SavedConfig = Partial<ProviderConfig> & { appearance?: unknown; [key: string]: unknown }

/** The file alone. The dialog edits this, and it may differ from what a turn uses. */
export async function readSavedConfig(): Promise<SavedConfig> {
  try {
    const parsed = JSON.parse(await readFile(configPath(), 'utf8')) as SavedConfig
    return parsed && typeof parsed === 'object' ? parsed : {}
  } catch {
    return {}
  }
}

/**
 * Every write goes through one queue.
 *
 * Each write is a read-modify-write of a whole-file JSON document, so two that
 * overlap can lose one of the two keys — and the appearance toggle fires its save
 * without being awaited, so an appearance write and a provider save really do
 * overlap in normal use. Chaining them makes the pair sequential without making
 * callers await anything they did not already await.
 */
let writeQueue: Promise<void> = Promise.resolve()

function mutateSavedConfig(mutate: (current: SavedConfig) => SavedConfig): Promise<void> {
  const run = async (): Promise<void> => {
    const path = configPath()
    const next = mutate(await readSavedConfig())
    await mkdir(dirname(path), { recursive: true })
    // Written to a sibling and renamed: a reader (or a crash) between the two
    // steps otherwise sees a truncated file, and `readSavedConfig` would report
    // that as "no config" and the next write would drop every key in it.
    const temp = `${path}.${process.pid}.tmp`
    await writeFile(temp, `${JSON.stringify(next, null, 2)}\n`, 'utf8')
    await rename(temp, path)
  }
  // A failed write must not wedge the queue for every later one.
  const queued = writeQueue.then(run, run)
  writeQueue = queued.then(
    () => undefined,
    () => undefined,
  )
  return queued
}

/** Merge into the file so a hand-written key that the dialog does not edit survives. */
export function writeSavedConfig(patch: Partial<ProviderConfig>): Promise<void> {
  return mutateSavedConfig((current) => ({ ...current, ...patch }))
}

/**
 * The saved light/dark choice, or null when the user has never made one.
 *
 * It lives in the same file as the provider block rather than a second one: a
 * preference the user set is config, and the app should not grow a file per
 * toggle. The write merges, so a later provider save keeps this key.
 *
 * Synchronous on purpose. The window's first frame cannot wait on I/O, and
 * installing the palette after that frame means a dark-mode user sees a white
 * flash on every launch; this is read once during startup, where a small file is
 * cheap. It is not an async function with a sync twin, because only one of the
 * two would ever be called.
 */
export function readSavedAppearance(): Appearance | null {
  try {
    const parsed = JSON.parse(readFileSync(configPath(), 'utf8')) as SavedConfig
    return parsed && typeof parsed === 'object' && isAppearance(parsed.appearance)
      ? parsed.appearance
      : null
  } catch {
    return null
  }
}

export function writeSavedAppearance(next: Appearance): Promise<void> {
  return mutateSavedConfig((current) => ({ ...current, appearance: next }))
}

function isAppearance(value: unknown): value is Appearance {
  return typeof value === 'string' && (APPEARANCES as string[]).includes(value)
}

function fromEnv(): Partial<ProviderConfig> {
  const env = process.env
  return {
    apiKey: env.A_DA_API_KEY || env.OPENAI_API_KEY || '',
    baseUrl: env.A_DA_BASE_URL || env.OPENAI_BASE_URL || '',
    model: env.A_DA_MODEL || env.OPENAI_MODEL || '',
  }
}

/** True when the environment is shadowing the file, which the dialog has to say. */
export function envOverrides(): string[] {
  const names: string[] = []
  for (const [key, value] of Object.entries(fromEnv())) {
    if (value) {
      names.push(
        key === 'apiKey'
          ? process.env.A_DA_API_KEY
            ? 'A_DA_API_KEY'
            : 'OPENAI_API_KEY'
          : key === 'baseUrl'
            ? process.env.A_DA_BASE_URL
              ? 'A_DA_BASE_URL'
              : 'OPENAI_BASE_URL'
            : process.env.A_DA_MODEL
              ? 'A_DA_MODEL'
              : 'OPENAI_MODEL',
      )
    }
  }
  return names
}

/** What a turn will use: environment first, then the file. `null` means offline. */
export async function readLlmConfig(): Promise<LlmConfig | null> {
  const file = await readSavedConfig()
  const env = fromEnv()
  const apiKey = env.apiKey || file.apiKey || ''
  const baseUrl = (env.baseUrl || file.baseUrl || 'https://api.openai.com/v1').replace(/\/+$/, '')
  const model = env.model || file.model || ''
  if (!apiKey || !model) return null
  return { baseUrl, apiKey, model, source: env.apiKey ? 'env' : configPath() }
}

/** One cheap round trip, so 保存 can be told apart from 保存并可用. */
export async function testConnection(
  config: ProviderConfig,
): Promise<{ ok: boolean; detail: string }> {
  const baseUrl = config.baseUrl.replace(/\/+$/, '')
  if (!baseUrl) return { ok: false, detail: '请先填写接口地址' }
  if (!config.model) return { ok: false, detail: '请先填写模型名' }
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 20_000)
  try {
    const response = await fetch(`${baseUrl}/chat/completions`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        authorization: `Bearer ${config.apiKey}`,
      },
      body: JSON.stringify({
        model: config.model,
        messages: [{ role: 'user', content: 'ping' }],
        max_tokens: 1,
        stream: false,
      }),
      signal: controller.signal,
    })
    if (!response.ok) {
      const detail = await response.text().catch(() => '')
      return { ok: false, detail: `HTTP ${response.status}：${detail.slice(0, 200)}` }
    }
    return { ok: true, detail: `连接成功，${config.model} 可用` }
  } catch (error) {
    const message = (error as Error).name === 'AbortError' ? '请求超时（20s）' : (error as Error).message
    return { ok: false, detail: message }
  } finally {
    clearTimeout(timer)
  }
}
