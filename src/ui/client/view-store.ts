/**
 * 复制视图（M1-1 / M1-4 / M1-5 / M1-6）。
 *
 * ## 它解决什么
 *
 * M0 结束时 UI 已经不直接抓 `store` 了，但 `client.state` 拿到的仍是**那个活对象**——
 * 界面能工作只是因为"同进程、读的就是最新值"。M1 要把它换成**一份由变更驱动的复制视图**：
 * UI 读的是快照，快照只在来源变了之后重建。这样 M3 把来源换成事件流时，UI 一行都不用改。
 *
 * ## 两个刻意的设计决定
 *
 * 1. **不逐条改造来源的变更点**（计划里 M1-2 原写的是"88 处 `notify()` 逐条归类"）。
 *    实测 `store.subscribe` 就是那 88 处共同的唯一出口，所以在这里挂一次即可；
 *    那 88 处的价值从"逐个改造"变成"**快照覆盖度清单**"——哪些状态必须进快照。
 *    好处是改动面从 88 处降到 1 处，而且不会漏（漏一处 `notify()` 的代价是界面不刷新，
 *    而挂在广播出口上根本不存在这个漏法）。
 * 2. **合帧（coalescing）**：来源在一帧内可能通知很多次（流式文本尤其）。默认 16ms 窗口内
 *    合并成一次通知，`flush()` 可强制立即发布——与 store 自己的 `notifySoon()` 同一思路，
 *    只不过这次做在客户端一侧，M3 换成 WebSocket 后同一份逻辑就是"批量下发"。
 *
 * ## 陈旧检测：通知 + 廉价结构比对
 *
 * 只信通知是不够的：测试里大量直接改状态造数据（`thread.items = [...]`）而不会通知，
 * 生产里也可能有人加了字段忘了通知。所以 `getState()` 里再做一次**廉价结构比对**——
 * 比的是"身份 + 长度"这类 O(1) 字段（`active`、`items` 引用与长度、`threads`、`queue`、
 * `openTabIds`…），任一变化就重建。它不改语义（变了就是变了），只是让"漏通知"这种
 * 静默失效无处藏身。
 *
 * **已知边界**：深层原地改（`items[0].text = 'x'`）不带通知时检测不到——那类改动在生产里
 * 必然伴随 `notify()`（流式就是走 `notifySoon`），测试里若真有这种写法，用 `flush()` 显式提交。
 */

import type { ClientState } from './types'

/** 快照来源：M0/M1 是 store；M3 起是事件流累积出来的状态。 */
export type StateSource = ClientState

export interface ViewStoreOptions {
  /** 变更来源：把它当成"状态可能变了"的广播口（M0/M1 传 `store.subscribe`）。 */
  subscribeToSource: (listener: () => void) => () => void
  /** 合并窗口（ms）。0 = 同步通知（进程内调试用）。默认 16。 */
  coalesceMs?: number
}

export interface ViewStore {
  /** 当前快照。同一次变更期间重复调用返回**同一个对象**（引用稳定，React 不会白重渲染）。 */
  getState(): ClientState
  /** 订阅"快照已更新"（已合帧）。 */
  subscribe(listener: () => void): () => void
  /** 立刻重建并同步通知（绕过合帧窗口）。 */
  flush(): void
  /** 已发布次数（每次重建 +1）；测试与诊断用。 */
  readonly publishes: number
}

/**
 * 廉价陈旧判据：**只用 O(1) 或小 O(n) 的原始值**，不做深比较。
 *
 * 为什么不能直接比数组身份：`queue` / `openTabs` / `projects` / `pendingAnswerQuestions`
 * 在 store 里都是**每次读都新建数组的 getter**，比身份会永远"不等"，等于每次访问都重建
 * （快照引用不稳定，React 会白重渲染）。所以这几项按"长度 + 端点 id"取指纹；
 * 指纹漏掉的中间变化由**通知**兜住——通知才是主信号，这里只是防漏的安全网
 * （见文件头"陈旧检测"一节）。
 */
function staleKeys(source: StateSource): unknown[] {
  const active = source.active
  const items = active.items
  const queue = source.queue
  const openTabs = source.openTabs
  const pending = source.pendingAnswerQuestions
  const info = source.workspaceInfo
  return [
    // 身份稳定的字段：直接比引用
    source.activeId,
    active,
    items,
    items.length,
    source.threads,
    source.threads.length,
    source.log,
    source.log.length,
    source.entries,
    source.entries.length,
    source.confirmModal,
    source.project,
    // 标量
    active.isSubagent === true,
    active.title,
    info.files,
    info.dirs,
    info.scanning,
    source.currentModel,
    source.contextWindow,
    source.supportsImages,
    source.approval,
    source.effort,
    source.mode,
    source.pendingDraft,
    source.appearance,
    source.debugOpen,
    source.settingsOpen,
    source.pluginsOpen,
    source.changesOpen,
    source.paletteOpen,
    source.sidebarOpen,
    source.searchOpen,
    source.running,
    // 分配型 getter：按指纹
    queue.length,
    queue.map((item) => item.item?.id ?? '').join('|'),
    openTabs.map((thread) => thread.id).join('|'),
    pending.length,
    pending.map((entry) => entry.callId).join('|'),
    source.projects.join('|'),
    source.projectThreads.length,
  ]
}

function sameKeys(a: unknown[], b: unknown[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false
  return true
}

export function createViewStore(source: StateSource, options: ViewStoreOptions): ViewStore {
  const coalesceMs = options.coalesceMs ?? 16

  /** 读助手是纯查询、不依赖快照数据，所以只建一次并跨快照复用—— */
  /** 免得每次重建都产生一批新函数引用，把组件的依赖数组搅乱。 */
  const helpers: Pick<
    ClientState,
    | 'isThreadRunning'
    | 'isThreadWaiting'
    | 'labelFor'
    | 'isPublic'
    | 'getThreadChangeCount'
    | 'getThreadFileChanges'
  > = {
    isThreadRunning: (threadId) => source.isThreadRunning(threadId),
    isThreadWaiting: (threadId) => source.isThreadWaiting(threadId),
    labelFor: (workspace) => source.labelFor(workspace),
    isPublic: (workspace) => source.isPublic(workspace),
    getThreadChangeCount: (threadId) => source.getThreadChangeCount(threadId),
    getThreadFileChanges: (threadId) => source.getThreadFileChanges(threadId),
  }

  /** 从来源读一份新快照。嵌套的领域对象仍复用来源的引用（M1 同进程；M3 起是反序列化副本）。 */
  function readSnapshot(): ClientState {
    return {
      threads: source.threads,
      active: source.active,
      queue: source.queue,
      log: source.log,
      workspaceInfo: source.workspaceInfo,
      entries: source.entries,
      currentModel: source.currentModel,
      contextWindow: source.contextWindow,
      supportsImages: source.supportsImages,
      approval: source.approval,
      effort: source.effort,
      activeThreadStats: source.activeThreadStats,
      pendingAnswerQuestions: source.pendingAnswerQuestions,

      activeId: source.activeId,
      project: source.project,
      projects: source.projects,
      projectThreads: source.projectThreads,
      openTabs: source.openTabs,
      mode: source.mode,
      running: source.running,
      pendingDraft: source.pendingDraft,
      appearance: source.appearance,
      debugOpen: source.debugOpen,
      settingsOpen: source.settingsOpen,
      pluginsOpen: source.pluginsOpen,
      changesOpen: source.changesOpen,
      paletteOpen: source.paletteOpen,
      sidebarOpen: source.sidebarOpen,
      searchOpen: source.searchOpen,
      confirmModal: source.confirmModal,

      ...helpers,
    }
  }

  let snapshot: ClientState | null = null
  let keys: unknown[] = []
  let dirty = true
  let publishes = 0
  const listeners = new Set<() => void>()
  let timer: ReturnType<typeof setTimeout> | null = null

  function rebuild(): void {
    snapshot = readSnapshot()
    keys = staleKeys(source)
    dirty = false
    publishes += 1
  }

  function publish(): void {
    rebuild()
    for (const listener of [...listeners]) listener()
  }

  function schedulePublish(): void {
    if (coalesceMs <= 0) {
      publish()
      return
    }
    if (timer !== null) return
    timer = setTimeout(() => {
      timer = null
      publish()
    }, coalesceMs)
  }

  options.subscribeToSource(() => {
    dirty = true
    schedulePublish()
  })

  return {
    getState() {
      // 通知过 → 重建；没通知但结构变了（测试造数据 / 漏通知）→ 也重建
      if (dirty || snapshot === null || !sameKeys(keys, staleKeys(source))) rebuild()
      return snapshot!
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
      dirty = true
      publish()
    },
    get publishes() {
      return publishes
    },
  }
}
