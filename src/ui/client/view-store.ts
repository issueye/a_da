/**
 * 复制视图（M1-1 / M1-4 / M1-5 / M1-6 / M1-3 的客户端一半）。
 *
 * ## 它的职责
 *
 * **只应用主机给的快照，不读主机的内存**。M0 结束时 `client.state` 还是那个活对象——
 * 界面能工作只是因为"同进程、读的就是最新值"。M1 把它换成一份**由主机事件驱动重建**的
 * 复制视图：`client.state` = 最近一次快照 + 客户端本地字段 + 从快照数据本地推导出来的助手。
 * 这样 M3 把来源换成 WebSocket 时，UI 一行都不用改。
 *
 * ## 三个数据来源，边界清楚
 *
 * 1. **主机快照**（`SnapshotSource.snapshot()`）：会话、队列、日志、工作区、配置、待答提问；
 * 2. **客户端本地**（`local`）：`confirmModal`——它带着回调，永远不可能上线；
 * 3. **本地推导**（`derive.ts`）：`isThreadRunning` / `labelFor` / `getThreadFileChanges` 这类
 *    纯函数，输入只有快照数据。
 *
 * ## 合帧（coalescing）
 *
 * 来源可能在一帧内通知多次（流式尤其）。`coalesceMs > 0` 时合并成一次重建 + 一次通知；
 * `flush()` 可强制立即应用。**进程内默认 0**（没有带宽要省，且"点一下立即断言绘制"的
 * 既有用例依赖同步通知）；WebSocket 源用 16–33ms（协议 §7.1）。
 */

import type { ClientSnapshot } from '../../shared/protocol'
import { computeThreadStats } from '../../agent/types'
import type { ClientConnectionState, ClientState, ConfirmOptions, FilePickerRequest } from './types'
import {
  deriveIsPublic,
  deriveLabelFor,
  deriveThreadChangeCount,
  deriveThreadFileChanges,
} from './derive'

/** 快照来源：主机侧替身（进程内）或 WebSocket 客户端。 */
export interface SnapshotSource {
  /** 当前快照。 */
  snapshot(): ClientSnapshot
  /** 订阅"有新快照"。 */
  subscribe(listener: (snapshot: ClientSnapshot) => void): () => void
  /**
   * 可选：廉价结构判据。进程内替身提供它，用来兜住"改了状态却没通知"
   * （这类静默失效会让界面停住而不报错）。WebSocket 源不提供——那时只有 `seq` 与重连兜底。
   */
  stalenessKeys?(): unknown[]
}

export interface ViewStoreOptions {
  /** 合帧窗口（ms）。0 = 同步应用 + 同步通知。默认 0。 */
  coalesceMs?: number
}

export interface ViewStore {
  /** 当前快照。同一变更周期内重复调用返回**同一个对象**（引用稳定）。 */
  getState(): ClientState
  /** 订阅"快照已更新"（已合帧）。 */
  subscribe(listener: () => void): () => void
  /** 立刻应用一次并同步通知（绕过合帧窗口）。 */
  flush(): void
  /** 客户端本地：弹确认框 / 关确认框（`confirmModal` 属客户端本地，见文件头）。 */
  showConfirm(options: ConfirmOptions): void
  closeConfirm(): void
  /** 更新"与主机的连接状态"（传输层调；进程内不用调）。 */
  setConnection(next: ClientConnectionState): void
  /** 打开/关闭应用内的文件选择器（客户端本地）。 */
  pickFiles(request: FilePickerRequest): void
  closeFilePicker(): void
  /** 已应用次数（每次重建 +1）；测试与诊断用。 */
  readonly publishes: number
}

function sameKeys(a: unknown[], b: unknown[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false
  return true
}

export function createViewStore(source: SnapshotSource, options: ViewStoreOptions = {}): ViewStore {
  const coalesceMs = options.coalesceMs ?? 0

  /** 客户端本地字段（不上快照：`confirmModal` 带回调）。 */
  let confirmModal: ConfirmOptions | null = null

  /** 与主机的连接状态（本地字段；进程内恒为 connected，WebSocket 会改它）。 */
  let connection: ClientConnectionState = { status: 'connected', attempts: 0 }

  /** 当前的文件选择请求（本地字段；窗口级模态层，与确认框同一套做法）。 */
  let filePicker: FilePickerRequest | null = null

  let snapshot: ClientSnapshot | null = null
  let state: ClientState | null = null
  let keys: unknown[] = []
  let publishes = 0
  let timer: ReturnType<typeof setTimeout> | null = null
  const listeners = new Set<() => void>()

  /** 把主机快照 + 客户端本地字段 + 本地推导合成为 UI 读的那份状态。 */
  function compose(next: ClientSnapshot): ClientState {
    const running = new Set(next.runningThreadIds)
    const waiting = new Set(next.waitingThreadIds)
    return {
      // ── 连接状态与文件选择请求（客户端本地）──
      connection,
      filePicker,

      // ── 主机数据 ──
      threads: next.threads,
      active: activeThreadOf(next),
      queue: next.queue,
      log: next.log,
      workspaceInfo: {
        files: next.workspace.files,
        dirs: next.workspace.dirs,
        scanning: next.workspace.scanning,
      },
      entries: next.workspace.entries,
      currentModel: next.config.model,
      contextWindow: next.config.contextWindow,
      supportsImages: next.config.supportsImages,
      approval: next.config.approval,
      effort: next.config.effort,
      activeThreadStats: computeThreadStats(activeThreadOf(next)),
      pendingAnswerQuestions: next.pendingQuestions,

      // ── 客户端本地镜像（M2 会从快照里搬走）──
      activeId: next.ui.activeId,
      project: next.workspace.project,
      projects: projectsFrom(next),
      projectThreads: next.threads.filter((thread) => thread.workspace === next.workspace.project),
      openTabs: openTabsFrom(next),
      mode: next.config.mode,
      running: running.has(next.ui.activeId),
      pendingDraft: next.ui.pendingDraft,
      // 线上是字符串，客户端做一次取值校验（不信任外部输入的形状）
      appearance: next.appearance === 'light' ? 'light' : 'dark',
      debugOpen: next.ui.debugOpen,
      settingsOpen: next.ui.settingsOpen,
      pluginsOpen: next.ui.pluginsOpen,
      changesOpen: next.ui.changesOpen,
      paletteOpen: next.ui.paletteOpen,
      sidebarOpen: next.ui.sidebarOpen,
      searchOpen: next.ui.searchOpen,
      confirmModal,

      // ── 本地推导（纯函数，输入只有快照数据）──
      isThreadRunning: (threadId) => running.has(threadId),
      isThreadWaiting: (threadId) => waiting.has(threadId),
      labelFor: (workspace) => deriveLabelFor(workspace, next.publicWorkspace),
      isPublic: (workspace) => deriveIsPublic(workspace, next.publicWorkspace),
      getThreadChangeCount: (threadId) => {
        const thread = next.threads.find((t) => t.id === threadId)
        return thread ? deriveThreadChangeCount(thread.items) : 0
      },
      getThreadFileChanges: (threadId) => {
        const thread = next.threads.find((t) => t.id === threadId)
        return thread ? deriveThreadFileChanges(thread.items) : []
      },
    }
  }

  function applySnapshot(next: ClientSnapshot): void {
    snapshot = next
    state = compose(next)
    keys = source.stalenessKeys ? source.stalenessKeys() : []
    publishes += 1
  }

  function publish(next: ClientSnapshot): void {
    applySnapshot(next)
    for (const listener of [...listeners]) listener()
  }

  function schedulePublish(next: ClientSnapshot): void {
    if (coalesceMs <= 0) {
      publish(next)
      return
    }
    // 合帧：窗口内只保留最后一次快照（新的覆盖旧的），窗口结束再通知一次
    pending = next
    if (timer !== null) return
    timer = setTimeout(() => {
      timer = null
      const latest = pending
      pending = null
      if (latest) publish(latest)
    }, coalesceMs)
  }

  let pending: ClientSnapshot | null = null

  source.subscribe((next) => schedulePublish(next))

  return {
    getState() {
      // 应用过通知 → 直接用；否则看看来源的廉价判据（进程内替身才有）说没说"变了"
      if (state === null) {
        applySnapshot(source.snapshot())
      } else if (source.stalenessKeys && !sameKeys(keys, source.stalenessKeys())) {
        applySnapshot(source.snapshot())
      }
      return state!
    },
    subscribe(listener) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    flush() {
      if (timer !== null) {
        clearTimeout(timer)
        timer = null
      }
      pending = null
      applySnapshot(source.snapshot())
      for (const listener of [...listeners]) listener()
    },
    showConfirm(next) {
      confirmModal = next
      if (state) state = compose(snapshot ?? source.snapshot())
      for (const listener of [...listeners]) listener()
    },
    closeConfirm() {
      confirmModal = null
      if (state) state = compose(snapshot ?? source.snapshot())
      for (const listener of [...listeners]) listener()
    },
    setConnection(next) {
      connection = next
      if (state) state = compose(snapshot ?? source.snapshot())
      for (const listener of [...listeners]) listener()
    },
    pickFiles(request) {
      filePicker = request
      if (state) state = compose(snapshot ?? source.snapshot())
      for (const listener of [...listeners]) listener()
    },
    closeFilePicker() {
      filePicker = null
      if (state) state = compose(snapshot ?? source.snapshot())
      for (const listener of [...listeners]) listener()
    },
    get publishes() {
      return publishes
    },
  }
}

/**
 * 焦点会话：优先客户端本地焦点（`ui.activeId`），其次主机记的焦点，最后退回第一个。
 * 会话列表恒非空（主机保证至少有一条），所以最后一个分支不会真的取到 undefined。
 */
function activeThreadOf(next: ClientSnapshot): ClientState['active'] {
  return (
    next.threads.find((thread) => thread.id === next.ui.activeId) ??
    next.threads.find((thread) => thread.id === next.activeThreadId) ??
    next.threads[0]!
  )
}

/** 项目清单：按"每个工作区的最新会话时间"倒序（与主机侧 `projects` 同一规则）。 */
function projectsFrom(next: ClientSnapshot): string[] {
  const newest = new Map<string, number>()
  for (const thread of next.threads) {
    const seen = newest.get(thread.workspace)
    if (seen === undefined || thread.createdAt > seen) newest.set(thread.workspace, thread.createdAt)
  }
  return [...newest.entries()].sort((a, b) => b[1] - a[1]).map(([path]) => path)
}

/** 开着的标签（顺序就是标签顺序）。 */
function openTabsFrom(next: ClientSnapshot): ClientState['openTabs'] {
  const byId = new Map(next.threads.map((thread) => [thread.id, thread]))
  const tabs = []
  for (const id of next.ui.openTabIds) {
    const thread = byId.get(id)
    if (thread) tabs.push(thread)
  }
  if (tabs.length === 0) {
    const active = byId.get(next.ui.activeId)
    if (active) tabs.push(active)
  }
  return tabs
}
