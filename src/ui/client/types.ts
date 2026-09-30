/**
 * 客户端接口（UI 只认这一份契约）。
 *
 * ## 三条通道，各管一类东西
 *
 * - **`state`**：一切**读**（协议 §9.1 的 B 组数据 + C 组的读侧 + 同步读助手）。
 *   M0 里它就是 store 那个活对象（`docs/ui-host-split-dev-plan.md` 的 M0 妥协）；
 *   **M1 起换成复制视图**，届时 UI 代码不用再动。
 * - **`ui`**：**纯客户端动作**（协议 §9.1 的 C 组写侧：标签、弹窗开合、草稿、确认框…）。
 *   它们**永远不进协议**；M0 临时寄存在 store 上，M2 搬到 `src/ui/state/`。
 * - **`request`**：**协议命令**（A 组），异步、带类型；M3 起走 WebSocket。
 *
 * 规则一句话：**读走 `state`，客户端动作走 `ui`，协议命令走 `request`**。
 *
 * 依据：`docs/jsonrpc-protocol.md` §9.1 的分组、`docs/ui-host-split-dev-plan.md` M0。
 */

import type {
  AgentMode,
  AgentQuestion,
  ApprovalMode,
  DebugEntry,
  Effort,
  FileChange,
  ParamsOf,
  ProtocolMethod,
  QueuedItem,
  ResultOf,
  Thread,
  ThreadStats,
  WorkspaceInfo,
} from '../../shared/protocol'
import type { Appearance } from '../../theme'

/**
 * 确认框的选项。**纯客户端状态**：`onConfirm` 是本地回调，不可能上协议（协议 §9.3）。
 * 形状与 store 里那份一致，所以 M0 可以直接把 store 当 `ClientState` 用。
 */
export interface ConfirmOptions {
  title: string
  message: string
  confirmText?: string
  cancelText?: string
  onConfirm: () => void
}

/**
 * 与主机的连接状态（协议 §1.5 要求"可理解 + 可重连"）。
 *
 * 为什么放进 `ClientState`：这是界面**要渲染**的东西，而界面读状态只有 `client.state` 一条通道。
 * 进程内传输永远是 `connected`；WebSocket 传输在断开/重连时改它。
 */
export interface ClientConnectionState {
  status: 'connected' | 'connecting' | 'disconnected'
  /** 已经尝试重连的次数（`connected` 时归零）。界面用它显示"第 N 次重连"。 */
  attempts: number
  /** 断开的原因（主机的关闭原因或传输错误），给界面显示一句人话。 */
  reason?: string
}

/** UI 能读的全部状态。M0 = store 活对象；M1 = 复制视图快照。 */
export interface ClientState {
  /** 与主机的连接状态（进程内恒为 connected） */
  readonly connection: ClientConnectionState

  // ── B 组：协议里的数据（快照 / 事件流提供） ──
  readonly threads: Thread[]
  readonly active: Thread
  readonly queue: QueuedItem[]
  readonly log: DebugEntry[]
  readonly workspaceInfo: WorkspaceInfo
  readonly entries: string[]
  readonly currentModel: string
  readonly contextWindow: number
  readonly supportsImages: boolean
  readonly approval: ApprovalMode
  readonly effort: Effort
  readonly activeThreadStats: ThreadStats
  readonly pendingAnswerQuestions: Array<{ callId: string; question: AgentQuestion }>

  // ── C 组读侧：纯客户端状态（M2 移到 src/ui/state/） ──
  readonly activeId: string
  readonly project: string
  readonly projects: string[]
  readonly projectThreads: Thread[]
  readonly openTabs: Thread[]
  readonly mode: AgentMode
  readonly running: boolean
  readonly pendingDraft: string | null
  readonly appearance: Appearance
  readonly debugOpen: boolean
  readonly settingsOpen: boolean
  readonly pluginsOpen: boolean
  readonly changesOpen: boolean
  readonly paletteOpen: boolean
  readonly sidebarOpen: boolean
  readonly searchOpen: boolean
  readonly confirmModal: ConfirmOptions | null

  // ── 同步读助手（纯查询；将来属于复制视图的派生值） ──
  isThreadRunning(threadId: string): boolean
  isThreadWaiting(threadId: string): boolean
  labelFor(workspace: string): string
  isPublic(workspace: string): boolean
  getThreadChangeCount(threadId: string): number
  getThreadFileChanges(threadId: string): FileChange[]
}

/** 纯客户端动作：改的都是"界面怎么看"，与主机数据无关。 */
export interface UiActions {  openTab(threadId: string): void
  closeTab(threadId: string): void
  setChangesOpen(open: boolean): void
  setPaletteOpen(open: boolean): void
  setPlugins(open: boolean): void
  setSettings(open: boolean): void
  setSearchOpen(open: boolean): void
  toggleSidebar(): void
  toggleAppearance(): void
  toggleDebug(): void
  applyPromptToComposer(content: string): void
  clearPendingDraft(): void
  showConfirm(options: ConfirmOptions): void
  closeConfirm(): void
}

/** UI 依赖的唯一接口。 */
export interface AgentClient {
  /** 读 */
  readonly state: ClientState
  /** 纯客户端动作 */
  readonly ui: UiActions
  /** 协议命令（A 组）：异步、带类型 */
  request<M extends ProtocolMethod>(method: M, params: ParamsOf<M>): Promise<ResultOf<M>>
  /** 订阅"状态变了"（M0 包装 store.subscribe；M1 起由复制视图驱动，M3 起由事件流驱动） */
  subscribe(listener: () => void): () => void
  /**
   * 立刻发布一次状态快照，绕过合帧窗口。
   *
   * 存在的理由有两个：**测试**里大量直接改状态造数据（不走命令、不通知），需要一次显式提交；
   * **诊断**时可以手动催一次。它不是协议方法——M3 的 WebSocket 实现里它可以变成
   * "向主机要一次快照"，也可以退化为 no-op。
   */
  refreshState(): void
}
