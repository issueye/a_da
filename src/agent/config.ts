/**
 * Where the model endpoint comes from.
 *
 * Precedence is environment first, then `~/.a-da/config.json`, so CI and shell
 * runs can override without touching the file the settings dialog writes.
 * `A_DA_CONFIG` moves that file, which is how the tests keep their hands off
 * the real one.
 */

import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { dirname, join } from 'node:path'

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
  return process.env.A_DA_CONFIG || join(homedir(), '.a-da', 'config.json')
}

/** The file alone. The dialog edits this, and it may differ from what a turn uses. */
export async function readSavedConfig(): Promise<Partial<ProviderConfig>> {
  try {
    const parsed = JSON.parse(await readFile(configPath(), 'utf8')) as Partial<ProviderConfig>
    return parsed && typeof parsed === 'object' ? parsed : {}
  } catch {
    return {}
  }
}

/** Merge into the file so a hand-written key that the dialog does not edit survives. */
export async function writeSavedConfig(patch: Partial<ProviderConfig>): Promise<void> {
  const path = configPath()
  const existing = await readSavedConfig()
  await mkdir(dirname(path), { recursive: true })
  await writeFile(path, `${JSON.stringify({ ...existing, ...patch }, null, 2)}\n`, 'utf8')
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
