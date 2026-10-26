/**
 * AGENT BASE 线协议数据结构 DTO (与 agent-proto/src/dto.rs 严格对齐)
 */

export type AgentMode = 'code' | 'plan' | 'create' | 'pm'
/** `full` = 完全访问（P1b）：不问 + 放开工作区边界与脚本网络（`~/.a-da` 红线除外）。 */
export type ApprovalMode = 'auto' | 'ask' | 'readonly' | 'full'
export type Effort = 'max' | 'high' | 'medium' | 'low'
export type ModelProtocol = 'openai_chat' | 'anthropic' | 'openai_responses'

export interface TokenUsage {
  promptTokens: number
  completionTokens: number
  totalTokens: number
  cachedTokens?: number
}

export interface ModelEntry {
  id: string
  name?: string
  contextWindow?: number
  maxOutputTokens?: number
  supportsImages?: boolean
}

export interface ProviderEntry {
  id: string
  name: string
  protocol: ModelProtocol
  baseUrl: string
  apiKey: string
  models: ModelEntry[]
  customHeaders?: Record<string, string>
  proxyUrl?: string
}

export interface ProviderConfig {
  baseUrl: string
  apiKey: string
  model: string
  protocol?: ModelProtocol
  name?: string
  contextWindow?: number
  maxOutputTokens?: number
  supportsImages?: boolean
  customHeaders?: Record<string, string>
  proxyUrl?: string
}

export interface ProviderPreset {
  id: string
  label: string
  baseUrl: string
  model: string
  contextWindow: number
  supportsImages: boolean
}

export type ItemKind = 'user' | 'thinking' | 'assistant' | 'toolCall' | 'tool' | 'question' | 'notice' | 'compact'
export type ItemRole = 'user' | 'assistant' | 'system' | 'tool'

export interface ToolCallItem {
  id: string
  name: string
  params: Record<string, unknown>
  result?: unknown
  error?: string
  status?: 'running' | 'done' | 'failed' | 'waiting_approval'
  durationMs?: number
}

export interface QuestionOption {
  /** 题干里给的短标识（如 `c1`）——**要在选项前面显示出来**，用户才能对上号/照着回话 */
  id?: string
  value: string
  label: string
  /** 选项的补充说明（模型常用来写风险/代价）；目前作 hover 提示用 */
  description?: string
}

/** 一次提问里的单个问题（`ask_user` 支持一次问多题）。 */
export interface QuestionItemData {
  /** 题号（如 `q1`）；答复按它对应回题目 */
  id?: string
  question: string
  options?: QuestionOption[]
  isMultiSelect?: boolean
  /** 是否允许文本补充（默认 true） */
  allowText?: boolean
}

export interface QuestionData {
  callId: string
  /** 单题路径就是题目本身；多题路径填首题（兼容不认识 `questions` 的旧渲染） */
  question: string
  options?: QuestionOption[]
  isMultiSelect?: boolean
  /** 一次问多题时的题目列表（界面逐题作答、一次提交） */
  questions?: QuestionItemData[]
}

export interface Item {
  id: string
  kind?: ItemKind
  role?: ItemRole
  text?: string
  at?: number
  createdAt?: number
  thinking?: string
  toolCalls?: ToolCallItem[]
  question?: QuestionData
  images?: string[]
  // 工具调用卡片打平字段
  callId?: string
  tool?: string
  name?: string
  title?: string
  args?: any
  details?: any
  state?: 'running' | 'done' | 'failed' | 'waiting_approval' | 'awaiting'
  status?: 'running' | 'done' | 'failed' | 'waiting_approval' | 'awaiting' | 'error' | 'denied' | 'timeout'
  result?: unknown
  output?: string
  patch?: string
  error?: string
  durationMs?: number
  startedAt?: number
  finishedAt?: number
  turnDurationMs?: number
  checkpointId?: string
  reverted?: boolean
  queued?: boolean
  level?: 'info' | 'error' | 'warn'
  // 思考与流式增量字段
  endedAt?: number
  streaming?: boolean
  usage?: TokenUsage
}

export interface ThreadStats {
  promptTokens: number
  completionTokens: number
  totalTokens: number
}

export interface QueuedItem {
  id: string
  threadId?: string
  text: string
  enqueuedAt?: number
  images?: string[]
  item?: Item
}

export interface Thread {
  id: string
  title: string
  workspace: string
  mode: AgentMode
  agentId?: string
  items: Item[]
  /**
   * 原始消息序列（模型面）。
   *
   * 界面的卡片渲染走 `items`，**这个字段只用于判断"这条会话是否已经开始"**——
   * 模式滑块能不能点、服务端允不允许改模式，都以"`items` 与 `messages` 都为空"为判据
   * （两处必须同源，否则会出现"滑块能点、点了报错"）。
   */
  messages?: unknown[]
  stats?: ThreadStats
  createdAt: number
  /**
   * 最近活动时刻（毫秒）：最后一条 item 的 `at`，空会话回退 `createdAt`。
   *
   * 快照为了压体积会裁剪非激活会话的 `items`，但**这个时刻照实给**——
   * 侧栏的"3 分钟前"与排序都靠它，别自己拿 `items` 推算。
   */
  updatedAt: number
  /**
   * 会话条目总数（**不受快照裁剪影响**）。
   *
   * 判断"这条会话有没有内容"要用它，不要用 `items.length`：
   * 快照只给激活 / 打开的 Tab / 运行中 / 子智能体会话带 `items`，其余是空数组。
   */
  itemCount?: number
  parentId?: string
  subagentId?: string
  isSubagent?: boolean
}

export interface WorkspaceSnapshot {
  project: string
  files: number
  dirs: number
  scanning: boolean
  entries: string[]
}

export interface ConfigSnapshot {
  model: string
  contextWindow: number
  maxOutputTokens?: number
  supportsImages: boolean
  approval: ApprovalMode
  effort: Effort
  mode: AgentMode
  /** 模型请求重试次数（默认 5；重试耗尽即断开本轮对话） */
  retryCount: number
}

export interface UiSnapshot {
  activeId: string
  openTabIds: string[]
  pendingDraft?: string
  debugOpen: boolean
  settingsOpen: boolean
  pluginsOpen: boolean
  changesOpen: boolean
  paletteOpen: boolean
  sidebarOpen: boolean
  searchOpen: boolean
}

export interface ClientSnapshot {
  /** 快照版本序列号（单调递增，M3-T2） */
  seq?: number
  activeThreadId: string
  threads: Thread[]
  workspaces: string[]
  activeWorkspace: string
  currentMode: AgentMode
  providerConfig: ProviderConfig
  approvalMode: ApprovalMode
  effort: Effort
  /** 模型请求重试次数（默认 5；重试耗尽即断开本轮对话） */
  retryCount: number
  running: boolean
  runningThreadIds: string[]
  /** 正在等待用户操作（审批 / 提问）的会话——用于"别的会话在等你"的全局提示 */
  waitingThreadIds?: string[]
  queue: QueuedItem[]
  providers?: ProviderEntry[]
  activeProviderId?: string
}

export interface ServerEventsCapability {
  granularity: string
  snapshotSeq: boolean
}

export interface ServerCapabilities {
  images: boolean
  rollback: boolean
  plugins: boolean
  hooks: boolean
  resync: boolean
  events: ServerEventsCapability
}

export interface HostInfo {
  pid: number
}

export interface InitializeResult {
  sessionId: string
  protocolVersion: string
  host: HostInfo
  capabilities: ServerCapabilities
}

export type PluginScope = 'builtin' | 'workspace' | 'global'
export type PluginStatus = 'ready' | 'not-ready' | 'incompatible' | 'broken' | 'conflict'

export interface PluginDiagnostic {
  pluginId: string
  level: 'info' | 'warn' | 'error'
  message: string
  hint?: string
}

export interface PluginToolInfo {
  name: string
  description: string
  parameters?: Record<string, unknown>
  isWrite: boolean
}

export interface PluginManifest {
  id: string
  name: string
  description: string
  version?: string
  author?: string
  scope?: PluginScope
}

export interface PluginConfigProperty {
  type: 'string' | 'number' | 'boolean' | 'secret'
  title: string
  description?: string
  default?: unknown
  required?: boolean
}

export interface PluginConfigSchema {
  properties: Record<string, PluginConfigProperty>
}

export interface LoadedPluginContributions {
  tools?: Array<{
    name: string
    label?: string
    description: string
    parameters?: Record<string, unknown>
  }>
  configSchema?: PluginConfigSchema
}

export interface LoadedPlugin {
  manifest: PluginManifest
  contributions: LoadedPluginContributions
  declarative: boolean
  status: PluginStatus
  diagnostics: PluginDiagnostic[]
  blockedTools?: string[]
}

/** 技能的来源（SP-45：`plugin` 是第三个来源，与插件本身的 scope 不是一套取值） */
export type SkillScope = 'builtin' | 'workspace' | 'global' | 'plugin'

export interface SkillSummary {
  id: string
  name: string
  description: string
  path: string
  scope: SkillScope
  enabled: boolean
  /** 这条技能是**哪个插件**带来的（SP-45）：只有插件技能才非空 */
  pluginName?: string
  pluginId?: string
  /** frontmatter 里认不出的键（SP-45）：非空 = 技能文件写错了键名，界面要提示 */
  unknownFrontmatterKeys?: string[]
  /**
   * 技能正文（线协议上本来就有；界面只用它算"启用技能的正文合计字节"——
   * 技能是**全量进提示词**的，这个数字直接对应每轮的固定开销）。
   */
  body?: string
}

export interface PromptItem {
  id: string
  name: string
  description: string
  scope: PluginScope
  content: string
  enabled: boolean
}

export interface PluginItem {
  plugin: LoadedPlugin
  id: string
  name: string
  fileName: string
  filePath: string
  scope: PluginScope
  enabled: boolean
  status: PluginStatus
  version?: string
  diagnostics: PluginDiagnostic[]
  tools: PluginToolInfo[]
  skills: SkillSummary[]
  prompts: PromptItem[]
  /** 该插件**真的注册上**的钩子点位（由装配期执行结果填入；目录扫描给的是空数组） */
  hooks: string[]
  isPackage?: boolean
  error?: string
  sizeBytes: number
  updatedAt: number
}

export interface BuiltinToolInfo {
  name: string
  label: string
  description: string
  isReadOnly: boolean
}

export interface PluginCapabilities {
  allowSystemPromptReplace: boolean
  allowTextRewrite: boolean
  allowThreadDeleteBlock: boolean
  allowCompactionReplace: boolean
  allowThirdPartyHooks: boolean
  allowBuiltinShadow: boolean
  hookTimeoutMs: number
}

export interface ResolvedPluginCapabilitiesDto {
  capabilities: PluginCapabilities
  invalid: string[]
  overrides: Record<string, Partial<PluginCapabilities>>
}

export interface PluginListResponse {
  plugins: PluginItem[]
  capabilities: ResolvedPluginCapabilitiesDto
  configs: Record<string, Record<string, unknown>>
  secrets: Record<string, boolean>
  diagnostics: PluginDiagnostic[]
}

// ── 插件市场（plugin-hub）──────────────────────────────────────────────

export interface MarketConfigView {
  baseUrl: string
  hasToken: boolean
  /** 只回显 Token 末 4 位；完整 Token 不回传界面 */
  tokenHint: string
}

export interface MarketPlugin {
  owner: string
  slug: string
  name: string
  description: string
  /** 优先取包内 metadata.author，缺失时退回 owner */
  author: string
  version?: string | null
  hasRelease: boolean
  tags: string[]
  downloads: number
  stars: number
  updatedAt: number
  /** 包根 index.ts 的 metadata（name/description/author/version 等） */
  metadata: Record<string, unknown>
}

export interface MarketSearchResult {
  baseUrl: string
  plugins: MarketPlugin[]
  total: number
}

export interface MarketInstallResult {
  ok: boolean
  plugin: {
    pluginId: string
    version: string
    indexPath: string
    replaced: boolean
  }
  /** 引擎侧是否真的重装配成功；null = 当前宿主没接重装配入口 */
  applied?: boolean | null
  report?: unknown
}

export interface MarketPublishResult {
  ok: boolean
  plugin: {
    owner: string
    slug: string
    version: string
    url: string
    /** false = 新建了插件仓库；true = 仓库已存在，本次只更新元数据并发布新版本 */
    existing: boolean
  }
}

export interface FileChange {
  path: string
  latestPatch?: string
  additions: number
  deletions: number
  editsCount: number
  reverted: boolean
  cardIds: string[]
}

export interface FsEntry {
  name: string
  path: string
  isDir?: boolean
  is_dir?: boolean
  size: number
  mtime: number
}

export interface FsRoot {
  path: string
  label: string
  kind: string
}

export interface FsListing {
  path: string
  parent: string | null
  entries?: FsEntry[]
  dirs?: FsEntry[]
  files?: FsEntry[]
  truncated?: boolean | { omitted: number } | null
  omitted?: number
}

export interface SubagentProfile {
  id: string
  name: string
  role: string
  description: string
  systemPrompt?: string
  allowedTools: string[]
  enabled: boolean
}

/** `diag` = P0 埋点的诊断遥测条目（点位在 `payload.point`、来源在 `payload.origin`）。 */
export type DebugKind = 'request' | 'response' | 'tools' | 'tool' | 'error' | 'info' | 'delta' | 'diag'

export interface DebugEntry {
  id: string
  kind: DebugKind
  at: number
  method?: string
  endpoint?: string
  payload?: any
  raw?: string
  error?: string
}

export interface ProductInfo {
  id: string
  name: string
  archetype: string
  persona?: string
}

export interface InitializeResult {
  sessionId: string
  protocolVersion: string
  host: { pid: number }
  capabilities: ServerCapabilities
  product?: ProductInfo
}
