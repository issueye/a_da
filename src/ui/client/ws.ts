/**
 * WebSocket 客户端（M3-3）：`AgentClient` 接口后面的**另一个实现**。
 *
 * 与 `in-process.ts` 的关系：两者面向同一个接口，UI 分不出差别（这正是 M0 定协议形状的目的）。
 * 差别只在传输——
 * - 进程内：命令直接交给同进程的派发表，事件直接喂复制视图；
 * - WebSocket：命令编码成 JSON-RPC 帧，事件从 socket 收。
 *
 * ## 三件事必须在这里做对
 *
 * 1. **按 id 关联响应**：并发命令各等各的，超时要如实失败（不静默吞）。
 * 2. **事件驱动复制视图**：把 `evt.*` 交给 `createViewStore` 应用；**不提供** `stalenessKeys`
 *    （那是进程内替身的防漏安全网，跨进程没有"偷看一眼主机内存"这回事）。
 * 3. **重连要重新对齐**：断开后自动重连，连上**先要一份快照**（协议 §1.3），
 *    因为它可能错过了断开期间的 `seq`——这份快照就是重新对齐的唯一依据。
 *
 * ## 反向请求（`req.*`）暂时不接
 *
 * 协议 §5 定义了主机向客户端要审批/提问的反向请求。今天的界面**不需要**它：审批与提问
 * 都以条目状态出现在快照里，客户端用命令答复（`approval.decide` / `question.answer`）。
 * 所以这里收到 `req.*` 一律如实回一个错误，而不是假装处理——等真有主机会发它时再接。
 */

import { PROTOCOL_VERSION, RpcErrorCode } from '../../shared/protocol'
import type { ClientSnapshot, ProtocolMethod } from '../../shared/protocol'
import type { AgentClient, ClientState, UiActions } from './types'
import { createViewStore } from './view-store'

export interface WebSocketClientOptions {
  /** 形如 `ws://127.0.0.1:PORT/?token=...` */
  url: string
  token: string
  clientName?: string
  /** 事件合帧窗口（ms）。跨进程默认 16（协议 §7.1）。 */
  coalesceMs?: number
  /** 单条命令超时（ms）。默认 30s：模型侧的写命令可能很久。 */
  requestTimeoutMs?: number
  /** 重连间隔（ms）。0 = 不重连（测试里常用）。 */
  reconnectDelayMs?: number
  /** 诊断用：连接状态变化时回调。 */
  onStatus?: (status: WebSocketClientStatus) => void
}

export type WebSocketClientStatus = 'connecting' | 'connected' | 'disconnected'

export interface WebSocketClient extends AgentClient {
  /** 连上并完成一次快照对齐后 resolve（首次连接用）。 */
  ready(): Promise<void>
  /** 主动关闭（不再重连）。 */
  close(): void
  readonly status: WebSocketClientStatus
}

export function createWebSocketClient(options: WebSocketClientOptions): WebSocketClient {
  const requestTimeoutMs = options.requestTimeoutMs ?? 30_000
  const reconnectDelayMs = options.reconnectDelayMs ?? 1000

  // ── 复制视图的输入口 ──
  let latest: ClientSnapshot | null = null
  const snapshotListeners = new Set<(snapshot: ClientSnapshot) => void>()
  const viewStore = createViewStore(
    {
      snapshot: () => {
        if (!latest) throw new Error('还没收到过主机的快照')
        return latest
      },
      subscribe: (listener) => {
        snapshotListeners.add(listener)
        return () => {
          snapshotListeners.delete(listener)
        }
      },
      // 刻意**不提供** stalenessKeys：跨进程没有"偷看主机内存"这回事
    },
    { coalesceMs: options.coalesceMs ?? 16 }
  )

  let socket: WebSocket | null = null
  let status: WebSocketClientStatus = 'connecting'
  let closedByUs = false
  let reconnectAttempts = 0
  let nextId = 1
  const pending = new Map<
    number,
    { resolve: (value: unknown) => void; reject: (err: Error) => void; timer: ReturnType<typeof setTimeout> }
  >()
  const readyWaiters: Array<() => void> = []

  function setStatus(next: WebSocketClientStatus, reason?: string): void {
    status = next
    // 界面要从 client.state 读到它（协议 §1.5：断开必须"可理解 + 可重连"）
    viewStore.setConnection({
      status: next,
      attempts: next === 'connected' ? 0 : reconnectAttempts,
      ...(reason ? { reason } : {}),
    })
    options.onStatus?.(next)
  }

  function applySnapshot(snapshot: ClientSnapshot): void {
    latest = snapshot
    for (const listener of [...snapshotListeners]) listener(snapshot)
  }

  function failAllPending(reason: string): void {
    for (const [id, entry] of [...pending]) {
      clearTimeout(entry.timer)
      pending.delete(id)
      entry.reject(new Error(reason))
    }
  }

  function send(frame: unknown): void {
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error('与主机的连接不可用')
    }
    socket.send(JSON.stringify(frame))
  }

  function handleFrame(raw: string): void {
    let frame: { id?: number; method?: string; params?: unknown; result?: unknown; error?: { code: number; message: string; data?: unknown } }
    try {
      frame = JSON.parse(raw) as typeof frame
    } catch {
      // 主机会保证帧合法；真收到坏帧就记下来，不要静默
      console.warn('[ws-client] 收到无法解析的帧:', raw.slice(0, 200))
      return
    }

    // ① 命令响应
    if (typeof frame.id === 'number') {
      const entry = pending.get(frame.id)
      if (frame.method !== undefined) {
        // 反向请求：今天的界面用不到（见文件头），如实拒绝
        send({
          jsonrpc: '2.0',
          id: frame.id,
          error: { code: RpcErrorCode.MethodNotFound, message: `客户端不处理反向请求 ${frame.method}` },
        })
        return
      }
      if (!entry) return
      clearTimeout(entry.timer)
      pending.delete(frame.id)
      if (frame.error) {
        const err = new Error(frame.error.message) as Error & { code?: number; data?: unknown }
        err.code = frame.error.code
        err.data = frame.error.data
        entry.reject(err)
        return
      }
      entry.resolve(frame.result)
      return
    }

    // ② 事件（服务端 → 客户端通知）
    if (frame.method === 'evt.state.snapshot') {
      const event = frame.params as { seq: number; payload: ClientSnapshot }
      applySnapshot(event.payload)
      // 第一份快照到手 = "连上并对齐了"
      for (const waiter of readyWaiters.splice(0)) waiter()
      return
    }
    // 其余 topic 属于 M3 的细粒度事件（还没有主机发它们）；先把原文记下来，不静默丢
    if (frame.method) console.warn('[ws-client] 暂不认识的事件:', frame.method)
  }

  function connect(): void {
    setStatus('connecting')
    const ws = new WebSocket(`${options.url}${options.url.includes('?') ? '&' : '?'}token=${encodeURIComponent(options.token)}`)
    socket = ws
    ws.addEventListener('open', () => {
      setStatus('connected')
      // 先握手再干别的：主机在握手前只接受 `session.initialize`（协议 §1.2），
      // 抢跑的命令会拿到 -32001 Unauthorized。令牌在这里再报一次（协议 §1.6）。
      void request('session.initialize', {
        token: options.token,
        protocolVersion: PROTOCOL_VERSION,
        client: options.clientName ? { name: options.clientName } : undefined,
      })
        .catch((err: Error) => {
          console.error('[ws-client] 握手失败:', err.message)
        })
        .finally(() => {
          // 服务端在 open 时也会先推一份快照；如果那条丢了，这里兜一次
          void request('session.snapshot', {}).then(
            (snapshot) => {
              // 只在还没收到任何快照时用它，避免把更新的一份盖回去
              if (!latest) applySnapshot(snapshot as ClientSnapshot)
              for (const waiter of readyWaiters.splice(0)) waiter()
            },
            () => {
              /* 拿不到就等 push 的那份 */
            }
          )
        })
    })

    ws.addEventListener('message', (event) => handleFrame(String((event as MessageEvent).data)))

    ws.addEventListener('close', (event) => {
      socket = null
      failAllPending('与主机的连接已断开')
      if (closedByUs) {
        setStatus('disconnected', '客户端主动关闭')
        return
      }
      // 关闭原因要带给界面：不然用户只看到"断了"，不知道是不是主机被杀
      const code = (event as CloseEvent).code
      const reason = (event as CloseEvent).reason
      reconnectAttempts += 1
      setStatus(
        'disconnected',
        reason || (code ? `主机侧关闭（code=${code}）` : '与主机的连接已断开')
      )
      if (reconnectDelayMs > 0) setTimeout(connect, reconnectDelayMs)
    })

    ws.addEventListener('error', () => {
      // 具体原因由 close 事件统一处理；这里只记一笔
      console.warn('[ws-client] 连接出错:', options.url)
    })
  }

  function request(method: ProtocolMethod, params: unknown): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const id = nextId++
      try {
        send({ jsonrpc: '2.0', id, method, params })
      } catch (err) {
        reject(err as Error)
        return
      }
      const timer = setTimeout(() => {
        pending.delete(id)
        const err = new Error(`命令 ${method} 超时（${requestTimeoutMs}ms）`) as Error & { code?: number }
        err.code = -32005 // AppErrorCode.Timeout
        reject(err)
      }, requestTimeoutMs)
      pending.set(id, { resolve, reject, timer })
    })
  }

  /** UI 外壳动作：与进程内适配器同形——凡进快照的走命令，确认框留本地。 */
  const ui: UiActions = {
    openTab: (threadId) => void request('ui.openTab', { threadId }).catch(reportUiFailure),
    closeTab: (threadId) => void request('ui.closeTab', { threadId }).catch(reportUiFailure),
    setChangesOpen: (open) => void request('ui.setShell', { patch: { changesOpen: open } }).catch(reportUiFailure),
    setPaletteOpen: (open) => void request('ui.setShell', { patch: { paletteOpen: open } }).catch(reportUiFailure),
    setPlugins: (open) => void request('ui.setShell', { patch: { pluginsOpen: open } }).catch(reportUiFailure),
    setSettings: (open) => void request('ui.setShell', { patch: { settingsOpen: open } }).catch(reportUiFailure),
    setSearchOpen: (open) => void request('ui.setShell', { patch: { searchOpen: open } }).catch(reportUiFailure),
    // 切换类动作按**绝对值**发：多客户端下"切换"会互相抵消
    toggleSidebar: () =>
      void request('ui.setShell', { patch: { sidebarOpen: !viewStore.getState().sidebarOpen } }).catch(
        reportUiFailure
      ),
    toggleAppearance: () =>
      void request('ui.setShell', {
        patch: { appearance: viewStore.getState().appearance === 'dark' ? 'light' : 'dark' },
      }).catch(reportUiFailure),
    toggleDebug: () =>
      void request('ui.setShell', { patch: { debugOpen: !viewStore.getState().debugOpen } }).catch(
        reportUiFailure
      ),
    applyPromptToComposer: (content) =>
      void request('ui.setShell', { patch: { pendingDraft: content } }).catch(reportUiFailure),
    clearPendingDraft: () =>
      void request('ui.setShell', { patch: { pendingDraft: null } }).catch(reportUiFailure),
    showConfirm: (confirmOptions) => viewStore.showConfirm(confirmOptions),
    closeConfirm: () => viewStore.closeConfirm(),
    // 轻提示是客户端本地状态：不过协议（协议 §9.1 的 C 组），所以两端只是各自渲染
    notify: (toast) => viewStore.notify(toast),
    dismissToast: (id) => viewStore.dismissToast(id),
    pickFiles: (request) => viewStore.pickFiles(request),
    closeFilePicker: () => viewStore.closeFilePicker(),
    /**
     * 切焦点：本地立刻生效（`focusThread`），主机那边尽力通知。
     *
     * 这是"点了没反应、30 秒后弹超时"那类问题的根治：焦点归客户端（协议 §11 定案 #1），
     * 不该等主机回话；通知失败只记日志。
     */
    activateThread: (threadId) => {
      viewStore.focusThread(threadId)
      void request('ui.activeThread', { threadId }).catch((err: Error) =>
        console.warn('[ws-client] 焦点通知主机失败:', err.message)
      )
    },
    activateProject: (workspace) => {
      void request('ui.activeProject', { workspace }).catch((err: Error) =>
        console.warn('[ws-client] 切换项目通知主机失败:', err.message)
      )
    },
  }

  /** UI 外壳动作失败不弹窗（它只是"开关没生效"），但必须在控制台留痕，不静默吞掉。 */
  function reportUiFailure(err: Error): void {
    console.warn('[ws-client] UI 外壳动作失败:', err.message)
  }

  connect()

  return {
    get state(): ClientState {
      return viewStore.getState()
    },
    ui,
    subscribe: (listener) => viewStore.subscribe(listener),
    /**
     * 重新对齐：向主机要一份当前快照并应用。
     *
     * 跨进程时这个动作**有意义**（客户端可能错过了断开期间的 `seq`），
     * 与进程内那种"重新读一眼内存"不同。
     */
    refreshState: () => {
      void request('session.snapshot', {}).then(
        (snapshot) => applySnapshot(snapshot as ClientSnapshot),
        reportUiFailure
      )
    },
    request: request as AgentClient['request'],
    ready: () =>
      new Promise<void>((resolve, reject) => {
        if (latest) {
          resolve()
          return
        }
        readyWaiters.push(resolve)
        setTimeout(() => reject(new Error('等待主机快照超时')), requestTimeoutMs)
      }),
    close: () => {
      closedByUs = true
      socket?.close(1000, 'client closing')
      socket = null
      setStatus('disconnected')
    },
    get status() {
      return status
    },
  }
}
