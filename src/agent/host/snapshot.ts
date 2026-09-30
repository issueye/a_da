/**
 * 主机侧：把内存状态组装成**一份可渲染的快照**（M1-3）。
 *
 * 为什么要单独一个模块而不是让客户端去读 store：M3 起主机与客户端分处两个进程，
 * 客户端**读不到**主机的内存对象。所以从 M1 开始就要把"组装快照"这件事放在主机侧，
 * 客户端只负责"应用"。这一步做完，M3 才是纯粹的传输替换。
 *
 * ## 陈旧判据（`stalenessKeys`）
 *
 * 事件发射挂在 `store.subscribe`（那 88 处 `notify()` 的共同出口）上，正常路径不会漏。
 * 但"有人改了状态却没通知"是这类系统最典型的静默失效（界面停住、还不报错），
 * 所以这里再给一组**廉价结构判据**（身份 + 长度 + 分配型 getter 的指纹）供进程内替身使用：
 * 客户端问一句"你变了吗"，变了就重新取快照。**WebSocket 源不提供这个**——
 * 那时只有 `seq` 与重连兜底（协议 §1.3），这也是把它设计成"可选能力"的原因。
 */

import type { AgentStore } from '../store'
import type { ClientSnapshot } from '../../shared/protocol'

/** 组装一份主机快照。整份给：M1 先粗后细，只有高频路径留给 M3 做增量。 */
export function readHostSnapshot(store: AgentStore): ClientSnapshot {
  const threads = store.threads
  const activeId = store.activeId
  return {
    threads,
    activeThreadId: activeId,
    runningThreadIds: threads.filter((thread) => store.isThreadRunning(thread.id)).map((t) => t.id),
    waitingThreadIds: threads.filter((thread) => store.isThreadWaiting(thread.id)).map((t) => t.id),
    queue: store.queue,
    log: store.log,
    workspace: {
      project: store.project,
      files: store.workspaceInfo.files,
      dirs: store.workspaceInfo.dirs,
      scanning: store.workspaceInfo.scanning,
      entries: store.entries,
    },
    config: {
      model: store.currentModel,
      contextWindow: store.contextWindow,
      supportsImages: store.supportsImages,
      approval: store.approval,
      effort: store.effort,
      mode: store.mode,
    },
    pendingQuestions: store.pendingAnswerQuestions,
    publicWorkspace: store.publicWorkspace,
    appearance: store.appearance,
    ui: {
      activeId,
      openTabIds: store.openTabIds,
      pendingDraft: store.pendingDraft,
      debugOpen: store.debugOpen,
      settingsOpen: store.settingsOpen,
      pluginsOpen: store.pluginsOpen,
      changesOpen: store.changesOpen,
      paletteOpen: store.paletteOpen,
      sidebarOpen: store.sidebarOpen,
      searchOpen: store.searchOpen,
    },
  }
}

/**
 * 廉价结构判据：任何一项变了就认为"状态可能变了"。
 *
 * 只用 O(1) 或小 O(n) 的原始值：`threads` / `entries` / `log` 是字段（身份稳定）；
 * `queue` / `openTabs` 是"每次读都新建数组"的 getter，所以取长度 + 端点 id 的指纹；
 * 其余是标量。指纹漏掉的中间变化由**通知**兜住（通知是主信号，这里只是防漏）。
 */
export function stalenessKeys(store: AgentStore): unknown[] {
  const queue = store.queue
  const openTabs = store.openTabs
  const pending = store.pendingAnswerQuestions
  const info = store.workspaceInfo
  return [
    store.activeId,
    store.active,
    store.active.items,
    store.active.items.length,
    store.threads,
    store.threads.length,
    store.log,
    store.log.length,
    store.entries,
    store.entries.length,
    store.project,
    info.files,
    info.dirs,
    info.scanning,
    store.currentModel,
    store.contextWindow,
    store.supportsImages,
    store.approval,
    store.effort,
    store.mode,
    store.pendingDraft,
    store.appearance,
    store.debugOpen,
    store.settingsOpen,
    store.pluginsOpen,
    store.changesOpen,
    store.paletteOpen,
    store.sidebarOpen,
    store.searchOpen,
    queue.length,
    queue.map((item) => item.item?.id ?? '').join('|'),
    openTabs.map((thread) => thread.id).join('|'),
    pending.map((entry) => entry.callId).join('|'),
    store.projects.join('|'),
  ]
}
