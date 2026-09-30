/**
 * 进程内客户端：把协议方法映射到今天那个 store（M0 的测试替身 / 迁移目标）。
 *
 * **为什么 M0 就用协议形状（`request`）而不是让 UI 继续直调 store**：这样 M3 换 WebSocket
 * 时 UI 一行都不用改——它从一开始面对的就是"异步命令"。映射表按 `docs/jsonrpc-protocol.md`
 * §3 的方法名逐条写，参数以协议为准；store 与协议签名不一致的地方在每条上写明。
 *
 * **M0 的两处已知偏差**（都在下面逐条标了 `M0:`）：
 * 1. 不带 `threadId` 路由的 store 方法（`send`）只用"当前会话"——UI 传的 `threadId` 仅作校验；
 * 2. `workspace.entries` 返回整份缓存（协议要求分页），M2 再做分页。
 */

import type { AgentStore } from '../../agent/store'
import { createHostEmitter } from '../../agent/host/emitter'
import { AppErrorCode, ProtocolError } from '../../shared/protocol'
import type { ParamsOf, ProtocolMethod, ResultOf } from '../../shared/protocol'
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

  const liveState: ClientState = store
  /** 两种读法共用的取状态入口：M1 起默认返回复制视图的应用结果。 */
  const readState = (): ClientState => (viewStore ? viewStore.getState() : liveState)

  const ui: UiActions = {
    openTab: (threadId) => store.openTab(threadId),
    closeTab: (threadId) => store.closeTab(threadId),
    setChangesOpen: (open) => store.setChangesOpen(open),
    setPaletteOpen: (open) => store.setPaletteOpen(open),
    setPlugins: (open) => store.setPlugins(open),
    setSettings: (open) => store.setSettings(open),
    setSearchOpen: (open) => store.setSearchOpen(open),
    toggleSidebar: () => store.toggleSidebar(),
    toggleAppearance: () => store.toggleAppearance(),
    toggleDebug: () => store.toggleDebug(),
    applyPromptToComposer: (content) => store.applyPromptToComposer(content),
    clearPendingDraft: () => store.clearPendingDraft(),
    // 确认框是**客户端本地**状态：回调不可能上线，所以它不进主机快照（见 view-store 文件头）。
    showConfirm: (options) => {
      if (viewStore) viewStore.showConfirm(options)
      else store.showConfirm(options)
    },
    closeConfirm: () => {
      if (viewStore) viewStore.closeConfirm()
      else store.closeConfirm()
    },
  }

  /** 未实现的协议方法：明确报错，而不是静默 no-op（"不允许静默失效"）。 */
  const notImplemented = (method: ProtocolMethod): never => {
    throw new ProtocolError(
      AppErrorCode.NotReady,
      `协议方法 ${method} 在进程内客户端里尚未实现`,
      { what: method }
    )
  }

  async function dispatch(method: ProtocolMethod, params: unknown): Promise<unknown> {
    const p = (params ?? {}) as Record<string, unknown>
    switch (method) {
      // ── 会话 ──
      case 'thread.create': {
        const thread = store.newThread(p.workspace as string)
        return { threadId: thread.id }
      }
      case 'thread.delete':
        return { message: await store.deleteThread(p.threadId as string) }
      case 'thread.send': {
        // M0：store.send 只用"当前会话"；传了别的 id 就如实说出来，别假装路由成功
        if (p.threadId && p.threadId !== store.activeId) {
          store.trace(`[客户端] thread.send 的 threadId 与当前会话不一致，M0 仍发给当前会话`)
        }
        store.send(p.text as string, p.images as string[] | undefined)
        return undefined
      }
      case 'thread.abort':
        store.stop(p.threadId as string)
        return undefined
      case 'thread.compact':
        return await store.compactThread(p.threadId as string, {
          customInstructions: p.customInstructions as string | undefined,
          trigger: p.trigger as 'manual' | 'auto' | undefined,
        })
      case 'thread.setMode':
        // M0：store.setMode 是全局写作模式（会同步给当前会话）；threadId 只作说明
        store.setMode(p.mode as ParamsOf<'thread.setMode'>['mode'])
        return undefined
      case 'thread.setWorkspace':
        store.setThreadWorkspace(p.threadId as string, p.workspace as string)
        return undefined
      case 'thread.editAndResend':
        // 注意参数顺序与协议不同：store 是 (itemId, text, images?, threadId?)
        await store.editUserMessageAndResend(
          p.itemId as string,
          p.text as string,
          p.images as string[] | undefined,
          p.threadId as string
        )
        return undefined

      // ── 排队指令 ──
      case 'queue.clear':
        store.clearQueue(p.threadId as string | undefined)
        return undefined
      case 'queue.promote':
        store.sendQueuedImmediately(p.index as number, p.threadId as string | undefined)
        return undefined
      case 'queue.remove':
        return store.removeQueuedItem(p.index as number, p.threadId as string | undefined)

      // ── 子智能体 ──
      case 'subagent.resume': {
        const { thread } = await store.resumeSubagentThread({
          subagentThreadId: p.subagentThreadId as string,
          instruction: p.instruction as string | undefined,
        })
        return { threadId: thread.id }
      }

      // ── 审批 / 提问 ──
      case 'approval.decide':
        store.decide(p.toolItemId as string, p.approved as boolean)
        return undefined
      case 'question.answer':
        store.answerQuestion(p.callId as string, {
          choice: p.choice as string | undefined,
          text: p.text as string | undefined,
        })
        return undefined

      // ── 工作区 ──
      case 'workspace.add':
        return { error: await store.addProject(p.path as string) }
      case 'workspace.remove':
        return { message: store.removeProject(p.workspace as string) }
      case 'workspace.openPublic':
        await store.openPublicWorkspace(p.threadId as string | undefined)
        return undefined
      case 'workspace.rescan':
        await store.refresh()
        return undefined
      case 'workspace.entries':
        // M0：整份返回（协议要求分页，M2 再做）
        return store.entries

      // ── 焦点上报 ──
      case 'ui.activeThread':
        store.selectThread(p.threadId as string)
        return undefined
      case 'ui.activeProject':
        store.selectProject(p.workspace as string)
        return undefined

      // ── 配置 ──
      case 'config.setProvider':
        return { error: await store.saveProvider(p.config as ParamsOf<'config.setProvider'>['config']) }
      case 'config.checkProvider':
        return { message: await store.checkProvider(p.config as ParamsOf<'config.checkProvider'>['config']) }
      case 'config.setApproval':
        store.setApproval(p.mode as ParamsOf<'config.setApproval'>['mode'])
        return undefined
      case 'config.setEffort':
        store.setEffort(p.effort as ParamsOf<'config.setEffort'>['effort'])
        return undefined

      // ── 改动审阅 ──
      case 'change.count':
        return { count: store.getThreadChangeCount(p.threadId as string) }
      case 'change.list':
        return store.getThreadFileChanges(p.threadId as string)
      case 'change.revertCard':
        return { ok: await store.revertCard(p.threadId as string, p.cardId as string) }
      case 'change.revertFile':
        return { ok: await store.revertFile(p.threadId as string, p.path as string) }
      case 'change.revertAll':
        return { ok: await store.revertAllChanges(p.threadId as string) }

      // ── 调试 ──
      case 'debug.trace':
        store.trace(p.text as string)
        return undefined
      case 'debug.log.clear':
        store.clearLog()
        return undefined

      default:
        return notImplemented(method)
    }
  }

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
