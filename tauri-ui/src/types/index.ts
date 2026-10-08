/**
 * 前端核心类型定义，与 a_da JSON-RPC 2.0 后端协议保持一致。
 */

export type AgentMode = 'code' | 'plan' | 'create'
export type ApprovalMode = 'auto' | 'ask' | 'readonly'
export type Effort = 'max' | 'high' | 'medium' | 'low'
export type ModelProtocol = 'openai_chat' | 'anthropic' | 'openai_responses'

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
  usage?: {
    promptTokens: number
    completionTokens: number
    totalTokens: number
    cachedTokens?: number
  }
}

export interface ThreadStats {
  promptTokens: number
  completionTokens: number
  totalTokens: number
}

export interface TodoStep {
  id?: string
  title: string
  status: 'pending' | 'in_progress' | 'completed'
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
  items: Item[]
  stats?: ThreadStats
  createdAt: number
  updatedAt: number
  parentId?: string
  subagentId?: string
  isSubagent?: boolean
}

export interface ClientSnapshot {
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

// ── 插件系统契约类型（与原有系统与 agent_core 保持 1:1 对齐） ──

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

export type ContextSource =
  | 'messages'
  | 'system_prompt'
  | 'skills'
  | 'tools'
  | 'completion'

export interface ContextBreakdownItem {
  source: ContextSource
  label: string
  color: string
  chars: number
  estimatedTokens: number
  percent: number
}

export interface ContextUsageSummary {
  /** 当前上下文占用的 Token 数（优先基于真实返回的 promptTokens，或估算） */
  usedTokens: number
  /** 当前模型支持的最大上下文窗口大小（如 128,000） */
  maxTokens: number
  /** 上下文使用率（0 ~ 1） */
  percent: number
  /** 格式化后的简短概括（如 "2.7k/128k (2.1%)"） */
  formattedSummary: string
  /** 缓存命中率（0 ~ 1，未命中或无缓存时为 null） */
  cacheHitRate: number | null
  /** 缓存命中的 Token 数 */
  cachedTokens: number
  /** 上下文各组成部分分解列表（按占比从大到小排序） */
  breakdown: ContextBreakdownItem[]
}

// ── 改动审查与 Diff 类型 ──

export interface FileChange {
  path: string
  latestPatch?: string
  additions: number
  deletions: number
  editsCount: number
  reverted: boolean
  cardIds: string[]
}

// ── 文件系统浏览器类型 (基于 fs.* 协议) ──

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

// ── 子智能体特化 Profile ──

export interface SubagentProfile {
  id: string
  name: string
  role: string
  description: string
  systemPrompt?: string
  allowedTools: string[]
  enabled: boolean
}

// ── 底层通信调试日志条目 ──

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

// ── 全局轻提示 Toast 类型 ──

export type ToastLevel = 'info' | 'success' | 'warn' | 'error'

export interface ToastItem {
  id: string
  level: ToastLevel
  message: string
  detail?: string
  durationMs?: number
  action?: {
    label: string
    onClick: () => void
  }
}

