/**
 * 协议的线上数据形状（wire DTO）。
 *
 * ## 依赖方向（重要）
 *
 * 这一层是**契约**，被 UI 与主机**两边**依赖，所以它**只能**依赖「纯类型模块」：
 * `agent/types.ts`、`agent/ai/types.ts`、`agent/subagents/types.ts`、`agent/skills/types.ts`、
 * `agent/prompts/types.ts`、`agent/plugins/types.ts`、`agent/stats/types.ts`、`agent/core/types.ts`。
 *
 * **绝不允许** import `agent/store.ts` / `agent/config.ts` / 任何含实现的模块——那会让契约反向
 * 依赖实现（协议设计 §7.1）。为此，原本寄生在实现里的几个类型（`ApprovalMode`、`Effort`、
 * `ProviderConfig`、`ProviderPreset`、`QueuedItem`）已经搬到本文件，实现侧改成从这里 import
 * （实现 → 契约，方向正确）。
 *
 * 所有 import 都是 `import type`：编译后不留任何运行时代码，因此不会成环。
 *
 * 设计依据：`docs/jsonrpc-protocol.md` §2。
 */

import type { AgentMode, AgentQuestion, DebugEntry, Item, Thread, ThreadStats } from '../../agent/types'
import type { TokenUsage } from '../../agent/ai/types'
import type { SubagentProfile, SubagentRunResult } from '../../agent/subagents/types'
import type { SkillSummary } from '../../agent/skills/types'
import type { PromptItem } from '../../agent/prompts/types'
import type { LoadedPlugin, PluginDiagnostic, PluginScope, PluginStatus } from '../../agent/plugins/types'
import type { ContextUsageSummary } from '../../agent/stats/types'
import type { AgentMessage } from '../../agent/core/types'

// ── 领域类型（直接复用，M0 只做转发；将来可搬进来） ──────────────────
export type { AgentMessage, AgentMode, AgentQuestion, DebugEntry, Item, Thread, ThreadStats }
export type { TokenUsage, SubagentProfile, SubagentRunResult, SkillSummary, PromptItem }
export type { LoadedPlugin, PluginDiagnostic, ContextUsageSummary }

// ── 配置类（原先住在 store.ts / config.ts，现归契约） ────────────────

/** 审批档位。`readonly` 的语义是"写操作必须经我确认"，插件不能在这一档放宽（协议 §3.5）。 */
export type ApprovalMode = 'auto' | 'ask' | 'readonly'

/** reasoning_effort 档位（映射到接口参数时 `max` 与 `high` 同值，见 store 的 EFFORT_VALUE）。 */
export type Effort = 'max' | 'high' | 'medium' | 'low'

/** 供应商配置（对话与「测试连接」共用同一份形状）。 */
export interface ProviderConfig {
  baseUrl: string
  apiKey: string
  model: string
  /** 模型最大上下文窗口（Token），用于遥测比率统计等 */
  contextWindow?: number
  /** 是否支持多模态图片输入 */
  supportsImages?: boolean
  /**
   * 自定义请求头，追加到默认头（`content-type` / `authorization`）之上。
   * **同名（大小写不敏感）会覆盖默认头**。
   */
  headers?: Record<string, string>
  /** 网络代理地址，例如 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080 */
  proxyUrl?: string
}

/** 内置供应商预设（只描述形状；预设数据表仍在 `agent/config.ts`）。 */
export interface ProviderPreset {
  id: string
  label: string
  baseUrl: string
  model: string
  contextWindow?: number
  supportsImages?: boolean
}

// ── 协议专用形状（线上才有，实现里不存在） ──────────────────────────

/** 排队中的一条指令（`evt.queue.updated` 的载荷）。 */
export interface QueuedItem {
  thread: Thread
  text: string
  images?: string[]
  item: Item
}

/** 工作区概况（`session.snapshot` / `evt.workspace.scanned`）。 */
export interface WorkspaceInfo {
  files: number
  dirs: number
  scanning: boolean
}

/** 列表用的会话摘要：不带 `items`/`messages` 两个大字段。 */
export type ThreadMeta = Omit<Thread, 'items' | 'messages'> & {
  itemCount: number
  running: boolean
}

/** 统一分页（大对象一律分页发，协议 §2.4）。 */
export interface Page<T> {
  items: T[]
  total: number
  cursor?: string
  hasMore: boolean
}

/** 流式文本增量：客户端按 `itemId` 追加（协议 §4）。 */
export interface MessageDelta {
  threadId: string
  itemId: string
  textDelta?: string
  thinkingDelta?: string
}

/** 工具卡增量更新：字段与 store 里的工具卡一一对应。 */
export interface CardPatch {
  threadId: string
  callId: string
  status?: 'awaiting' | 'running' | 'done' | 'error' | 'denied'
  outputDelta?: string
  /** 统一 diff 文本（改动审阅与回滚用） */
  patch?: string
  details?: Record<string, unknown>
  errorMessage?: string
}

/** 待审批（`req.approval.decide` 的载荷 + `approval.listPending` 的条目）。 */
export interface PendingApproval {
  callId: string
  threadId: string
  toolName: string
  args: unknown
  isWrite: boolean
  mode: ApprovalMode
  reason?: string
  at: number
}

/** 待回答（`req.question.ask` 的载荷 + `question.listPending` 的条目）。 */
export interface PendingQuestion {
  callId: string
  threadId: string
  question: AgentQuestion
}

/** 长命令的进度（`evt.progress`）。 */
export interface Progress {
  id: number | string
  done?: number
  total?: number
  label?: string
  phase?: string
}

/** 改动审阅的一行（原 `store.getThreadFileChanges()` 的内联结构，此处定型）。 */
export interface FileChange {
  path: string
  latestPatch: string
  additions: number
  deletions: number
  editsCount: number
  reverted: boolean
  cardIds: string[]
}

/** 会话文件改动汇总（`change.summary`）。 */
export interface ChangeSummary {
  count: number
  files: FileChange[]
}

/**
 * 主机产出的**可渲染快照**（`session.snapshot` 的载荷；M1 里也是粗粒度事件的载荷）。
 *
 * M1 的定位（`docs/ui-host-split-dev-plan.md` M1-3/M1-4）：**先粗后细**——一次给整份快照，
 * 客户端只"应用"，不读主机内存。等 M3 接了 WebSocket，再把高频路径拆成增量
 * （`evt.message.delta` / `evt.card.updated`），其余仍走快照。
 *
 * **注意 `ui` 这一段**：这些字段（焦点、标签、草稿、浮层开关）按协议 §9.1 属于**客户端本地**，
 * M1 只是**暂借**主机镜像一份（因为今天它们就住在 store 里）。M2 会把它们搬进
 * `src/ui/state/`，届时 `ui` 整段从快照里消失。`confirmModal` **不在这里面**——它带着回调，
 * 永远不可能上线，所以它从一开始就是客户端本地的。
 */
export interface ClientSnapshot {
  /** 会话（含 items/messages：M1 整份给，M3 起分页/增量） */
  threads: Thread[]
  /** 当前焦点会话（M2 会变成客户端本地；主机只保证这里的 id 有效） */
  activeThreadId: string
  /** 正在跑的会话（`ClientState.isThreadRunning` 的推导输入） */
  runningThreadIds: string[]
  /** 正在等子智能体唤醒的会话（`isThreadWaiting` 的推导输入） */
  waitingThreadIds: string[]
  queue: QueuedItem[]
  log: DebugEntry[]
  workspace: {
    project: string
    files: number
    dirs: number
    scanning: boolean
    entries: string[]
  }
  config: {
    model: string
    contextWindow: number
    supportsImages: boolean
    approval: ApprovalMode
    effort: Effort
    mode: AgentMode
  }
  /** 待回答的提问（`ask_user` 挂着没答的） */
  pendingQuestions: Array<{ callId: string; question: AgentQuestion }>
  /** 公共区路径：`labelFor` / `isPublic` 的推导输入 */
  publicWorkspace: string
  /** 主题（客户端偏好；M2 搬到客户端本地） */
  appearance: string
  /** 纯客户端状态的主机镜像（M2 搬走，见上方说明） */
  ui: {
    activeId: string
    openTabIds: string[]
    pendingDraft: string | null
    debugOpen: boolean
    settingsOpen: boolean
    pluginsOpen: boolean
    changesOpen: boolean
    paletteOpen: boolean
    sidebarOpen: boolean
    searchOpen: boolean
  }
}

/** 主机 → 客户端的粗粒度事件（M1 只有这一种；M3 起补齐协议 §4 的其余 topic）。 */
export interface SnapshotEvent {
  seq: number
  topic: 'evt.state.snapshot'
  payload: ClientSnapshot
}

// ── 插件 / 技能 / 提示词 / 子智能体档案：管理页的线上形状（协议 §3.8、§3.9） ──
//
// 这些形状原先寄生在实现模块里（`config.ts` 的 PluginCapabilities、`tools/loader.ts` 的
// PluginItem、`tools/registry.ts` 的 BuiltinToolInfo）。管理页要跨进程拿到它们，所以归位到契约；
// 实现侧改成从这里 import（依赖方向仍是「实现 → 契约」）。

/** 插件能力开关（默认值与解析逻辑在主机侧 `config.ts`）。 */
export interface PluginCapabilities {
  /** `beforeAgentStart` 可整体替换系统提示词 */
  allowSystemPromptReplace: boolean
  /** `afterAgentEnd.appendText` 可追加文本到本次会话 */
  allowTextRewrite: boolean
  /** `beforeThreadDelete` 可阻止删除 */
  allowThreadDeleteBlock: boolean
  /** `beforeCompaction` 可替换选择策略 */
  allowCompactionReplace: boolean
  /** 钩子在 plan 模式也生效 */
  allowPlanModeHooks: boolean
  /** 第三方扩展可注册钩子 */
  allowThirdPartyHooks: boolean
  /** 插件工具可覆盖同名核心内置工具 */
  allowBuiltinShadow: boolean
  /** 单个钩子的超时毫秒数；0 = 不限。超时**放行并记 trace** */
  hookTimeoutMs: number
}

/** 一个插件工具的展示信息（`PluginItem.tools` 的元素）。 */
export interface PluginToolInfo {
  name: string
  description: string
  parameters?: Record<string, unknown>
  isWrite: boolean
}

/**
 * 插件管理页的一行（`plugin.list` 的元素）。
 *
 * `plugin` 是权威来源，其余平铺字段由它派生（`skills`/`prompts` 例外：它们来自技能与提示词
 * 管理器的扫描结果，是"归纳后的条目"，不是插件自己声明的原文）。
 */
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
  /** 插件包内包含的技能列表 */
  skills: SkillSummary[]
  /** 插件包内包含的提示词列表 */
  prompts: PromptItem[]
  /** 是否为复合插件包目录（包含 skills/、prompts/ 或独立子目录） */
  isPackage?: boolean
  /** 加载失败原文与 error 级诊断的汇总 */
  error?: string
  sizeBytes: number
  updatedAt: number
}

/** 核心内置工具的展示目录条目（`plugin.builtinCatalog` 的元素）。 */
export interface BuiltinToolInfo {
  name: string
  label: string
  description: string
  isReadOnly: boolean
}

/** 解析后的能力开关（`plugin.list` 顺带返回，省一次往返）。 */
export interface ResolvedPluginCapabilitiesDto {
  capabilities: PluginCapabilities
  /** 取值不合法、被忽略的键（界面要如实说出来） */
  invalid: string[]
  /** 按插件覆盖的那一层 */
  overrides: Record<string, Partial<PluginCapabilities>>
}

// ── 文件服务（协议 §3.14）：界面自己的选择器靠这几个形状浏览主机文件系统 ──

/** 一个目录项（**只有元数据**，不含内容）。 */
export interface FsEntry {
  name: string
  /** 绝对路径（已规范化） */
  path: string
  kind: 'dir' | 'file'
  sizeBytes?: number
  mtimeMs?: number
}

/** 一次列目录的结果。截断与隐藏都**如实报告**，界面必须说出来。 */
export interface FsListing {
  path: string
  /** 上一级；到根时为 null */
  parent: string | null
  entries: FsEntry[]
  truncated: boolean
  /** 被截断掉多少条 */
  omitted: number
  /** 因"隐藏"被省掉多少条（仅当没要求显示隐藏项时统计） */
  hiddenCount: number
}

/** 可跳转的根：驱动器 / 文件系统根、主目录、当前工作区。 */
export interface FsRoot {
  path: string
  label: string
  kind: 'drive' | 'home' | 'workspace'
}
