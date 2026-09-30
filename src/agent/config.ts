/**
 * Where the model endpoint comes from.
 *
 * Precedence is environment first, then `~/.a-da/config.json`, so CI and shell
 * runs can override without touching the file the settings dialog writes.
 * `A_DA_CONFIG` moves that file, which is how the tests keep their hands off
 * the real one.
 */

import { existsSync, readFileSync } from 'node:fs'
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { getAppHome } from './home'
import { buildRequestHeaders } from './ai/headers'
import { APPEARANCES, type Appearance } from '../theme'

/**
 * `ProviderConfig` / `ProviderPreset` 的形状已搬到契约层 `src/shared/protocol`（协议设计 §7.1）：
 * 配置要跨进程交给 UI 与主机两边，形状属于契约。这里原样再导出，预设**数据表**留在本文件。
 */
import type { ProviderConfig, ProviderPreset } from '../shared/protocol'
export type { ProviderConfig, ProviderPreset }

export interface LlmConfig extends ProviderConfig {
  /** `env` or the config file path: which one the running turn will actually use. */
  source: string
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
  workspacePluginState?: Record<string, { disabledPlugins?: string[]; capabilities?: Record<string, unknown> }>
  /** 每个插件的配置块：pluginId → 键值对 */
  pluginConfig?: Record<string, Record<string, unknown>>
  /** 插件能力开关（见 docs/plugin-system-design.md §6.4.2） */
  pluginCapabilities?: Record<string, unknown>
  [key: string]: unknown
}

/**
 * 插件能力开关（设计文档 §6.4.2）。
 *
 * **默认全部开放**，用户可以逐项关掉——核心不替用户做安全判断，但要让他看得见后果
 * （关掉后用到的插件显示"受限"状态，不允许静默失效）。
 */
export interface PluginCapabilities {
  /** `beforeAgentStart` 可整体替换系统提示词 */
  allowSystemPromptReplace: boolean
  /** `afterAgentEnd.appendText` 可追加文本到本次会话 */
  allowTextRewrite: boolean
  /** `beforeThreadDelete` 可阻止删除（钩子本身属 M3） */
  allowThreadDeleteBlock: boolean
  /** `beforeCompaction` 可替换选择策略（钩子本身属 M3） */
  allowCompactionReplace: boolean
  /** 钩子在 plan 模式也生效 */
  allowPlanModeHooks: boolean
  /** 第三方扩展可注册钩子 */
  allowThirdPartyHooks: boolean
  /** 插件工具可覆盖同名核心内置工具 */
  allowBuiltinShadow: boolean
  /** 单个钩子的超时毫秒数；0 = 不限。超时**放行并记 trace**，不变成隐式拒绝 */
  hookTimeoutMs: number
}

export const DEFAULT_PLUGIN_CAPABILITIES: PluginCapabilities = {
  allowSystemPromptReplace: true,
  allowTextRewrite: true,
  allowThreadDeleteBlock: true,
  allowCompactionReplace: true,
  allowPlanModeHooks: true,
  allowThirdPartyHooks: true,
  allowBuiltinShadow: true,
  hookTimeoutMs: 500,
}

const CAPABILITY_BOOLEAN_KEYS = [
  'allowSystemPromptReplace',
  'allowTextRewrite',
  'allowThreadDeleteBlock',
  'allowCompactionReplace',
  'allowPlanModeHooks',
  'allowThirdPartyHooks',
  'allowBuiltinShadow',
] as const satisfies ReadonlyArray<keyof PluginCapabilities>

/**
 * 把配置里的原始值收敛成可用的开关值。
 *
 * 只接受真正的布尔与 >= 0 的数字：`"false"`、`1`、`"500"` 这类手写出来的值一律不收，
 * 并把键名记进 `invalid`——用户手改 config.json 写错了却毫无反馈，比写错本身更麻烦。
 */
function coerceCapabilities(raw: unknown): { values: Partial<PluginCapabilities>; invalid: string[] } {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return { values: {}, invalid: [] }
  const source = raw as Record<string, unknown>
  const values: Partial<PluginCapabilities> = {}
  const invalid: string[] = []

  for (const key of CAPABILITY_BOOLEAN_KEYS) {
    const value = source[key]
    if (value === undefined) continue
    if (typeof value === 'boolean') values[key] = value
    else invalid.push(key)
  }

  const timeout = source.hookTimeoutMs
  if (timeout !== undefined) {
    if (typeof timeout === 'number' && Number.isFinite(timeout) && timeout >= 0) {
      values.hookTimeoutMs = timeout
    } else {
      invalid.push('hookTimeoutMs')
    }
  }

  return { values, invalid }
}

export interface ResolvedPluginCapabilities {
  /** 全局默认叠加工作区覆盖之后的最终值 */
  capabilities: PluginCapabilities
  /** 某个插件的有效值（再叠加 `pluginCapabilities.overrides[pluginId]`） */
  forPlugin: (pluginId: string) => PluginCapabilities
  /** 取值不合法、被忽略的键（调用方应当把它说出来，别静默） */
  invalid: string[]
}

/**
 * 读插件能力开关。
 *
 * 三层叠加：内置默认（全开）→ `pluginCapabilities`（全局）→
 * `pluginCapabilities.overrides[pluginId]`（按插件）与
 * `workspacePluginState[workspace].capabilities`（按工作区）。
 */
export async function readPluginCapabilities(
  workspace?: string,
): Promise<ResolvedPluginCapabilities> {
  const cfg = await readSavedConfig()
  const global = coerceCapabilities(cfg.pluginCapabilities)
  const perWorkspace = coerceCapabilities(cfg.workspacePluginState?.[workspace ?? '']?.capabilities)

  const rawOverrides = (cfg.pluginCapabilities as Record<string, unknown> | undefined)?.overrides
  const overrideMap: Record<string, Partial<PluginCapabilities>> = {}
  const invalid = [...global.invalid.map((key) => `pluginCapabilities.${key}`)]
  if (rawOverrides && typeof rawOverrides === 'object' && !Array.isArray(rawOverrides)) {
    for (const [pluginId, raw] of Object.entries(rawOverrides as Record<string, unknown>)) {
      const coerced = coerceCapabilities(raw)
      overrideMap[pluginId] = coerced.values
      invalid.push(...coerced.invalid.map((key) => `pluginCapabilities.overrides.${pluginId}.${key}`))
    }
  }
  invalid.push(
    ...perWorkspace.invalid.map((key) => `workspacePluginState.${workspace}.capabilities.${key}`),
  )

  const capabilities: PluginCapabilities = {
    ...DEFAULT_PLUGIN_CAPABILITIES,
    ...global.values,
    ...perWorkspace.values,
  }

  return {
    capabilities,
    forPlugin: (pluginId: string) => ({ ...capabilities, ...(overrideMap[pluginId] ?? {}) }),
    invalid,
  }
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
 * 写入插件的密钥文件 `~/.a-da/secrets/<pluginId>_<key>`。
 *
 * 与 {@link readPluginSecret} 配对。刻意不走 config.json：那个文件会被复制、被截图、
 * 被提交，密钥不该出现在里面。空值视为"清除"。
 */
export async function savePluginSecret(
  pluginId: string,
  key: string,
  value: string,
): Promise<void> {
  const dir = join(getAppHome(), 'secrets')
  await mkdir(dir, { recursive: true })
  const path = join(dir, `${pluginId}_${key}`)
  if (value.trim() === '') {
    await rm(path, { force: true })
    return
  }
  await writeFile(path, `${value.trim()}
`, 'utf8')
}

/**
 * 写入某个插件的配置项（合并进 `pluginConfig[pluginId]`）。
 *
 * 与 {@link readPluginConfig} 配对：M1 先把读写两侧都落定，插件配置表单（M3）与
 * 脚本化写入都走这里——插件自己不该直接改 config.json，否则优先级（环境变量 >
 * 文件 > 默认值）就被绕过了。
 *
 * 密钥类（`type: 'secret'`）刻意不在这里写：它按约定放密钥文件。
 */
export function savePluginConfig(
  pluginId: string,
  values: Record<string, unknown>,
): Promise<void> {
  return mutateSavedConfig((current) => {
    const all = { ...(current.pluginConfig ?? {}) }
    all[pluginId] = { ...(all[pluginId] ?? {}), ...values }
    return { ...current, pluginConfig: all }
  })
}

/**
 * 写入插件能力开关（只写传进来的项，其余保持原样）。
 *
 * 刻意**只接受合法的布尔值与非负整数**：写进去一个 `"false"` 或 `-1` 会让读取侧的
 * `coerceCapabilities` 把它记成 invalid，用户看到的是"我明明改了却没生效"。
 * 这里在写入前就挡住，非法值由调用方（界面）负责提示。
 */
export function savePluginCapabilities(patch: Partial<PluginCapabilities>): Promise<void> {
  return mutateSavedConfig((current) => {
    const existing =
      current.pluginCapabilities && typeof current.pluginCapabilities === 'object'
        ? { ...(current.pluginCapabilities as Record<string, unknown>) }
        : {}
    for (const [key, value] of Object.entries(patch)) {
      if (value === undefined) continue
      if (typeof value === 'boolean') existing[key] = value
      else if (typeof value === 'number' && Number.isFinite(value) && value >= 0) existing[key] = value
    }
    return { ...current, pluginCapabilities: existing }
  })
}

/**
 * 读一次配置，返回一个「某插件在这个工作区是否停用」的判断函数。
 *
 * 扫描与加载会逐个插件问同一件事，而每次 {@link readPluginDisabled} 都要重读一遍
 * 配置文件——插件多起来就是几十次无谓的文件读取（还夹着 JSON.parse）。
 */
export async function createPluginDisabledResolver(
  workspace?: string,
): Promise<(pluginId: string) => boolean> {
  const cfg = await readSavedConfig()
  const global = new Set(Array.isArray(cfg.disabledPlugins) ? cfg.disabledPlugins : [])
  const wsList = workspace ? cfg.workspacePluginState?.[workspace]?.disabledPlugins : undefined
  const perWorkspace = new Set(Array.isArray(wsList) ? wsList : [])
  return (pluginId: string) => global.has(pluginId) || perWorkspace.has(pluginId)
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
  const disabled = await createPluginDisabledResolver(workspace)
  return disabled(pluginId)
}

/**
 * 写入某个插件的启停状态。
 *
 * **作用域与读取保持一致**：给了 `workspace` 就只动这个工作区那份，没给就动全局
 * 默认。刻意不做跨作用域删除——在工作区里"启用"只清掉工作区那条记录，全局那份
 * 仍然生效，否则一次项目内的勾选会悄悄改掉全局设置，而用户在别处看不到这个改动。
 *
 * 谁用全局、谁用工作区是个产品决定（插件管理页目前写全局，见
 * `ExtensionLoader.togglePlugin`）：M1 先把两侧的读写 API 都落定，界面上给用户
 * 选"仅本工作区"是 M3 的事。
 */
export function setPluginDisabled(
  pluginId: string,
  disabled: boolean,
  workspace?: string,
): Promise<void> {
  return mutateSavedConfig((current) => {
    if (!workspace) {
      const global = new Set(Array.isArray(current.disabledPlugins) ? current.disabledPlugins : [])
      if (disabled) global.add(pluginId)
      else global.delete(pluginId)
      return { ...current, disabledPlugins: [...global] }
    }

    const all = { ...(current.workspacePluginState ?? {}) }
    const existing = all[workspace]?.disabledPlugins
    const list = new Set(Array.isArray(existing) ? existing : [])
    if (disabled) list.add(pluginId)
    else list.delete(pluginId)
    all[workspace] = { ...all[workspace], disabledPlugins: [...list] }
    return { ...current, workspacePluginState: all }
  })
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
    headers: parseHeadersText(env.A_DA_HEADERS),
  }
}

/**
 * 解析「自定义请求头」文本。
 *
 * 接受两种分隔：`;` 与换行。UI 是单行输入框（只能打 `;`），而 `config.json`
 * 与环境变量里可以一行一个——两种写法都提供，用户不必为了多个头去猜格式。
 *
 * 每项按**第一个**冒号切分，所以值里可以带冒号（`X-Url: https://…` 不会被切坏）。
 * 分隔符本身不能出现在值里——这是单行文本格式的固有取舍，需要更复杂的值就写
 * `config.json`（那是个真正的对象）。
 */
export function parseHeadersText(raw: unknown): Record<string, string> | undefined {
  const text = typeof raw === 'string' ? raw : ''
  const result: Record<string, string> = {}
  for (const chunk of text.split(/[;\n\r]/)) {
    const entry = chunk.trim()
    if (!entry) continue
    const at = entry.indexOf(':')
    if (at <= 0) continue
    const name = entry.slice(0, at).trim()
    const value = entry.slice(at + 1).trim()
    if (name && value) result[name] = value
  }
  // 没解析出任何一项时返回 undefined 而不是空对象：`envOverrides()` 用
  // "值不是 undefined/空串"判断某个环境变量是否真在生效，空对象会让它误报。
  return Object.keys(result).length > 0 ? result : undefined
}

/**
 * 把自定义头渲染回单行文本（`Name: Value; Name2: Value2`）。
 *
 * 与 {@link parseHeadersText} 互为逆运算，供设置弹窗回填输入框。
 */
export function formatHeadersText(headers: Record<string, string> | undefined): string {
  if (!headers) return ''
  return Object.entries(headers)
    .filter(([name, value]) => name.trim() && value.trim())
    .map(([name, value]) => `${name.trim()}: ${value.trim()}`)
    .join('; ')
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
                : key === 'headers'
                  ? 'A_DA_HEADERS'
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
      headers: buildRequestHeaders(config),
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
