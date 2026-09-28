/**
 * Where the model endpoint comes from.
 *
 * Precedence is environment first, then `~/.a-da/config.json`, so CI and shell
 * runs can override without touching the file the settings dialog writes.
 * `A_DA_CONFIG` moves that file, which is how the tests keep their hands off
 * the real one.
 */

import { existsSync, readFileSync } from 'node:fs'
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { getAppHome } from './home'
import { APPEARANCES, type Appearance } from '../theme'

export interface ProviderConfig {
  baseUrl: string
  apiKey: string
  model: string
  /** 模型最大上下文窗口（Token），用于遥测比率统计等 */
  contextWindow?: number
  /** 是否支持多模态图片输入 */
  supportsImages?: boolean
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
  contextWindow?: number
  supportsImages?: boolean
}

/** Any OpenAI-compatible gateway works; these are the ones with a fixed URL. */
export const PROVIDER_PRESETS: ProviderPreset[] = [
  { id: 'openai', label: 'OpenAI', baseUrl: 'https://api.openai.com/v1', model: 'gpt-4o-mini', contextWindow: 128000, supportsImages: true },
  { id: 'deepseek', label: 'DeepSeek', baseUrl: 'https://api.deepseek.com/v1', model: 'deepseek-chat', contextWindow: 128000, supportsImages: false },
  { id: 'dashscope', label: '阿里云百炼', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen-plus', contextWindow: 128000, supportsImages: false },
  { id: 'moonshot', label: 'Moonshot', baseUrl: 'https://api.moonshot.cn/v1', model: 'kimi-k2-0905-preview', contextWindow: 128000, supportsImages: false },
  { id: 'ollama', label: 'Ollama（本地）', baseUrl: 'http://127.0.0.1:11434/v1', model: 'qwen2.5-coder', contextWindow: 128000, supportsImages: false },
  { id: 'custom', label: '自定义', baseUrl: '', model: '', contextWindow: 128000, supportsImages: false },
]

export function configPath(): string {
  return process.env.A_DA_CONFIG || join(getAppHome(), 'config.json')
}

/**
 * What is on disk, which is not only the provider block: the file also carries
 * the appearance choice and whatever a user hand-wrote. Hence the open index
 * signature — the dialog edits three keys, and everything else must survive it.
 */
export type SavedConfig = Partial<ProviderConfig> & {
  appearance?: unknown
  disabledPlugins?: string[]
  /** 按工作区覆盖的插件启停状态：workspace 路径 → 该工作区下额外禁用的插件 id */
  workspacePluginState?: Record<string, { disabledPlugins?: string[] }>
  /** 每个插件的配置块：pluginId → 键值对 */
  pluginConfig?: Record<string, Record<string, unknown>>
  /** 插件能力开关（见 docs/plugin-system-design.md §6.4.2） */
  pluginCapabilities?: Record<string, unknown>
  [key: string]: unknown
}

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

/** 读取已禁用的扩展插件 ID 列表 */
export async function readDisabledPlugins(): Promise<string[]> {
  const cfg = await readSavedConfig()
  return Array.isArray(cfg.disabledPlugins) ? cfg.disabledPlugins : []
}

/** 保存已禁用的扩展插件 ID 列表 */
export function saveDisabledPlugins(disabled: string[]): Promise<void> {
  return mutateSavedConfig((current) => ({ ...current, disabledPlugins: disabled }))
}

/**
 * 某个插件在当前工作区是否被停用。
 *
 * 解析顺序刻意与"全局默认 + 工作区覆盖"一致：全局 `disabledPlugins` 是默认值，
 * `workspacePluginState[workspace]` 在其之上追加（只做追加，不做取消——
 * 全局禁用的插件不该被某个工作区重新启用，那会让"我明明关了它"变成难查的问题）。
 */
export async function readPluginDisabled(
  pluginId: string,
  workspace?: string,
): Promise<boolean> {
  const cfg = await readSavedConfig()
  const global = Array.isArray(cfg.disabledPlugins) ? cfg.disabledPlugins : []
  if (global.includes(pluginId)) return true
  if (!workspace) return false
  const perWorkspace = cfg.workspacePluginState?.[workspace]?.disabledPlugins
  return Array.isArray(perWorkspace) ? perWorkspace.includes(pluginId) : false
}

/**
 * 把插件的配置键名转成环境变量片段：`baseUrl` → `BASE_URL`、`sampleTimeoutMs` → `SAMPLE_TIMEOUT_MS`。
 *
 * 必须按驼峰拆词，不能只做大写——否则 `baseUrl` 会变成 `BASEURL`，
 * 用户按直觉写的 `..._BASE_URL` 永远匹配不上，而且不报错，只是"设了没生效"。
 */
function envKeyOf(key: string): string {
  return key
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/[^A-Za-z0-9]+/g, '_')
    .toUpperCase()
}

/**
 * 读某个插件的配置块。
 *
 * 这是插件配置的**统一入口**——插件不再各自去翻 `config.json` 的顶层键，
 * 而是声明 `configSchema` 后从这里取。优先级：
 * 环境变量 `A_DA_PLUGIN_<ID>_<KEY>` > `config.json` 的 `pluginConfig[pluginId]` > 默认值。
 *
 * 环境变量覆盖的对象是 `defaults` 与文件块的**并集**（不是二者之一）：
 * 只配了默认值的键、以及只写在 config.json 里的键，都要能被环境变量覆盖——
 * 否则"设了环境变量却没生效"会变成很难查的问题。取值时沿用该键原有值的类型
 * （数字键转数字），避免把 `samples` 变成字符串 `"3"`。
 *
 * 密钥类的值（`type: 'secret'`）按约定放在 `~/.a-da/secrets/<pluginId>_<key>`，
 * 由插件自己用 {@link readPluginSecret} 读，不进 config.json。
 */
export async function readPluginConfig<T extends Record<string, unknown>>(
  pluginId: string,
  defaults: T = {} as T,
): Promise<T> {
  const cfg = await readSavedConfig()
  const block = cfg.pluginConfig?.[pluginId]
  const fromFile = block && typeof block === 'object' ? (block as Record<string, unknown>) : {}

  const merged: Record<string, unknown> = { ...defaults, ...fromFile }

  const prefix = `A_DA_PLUGIN_${envKeyOf(pluginId)}_`
  // 键名来自两处的并集：默认值里有、文件里有，都要能被环境变量覆盖。
  for (const key of new Set([...Object.keys(defaults), ...Object.keys(fromFile)])) {
    const env = process.env[`${prefix}${envKeyOf(key)}`]
    if (env === undefined || env.trim() === '') continue
    // 类型沿用该键在默认值里的声明；默认值没声明就看文件里那个值的类型。
    const sample = defaults[key] ?? fromFile[key]
    merged[key] = typeof sample === 'number' ? Number(env) : env.trim()
  }
  return merged as T
}

/**
 * 读插件的密钥文件 `~/.a-da/secrets/<pluginId>_<key>`。
 *
 * 单独一个函数而不是塞进 `readPluginConfig`：密钥的存储位置与普通配置不同
 * （刻意不进 config.json，因为它会被复制、被截图、被提交），
 * 调用方需要显式表达"这是敏感值"。
 */
export function readPluginSecret(pluginId: string, key: string): string {
  try {
    const path = join(getAppHome(), 'secrets', `${pluginId}_${key}`)
    if (!existsSync(path)) return ''
    return readFileSync(path, 'utf8').trim()
  } catch {
    return ''
  }
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
    contextWindow: env.A_DA_CONTEXT_WINDOW ? parseInt(env.A_DA_CONTEXT_WINDOW, 10) : undefined,
    supportsImages: env.A_DA_SUPPORTS_IMAGES ? env.A_DA_SUPPORTS_IMAGES === '1' || env.A_DA_SUPPORTS_IMAGES === 'true' : undefined,
  }
}

/** True when the environment is shadowing the file, which the dialog has to say. */
export function envOverrides(): string[] {
  const names: string[] = []
  for (const [key, value] of Object.entries(fromEnv())) {
    if (value !== undefined && value !== '') {
      names.push(
        key === 'apiKey'
          ? process.env.A_DA_API_KEY
            ? 'A_DA_API_KEY'
            : 'OPENAI_API_KEY'
          : key === 'baseUrl'
            ? process.env.A_DA_BASE_URL
              ? 'A_DA_BASE_URL'
              : 'OPENAI_BASE_URL'
            : key === 'model'
              ? process.env.A_DA_MODEL
                ? 'A_DA_MODEL'
                : 'OPENAI_MODEL'
              : key === 'contextWindow'
                ? 'A_DA_CONTEXT_WINDOW'
                : 'A_DA_SUPPORTS_IMAGES',
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
  const contextWindow =
    env.contextWindow !== undefined
      ? env.contextWindow
      : typeof file.contextWindow === 'number'
        ? file.contextWindow
        : undefined
  const supportsImages =
    env.supportsImages !== undefined
      ? env.supportsImages
      : Boolean(file.supportsImages)

  return {
    baseUrl,
    apiKey,
    model,
    contextWindow,
    supportsImages,
    source: env.apiKey ? 'env' : configPath(),
  }
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
