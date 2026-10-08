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

/**
 * 轻提示（toast）的语义级别。
 *
 * 三个级别对应三种**该怎么办**，不是三种颜色：`error` 要用户去看/重试，
 * `warn` 是"做成了但有个副作用你得知道"（例如插件停用是**全局**的、关掉某个
 * 能力开关会让别的插件受限），`success` 只是确认事情发生了。
 */
export type ToastLevel = 'success' | 'warn' | 'error'

/**
 * 一条轻提示。
 *
 * **纯客户端状态**：它是"界面怎么看"，不是主机数据（`evt.notify` 那条协议通道
 * 至今是空的，见 `docs/jsonrpc-protocol.md`）。放客户端还有一个硬理由：主机在
 * 跑长任务时提示要立刻可见，等一趟往返就成了"点了半天才弹出来"。
 */
export interface ToastItem {
  id: string
  level: ToastLevel
  /** 一句话说清发生了什么，别写成错误码 */
  message: string
  /** 可选补充：为什么/下一步。多行会换行显示 */
  detail?: string
  /** 毫秒；0 = 不自动消失（错误默认不自动消失，见 `ToastHost`） */
  durationMs: number
}

/**
 * 文件选择请求（**纯客户端状态**：`onPicked` 是本地回调，不可能上协议）。
 *
 * 为什么放在客户端状态里而不是让调用方各自渲染：选择器是**窗口级**的模态层
 * （和确认框同一个道理）——挂在下拉/弹层内部会在弹层关闭时被一起卸载。
 * 调用方只需要 `client.ui.pickFiles({...})`，由 `AgentWindow` 统一渲染一份。
 */
export interface FilePickerRequest {
  mode: 'directory' | 'files'
  title?: string
  startPath?: string
  /** 只允许选这些文件（按文件名匹配）；`files` 模式生效 */
  accept?: RegExp
  onPicked: (paths: string[]) => void
}

/** UI 能读的全部状态。M0 = store 活对象；M1 = 复制视图快照。 */
export interface ClientState {
  /** 与主机的连接状态（进程内恒为 connected） */
  readonly connection: ClientConnectionState

  /** 当前要显示的文件选择器（null = 不显示） */
  readonly filePicker: FilePickerRequest | null

  /**
   * 当前挂着的轻提示（**客户端本地**，与确认框、文件选择器同一类）。
   *
   * 为什么不走快照：toast 是"这一刻给这个用户看的一句话"，多客户端下各自弹各自的
   * 才是对的；而且主机在某客户端上弹提示这件事本来就没有语义。等真的要支持
   * "主机推提示"（`evt.notify`）时，再加一条**入队**通道，而不是把它变成快照字段
   * ——快照字段会被后到的快照整体覆盖，正在显示的提示会闪掉。
   */
  readonly toasts: ToastItem[]

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
  /**
   * 弹一条轻提示。`level` 不给时按 `success`。
   *
   * **别用它报告失败后还能继续的事**：失败要么留在原地（页面里的错误条），要么
   * 用 `error` 级别（默认不自动消失）。它也不该被用来问问题——那用 `showConfirm`。
   */
  notify(toast: { level?: ToastLevel; message: string; detail?: string; durationMs?: number }): void
  /** 关掉一条（点 × 或自动消失都走它）。 */
  dismissToast(id: string): void
  /** 打开应用内的文件/目录选择器（数据来自主机 `fs.*`），替代原生选择窗口 */
  pickFiles(request: FilePickerRequest): void
  closeFilePicker(): void
  /**
   * 切换焦点会话。
   *
   * **客户端即时生效 + 尽力通知主机**：协议 §11 定案把"焦点"划给客户端，所以点一下会话
   * 不该等主机回话——尤其主机在跑长任务时，等它会变成"点了没反应，30 秒后弹一个超时"。
   * 通知失败只记日志（主机那边的焦点只影响队列归属与插件按工作区加载，界面不受影响）。
   */
  activateThread(threadId: string): void
  /** 切换当前项目（跟着切到该项目最新的会话）；同样即时生效、尽力通知 */
  activateProject(workspace: string): void
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
