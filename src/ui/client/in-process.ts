/**
 * 进程内客户端：**本机传输替身**——它把命令交给同一进程里的主机派发表
 * （`agent/host/dispatch.ts`），并把主机事件直接喂给复制视图。
 *
 * 两个用途：开发/测试默认走它（不起进程、不起端口）；M3 的 WebSocket 客户端
 * （`ui/client/ws.ts`）在**同一个 `AgentClient` 接口**后面替换它，UI 不需要任何改动。
 *
 * 端口边界：这里只做"传输 + 视图"，**不实现任何命令**——命令一律在主机侧。
 */

import type { AgentStore } from '../../agent/store'
import type { ProtocolMethod } from '../../shared/protocol'
import { createHostEmitter } from '../../agent/host/emitter'
import { createCommandDispatcher } from '../../agent/host/dispatch'
import type { AgentClient, ClientState, UiActions } from './types'
import { createViewStore } from './view-store'

export interface InProcessClientOptions {
  /**
   * 是否用**复制视图**（M1 起默认）。关掉退回 M0 的"活对象"读法，便于对照调试；
   * 环境变量 `A_DA_CLIENT_VIEW=live` 也能关。
   */
  replicated?: boolean
  /** 合帧窗口（ms）：0 = 同步（进程内默认）；WebSocket 传输用 16–33ms。 */
  coalesceMs?: number
}

export function createInProcessClient(
  store: AgentStore,
  options: InProcessClientOptions = {}
): AgentClient {
  const replicated = options.replicated ?? process.env.A_DA_CLIENT_VIEW !== 'live'

  /**
   * 主机侧替身：发射器（把 store 的广播变成事件）+ 快照组装都在 `agent/host` 里——
   * 客户端只**应用**，不读主机内存（M1-3/M1-6）。
   */
  const emitter = replicated ? createHostEmitter(store, { coalesceMs: options.coalesceMs ?? 0 }) : null
  const viewStore = emitter
    ? createViewStore(
        {
          snapshot: () => emitter.snapshot(),
          subscribe: (listener) => emitter.subscribe((event) => listener(event.payload)),
          // 进程内替身的防漏安全网：改了状态却没通知时也能被发现（WS 源不提供）
          stalenessKeys: () => emitter.stalenessKeys(),
        },
        { coalesceMs: options.coalesceMs ?? 0 }
      )
    : null

  /**
   * 对照调试用（`A_DA_CLIENT_VIEW=live`）的活对象读法。
   *
   * 用 `Object.create` 而不是 `{...store}`：前者把 store 当**原型**，属性读仍然走它的 getter
   * （展开会把 getter 当场求值并冻住，等于把活对象变成一次性快照）。此处只补一个进程内恒真的
   * `connection`——界面会读它来决定"要不要显示断线提示"。
   */
  const liveState: ClientState = Object.create(store, {
    connection: {
      value: { status: 'connected', attempts: 0 },
      enumerable: true,
    },
  }) as ClientState
  /** 两种读法共用的取状态入口：M1 起默认返回复制视图的应用结果。 */
  const readState = (): ClientState => (viewStore ? viewStore.getState() : liveState)

  /**
   * 纯客户端动作。
   *
   * 这里**不直接改 store**：凡是会进快照的状态（外壳开关、主题、草稿、标签）都走命令，
   * 与 WebSocket 客户端**走同一条路**——这样这些命令在进程内就被 UI 测试覆盖了，
   * 而"M3 换传输"也不会碰到新的行为差异。真正纯客户端的只剩确认框（带回调，不可能上线）。
   */
  const ui: UiActions = {
    openTab: (threadId) => void dispatch('ui.openTab', { threadId }),
    closeTab: (threadId) => void dispatch('ui.closeTab', { threadId }),
    setChangesOpen: (open) => void dispatch('ui.setShell', { patch: { changesOpen: open } }),
    setPaletteOpen: (open) => void dispatch('ui.setShell', { patch: { paletteOpen: open } }),
    setPlugins: (open) => void dispatch('ui.setShell', { patch: { pluginsOpen: open } }),
    setSettings: (open) => void dispatch('ui.setShell', { patch: { settingsOpen: open } }),
    setSearchOpen: (open) => void dispatch('ui.setShell', { patch: { searchOpen: open } }),
    toggleSidebar: () =>
      void dispatch('ui.setShell', { patch: { sidebarOpen: !readState().sidebarOpen } }),
    toggleAppearance: () =>
      void dispatch('ui.setShell', {
        patch: { appearance: readState().appearance === 'dark' ? 'light' : 'dark' },
      }),
    toggleDebug: () => void dispatch('ui.setShell', { patch: { debugOpen: !readState().debugOpen } }),
    applyPromptToComposer: (content) =>
      void dispatch('ui.setShell', { patch: { pendingDraft: content } }),
    clearPendingDraft: () => void dispatch('ui.setShell', { patch: { pendingDraft: null } }),
    // 确认框是**客户端本地**状态：回调不可能上线，所以它不进主机快照（见 view-store 文件头）。
    showConfirm: (options) => {
      if (viewStore) viewStore.showConfirm(options)
      else store.showConfirm(options)
    },
    closeConfirm: () => {
      if (viewStore) viewStore.closeConfirm()
      else store.closeConfirm()
    },
    // 文件选择器同样是客户端本地状态（回调不可能上线），只是它会去问主机的 fs.*
    pickFiles: (request) => viewStore?.pickFiles(request),
    closeFilePicker: () => viewStore?.closeFilePicker(),
  }

  const dispatch = createCommandDispatcher(store)

  return {
    // `state` 是 getter：复制视图模式下每次访问取当前快照（同一个变更周期内引用稳定）
    get state() {
      return readState()
    },
    ui,
    // 订阅也走客户端层：复制视图模式下一并享受合帧（M3 换成 WebSocket 时这里是同一条缝）
    subscribe: (listener) =>
      viewStore ? viewStore.subscribe(listener) : store.subscribe(listener),
    /**
     * 立刻应用一次主机快照（绕过合帧窗口）。
     *
     * 进程内它的含义是"重新向主机替身要一份快照并应用"；WS 实现里对应"要一次快照"。
     * 测试造完数据后显式提交也用它。
     */
    refreshState: () => {
      // live 模式（对照调试用）读的就是活对象，没有"应用快照"这回事，因此无事可做
      viewStore?.flush()
    },
    // 泛型签名与内部 `unknown` 派发之间的转换集中在这里，只此一处
    request: ((method: ProtocolMethod, params: unknown) =>
      dispatch(method, params)) as AgentClient['request'],
  }
}
