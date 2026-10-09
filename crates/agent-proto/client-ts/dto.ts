/**
 * AGENT BASE 线协议数据结构 DTO (与 agent-proto/src/dto.rs 严格对齐)
 */

export type AgentMode = 'code' | 'plan' | 'create' | 'pm'
export type ApprovalMode = 'auto' | 'ask' | 'readonly'
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
  value: string
  label: string
}

export interface QuestionData {
  callId: string
  question: string
  options?: QuestionOption[]
  isMultiSelect?: boolean
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
  status?: 'running' | 'done' | 'failed' | 'waiting_approval' | 'awaiting' | 'error' | 'denied'
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
  stats?: ThreadStats
  createdAt: number
  updatedAt: number
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
  running: boolean
  runningThreadIds: string[]
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

export interface SkillSummary {
  id: string
  name: string
  description: string
  path: string
  scope: PluginScope
  enabled: boolean
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
  allowPlanModeHooks: boolean
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

export type DebugKind = 'request' | 'response' | 'tools' | 'tool' | 'error' | 'info' | 'delta'

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
