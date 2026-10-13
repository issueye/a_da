/**
 * JSON-RPC 2.0 WebSocket 客户端。
 * 负责与本地 agent_core (或 a-da host) 建立安全双向隧道。
 * 支持在 Tauri 环境下通过 get_core_info 动态发现同进程分配的端口与认证令牌，
 * 在非 Tauri 环境下无缝回落至默认端口。
 */

// 调试日志**单条 payload** 上限（字节）：环形缓冲只限条数，条数有界不等于内存有界
// （vt.state.snapshot 一帧实测 1.8 MB，300 条就是 ~540 MB 常驻）。
const DEBUG_PAYLOAD_LIMIT = 4096

import type {
  ClientSnapshot,
  AgentMode,
  ProviderConfig,
  ProviderPreset,
  ApprovalMode,
  Effort,
  Thread,
  PluginItem,
  BuiltinToolInfo,
  ResolvedPluginCapabilitiesDto,
  PluginListResponse,
  SkillSummary,
  PluginCapabilities,
  Item,
  DebugEntry,
  FsRoot,
  FsListing,
  PromptItem,
  SubagentProfile,
  ModelProtocol,
  ModelEntry,
  ProviderEntry,
  QueuedItem,
  ProductInfo,
} from '../types'
// W6-T6：重连退避策略抽成**无副作用纯模块**（便于独立验证，见该文件注释）
import { reconnectDelayMs } from './reconnect-policy'

export type Listener = (snapshot: ClientSnapshot) => void

import {
  getCoreInfo,
  getDesktopConfig,
  setDesktopConfig,
  restartDesktopApp,
  getDesktopPlatform,
  type DesktopMode,
  type DesktopConfig,
  type CoreInfo,
} from './desktop-bridge'

export type { DesktopMode, DesktopConfig, CoreInfo }
export { getDesktopConfig, setDesktopConfig, restartDesktopApp }

/**
 * 探测并获取本地核心服务的连接配置
 */
async function resolveCoreConnection(
  defaultUrl = 'ws://127.0.0.1:52353/rpc',
  defaultToken = ''
): Promise<{ url: string; token: string; mode?: DesktopMode }> {
  try {
    const info = await getCoreInfo()
    if (info && info.alive && (info.port > 0 || Boolean(info.url))) {
      const resolvedUrl = info.url || `ws://127.0.0.1:${info.port}/rpc`
      console.log(`[AgentWS] 获取到核心服务连接信息: ${resolvedUrl} (mode: ${info.mode || 'gateway'})`)
      return {
        url: resolvedUrl,
        token: info.token || '',
        mode: info.mode,
      }
    }
    if (info?.error) {
      console.warn('[AgentWS] 核心服务启动返回错误:', info.error)
    }
  } catch (err) {
    console.warn('[AgentWS] 获取桌面端核心服务信息失败，回退到默认端口:', err)
  }
  return {
    url: defaultUrl,
    token: defaultToken,
  }
}

export class AgentWebSocketClient {
  private ws: WebSocket | null = null
  private nextId = 1
  private pendingRequests = new Map<number | string, { resolve: (val: any) => void; reject: (err: any) => void; method: string }>()
  private listeners = new Set<Listener>()
  private reconnectTimer: any = null
  private _connected = false
  public productInfo: ProductInfo | null = null

  // ── W6-T6：重连策略 ────────────────────────────────────────────────────────
  //
  // 修掉的三处缺陷：
  // 1. **固定 2s 重试**：服务端长期不在时会被无限次、等间隔地敲打；
  // 2. **没有代次保护**：`connect()` 可被多处触发（`onclose` 的定时器 + `setConnection`），
  //    旧连接的异步回调会覆盖新连接的 url/token，导致**多条 socket 并存**；
  // 3. **断线不清在途请求**：`onclose` 后 `pendingRequests` 里的 Promise 无人兑现，
  //    调用方要等 15s 超时才失败（或永久挂住）。

  /** 重连代次：每次 `connect()` 递增。异步回调据此判断"我还是当前那条连接吗"。 */
  private connectionGeneration = 0
  /** 连续失败次数（`onopen` 成功后清零）。 */
  private reconnectAttempts = 0
  /** 断线时立刻失败所有在途请求——**不许让调用方干等超时**。 */
  private failAllPending(reason: string) {
    if (this.pendingRequests.size === 0) return
    const entries = Array.from(this.pendingRequests.entries())
    this.pendingRequests.clear()
    for (const [, pending] of entries) {
      pending.reject(new Error(reason))
    }
  }

  /** 安排一次重连（带退避）。`onclose` 与创建失败两条路径共用。 */
  private scheduleReconnect() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
    }
    const delay = reconnectDelayMs(this.reconnectAttempts)
    this.reconnectAttempts += 1
    console.log(`[AgentWS] ${delay}ms 后重连（第 ${this.reconnectAttempts} 次尝试）`)
    this.reconnectTimer = setTimeout(async () => {
      this.reconnectTimer = null
      const conn = await resolveCoreConnection(this.url, this.token)
      this.url = conn.url
      this.token = conn.token
      if (conn.mode) {
        this.desktopMode = conn.mode
      }
      this.connect()
    }, delay)
  }

  public async reconnectImmediately() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    console.log('[AgentWS] 触发立即重连，正在探测服务端点...')
    const conn = await resolveCoreConnection(this.url, this.token)
    this.url = conn.url
    this.token = conn.token
    if (conn.mode) {
      this.desktopMode = conn.mode
    }
    this.connect()
  }

  /**
   * 将指定会话完整序列化导出为标准 Markdown 格式
   */
  public exportThreadToMarkdown(thread: Thread): string {
    const title = thread.title || '无标题会话'
    const dateStr = thread.createdAt ? new Date(thread.createdAt).toLocaleString('zh-CN') : '未知时间'
    const ws = thread.workspace || '默认工作区'
    const isPm = thread.mode === 'pm' || thread.agentId === 'ada-pm' || thread.agentId === 'pm-assistant'
    const prodName = isPm ? 'a_da 项目管理助手' : (this.productInfo?.name || 'a_da 编程助手')
    // PM 是**模式**：产品身份恒为 ada-coding（历史 `ada-pm` 写法只用于识别老会话）
    const agentId = this.productInfo?.id || 'ada-coding'

    const lines: string[] = [
      `# ${title}`,
      ``,
      `> - **工作区**: \`${ws}\``,
      `> - **创建时间**: ${dateStr}`,
      `> - **助手规格**: ${prodName} (${agentId})`,
      ``,
      `---`,
      ``,
    ]

    for (const item of thread.items || []) {
      if (item.kind === 'user') {
        lines.push(`## 👤 用户`)
        lines.push(``)
        lines.push(item.text || '')
        lines.push(``)
      } else if (item.kind === 'thinking') {
        if (item.text && item.text.trim()) {
          lines.push(`> 💭 **思考过程**`)
          lines.push(`>`)
          const quoted = item.text.trim().split('\n').map((l) => `> ${l}`).join('\n')
          lines.push(quoted)
          lines.push(``)
        }
      } else if (item.kind === 'toolCall') {
        lines.push(`🔧 **工具调用**: \`${item.name}\``)
        if (item.args && Object.keys(item.args).length > 0) {
          lines.push('```json')
          lines.push(JSON.stringify(item.args, null, 2))
          lines.push('```')
        }
        lines.push(``)
      } else if (item.kind === 'tool') {
        const status = item.status === 'error' ? '❌ 失败' : '✅ 成功'
        lines.push(`📋 **工具结果** (${status})`)
        if (item.output) {
          lines.push('```')
          lines.push(item.output.trim())
          lines.push('```')
        }
        lines.push(``)
      } else if (item.kind === 'assistant') {
        lines.push(`## 🤖 助手`)
        lines.push(``)
        lines.push(item.text || '')
        lines.push(``)
      }
    }

    return lines.join('\n')
  }

  /// 最后**应用**过的快照 seq（增量帧的 baseSeq 要跟它对齐；不连续就自愈要全量）
  private lastSnapshotSeq = -1

  public debugLogs: DebugEntry[] = []
  private debugListeners = new Set<(logs: DebugEntry[]) => void>()
  private debugNotifyTimer: ReturnType<typeof setTimeout> | null = null

  /// 调试日志单条 payload 的上限（字节）。
  ///
  /// 🔴 **条数有界 ≠ 内存有界**：环形缓冲只留 300 条，但每条都把整个事件 payload 挂上，
  /// 而 `evt.state.snapshot` 一帧实测 1.8 MB（一条长会话 822 个 item）——
  /// 300 × 1.8 MB ≈ **540 MB 常驻**，而且每帧还复制一次、通知界面重渲染。
  /// 所以这里在**收口处**统一限幅：大 payload 只留摘要。
  /// 把事件 payload 压成"能看的摘要"，超限就只留预览与真实体积。
  private capDebugPayload(method: string, payload: any): any {
    if (payload === undefined || payload === null) return payload

    // 1. 全量快照：**永不**入缓冲，只留规模摘要（这是最大的那一类）
    if (method.includes('snapshot')) {
      const p = payload?.payload || payload
      const threads = Array.isArray(p?.threads) ? p.threads : []
      const items = threads.reduce(
        (n: number, t: any) => n + (Array.isArray(t?.items) ? t.items.length : 0),
        0
      )
      return { 摘要: `全量快照 seq=${p?.seq ?? '?'}`, 线程数: threads.length, item数: items }
    }

    // 2. 线程列表事件：同样只留条数
    if (method === 'evt.thread.updated') {
      return { 摘要: '线程列表更新', 线程数: Array.isArray(payload?.threads) ? payload.threads.length : 0 }
    }

    // 3. 流式增量：只留长度，不留正文
    if (method.includes('delta')) {
      const text = typeof payload === 'string' ? payload : payload?.text
      return { 摘要: `${method} 增量`, 文本长度: typeof text === 'string' ? text.length : undefined }
    }

    // 4. 其它事件：小就原样留，大就截断（附真实体积，别让人以为日志本身就这么点）
    let json = ''
    try {
      json = JSON.stringify(payload)
    } catch {
      return { 摘要: '(payload 无法序列化)' }
    }
    if (json.length <= DEBUG_PAYLOAD_LIMIT) return payload
    return {
      摘要: `payload 过大已截断（原始 ${Math.round(json.length / 1024)} KB）`,
      预览: json.slice(0, 600),
    }
  }

  public recordDebug(entry: Omit<DebugEntry, 'id' | 'at'>) {
    const item: DebugEntry = {
      ...entry,
      payload: this.capDebugPayload(entry.method || "", (entry as any).payload),
      id: `dbg_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`,
      at: Date.now(),
    }
    this.debugLogs.unshift(item)
    if (this.debugLogs.length > 300) {
      this.debugLogs.pop()
    }
    // 通知节流：调试面板是给人看的，没必要每帧（16ms）刷一次 React
    if (this.debugNotifyTimer) return
    this.debugNotifyTimer = setTimeout(() => {
      this.debugNotifyTimer = null
      for (const listener of this.debugListeners) {
        listener([...this.debugLogs])
      }
    }, 250)
  }

  public subscribeDebug(listener: (logs: DebugEntry[]) => void): () => void {
    this.debugListeners.add(listener)
    listener([...this.debugLogs])
    return () => {
      this.debugListeners.delete(listener)
    }
  }

  public clearDebugLogs() {
    this.debugLogs = []
    for (const listener of this.debugListeners) {
      listener([])
    }
  }

  private creatingThreadPromise: Promise<{ threadId: string } | null> | null = null

  /** 已经回告过服务端的激活会话 id（防止快照高频到达时重复发 `ui.activeThread`）。 */
  private lastActiveSyncSent: string = ''

  public snapshot: ClientSnapshot = {
    activeThreadId: '',
    threads: [],
    workspaces: ['E:\\codes\\rust_projects\\a_da'],
    activeWorkspace: 'E:\\codes\\rust_projects\\a_da',
    currentMode: 'code',
    providerConfig: {
      baseUrl: 'https://api.openai.com/v1',
      apiKey: '',
      model: 'gpt-4o',
    },
    approvalMode: 'auto',
    effort: 'medium',
    retryCount: 5,
    running: false,
    runningThreadIds: [],
    queue: [],
    providers: [],
    activeProviderId: '',
  }

  public desktopMode: DesktopMode = 'gateway'

  constructor(
    private url: string = 'ws://127.0.0.1:52353/rpc',
    private token: string = ''
  ) {
    this.initAndConnect()
  }

  private async initAndConnect() {
    const conn = await resolveCoreConnection(this.url, this.token)
    this.url = conn.url
    this.token = conn.token
    if (conn.mode) {
      this.desktopMode = conn.mode
    }
    this.connect()
  }

  public get connected() {
    return this._connected
  }

  public setConnection(url: string, token: string = '') {
    this.url = url
    this.token = token
    if (this.ws) {
      try {
        this.ws.close()
      } catch {}
    }
    this.connect()
  }

  public subscribe(listener: Listener): () => void {
    this.listeners.add(listener)
    listener(this.snapshot)
    return () => {
      this.listeners.delete(listener)
    }
  }

  private notify() {
    for (const listener of this.listeners) {
      listener(this.snapshot)
    }
  }

  public connect() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }

    // W6-T6：代次保护。所有异步回调都带上 `gen`，与当前代次不符就**立即返回**——
    // 否则旧连接（或旧 `resolveCoreConnection`）的回调会覆盖新连接的状态，
    // 造成"两条 socket 并存、消息重复处理"。
    const gen = ++this.connectionGeneration

    // 关掉旧 socket，并**摘掉它的回调**：不摘的话它关闭时会再触发一次 `onclose`
    // → 又排一个重连定时器（这正是"重复连接"的来源之一）。
    if (this.ws) {
      const old = this.ws
      old.onopen = null
      old.onclose = null
      old.onerror = null
      old.onmessage = null
      try {
        old.close()
      } catch {}
      this.ws = null
    }

    try {
      const fullUrl = this.token ? `${this.url}?token=${encodeURIComponent(this.token)}` : this.url
      const socket = new WebSocket(fullUrl)
      this.ws = socket

      socket.onopen = async () => {
        if (gen !== this.connectionGeneration) return // 过期连接，丢弃
        console.log('[AgentWS] WebSocket 已连接:', this.url)
        this._connected = true
        this.reconnectAttempts = 0 // 连上了 → 退避计数清零
        try {
          const platform = getDesktopPlatform()
          const initRes = await this.request('session.initialize', {
            token: this.token,
            protocolVersion: '1.0',
            // 声明增量帧能力：宿主收到才会把后续帧切成 `evt.state.patch`
            // （不声明就继续收全量快照——服务端以此作安全绳/回滚路径）
            // ⚠️ 增量帧暂时关闭（用户实测：开启后点批准/发消息明显变慢）。宿主侧 diff_items/evt.state.patch
            // 已落地并有测试，但客户端合并未过实机验收——不声明能力即回到全量快照的已知正常行为。
            // 重新启用前必须先用合成负载量帧大小与 JS 堆（见 docs/state-patch-plan.md 验收清单）。
            client: { name: `ada-${platform}`, version: '0.1.0', platform },
          })
          if (gen !== this.connectionGeneration) return
          if (initRes && initRes.product) {
            const prod = initRes.product
            this.productInfo = prod
            console.log('[AgentWS] 成功接入产品:', prod.name, `(${prod.id})`)
          }
          const snap = await this.request('session.snapshot', {})
          if (gen !== this.connectionGeneration) return
          if (snap) {
            this.applySnapshot(snap)
          }
        } catch (err) {
          console.error('[AgentWS] 初始化握手失败:', err)
        }
      }

      socket.onmessage = (event) => {
        if (gen !== this.connectionGeneration) return
        try {
          const msg = JSON.parse(event.data)
          this.handleMessage(msg)
        } catch (e) {
          console.error('[AgentWS] 解析消息失败:', e, event.data)
        }
      }

      socket.onclose = () => {
        if (gen !== this.connectionGeneration) return // 过期连接的关闭事件，忽略
        this._connected = false
        // W6-T6：断线立刻失败在途请求，别让调用方等 15s 超时
        this.failAllPending('连接已断开，请求未完成')
        this.notify()
        this.scheduleReconnect()
      }

      socket.onerror = (err) => {
        if (gen !== this.connectionGeneration) return
        console.warn('[AgentWS] 连接异常:', err)
      }
    } catch (err) {
      console.error('[AgentWS] 创建 WebSocket 失败:', err)
      this.failAllPending('连接创建失败，请求未完成')
      this.scheduleReconnect()
    }
  }

  private handleMessage(msg: any) {
    // 响应消息
    if ('id' in msg && msg.id !== null) {
      const pending = this.pendingRequests.get(msg.id)
      if (pending) {
        this.pendingRequests.delete(msg.id)
        if (msg.error) {
          this.recordDebug({
            kind: 'error',
            method: pending.method,
            error: msg.error.message || 'RPC Error',
            payload: msg.error,
          })
          pending.reject(new Error(msg.error.message || 'RPC Error'))
        } else {
          this.recordDebug({
            kind: 'response',
            method: pending.method,
            payload: msg.result,
          })
          pending.resolve(msg.result)
        }
      }
      return
    }

    // 事件通知
    if (msg.method) {
      this.recordDebug({
        kind: msg.method.includes('delta') ? 'delta' : msg.method.includes('item') || msg.method.includes('card') ? 'tool' : 'info',
        method: msg.method,
        payload: msg.params,
      })
      this.handleEvent(msg.method, msg.params)
    }
  }

  private handleEvent(method: string, params: any) {
    if (method === 'evt.state.patch') {
      this.applyPatch(params?.payload || params)
    } else if (method === 'evt.state.snapshot' || method === 'evt.snapshot') {
      const snap = params?.payload || params
      this.applySnapshot(snap)
    } else if (method === 'evt.thread.updated') {
      if (params.threads) this.snapshot.threads = params.threads
      // 服务端这条事件带的是**它认为**的激活会话，可能滞后于用户刚做的选择：
      // 点"新建对话"后它常把旧的激活会话原样发回来，早先这里无条件覆盖，于是新建的页签
      // 永远拿不到焦点（界面停在旧会话上）。本地已有一个仍然有效的激活会话时不覆盖。
      const localActive = this.snapshot.activeThreadId
      const localActiveValid =
        localActive &&
        (localActive.startsWith('thread_tmp_') ||
          this.snapshot.threads.some((t) => t.id === localActive))
      if (params.activeThreadId && !localActiveValid) {
        this.snapshot.activeThreadId = params.activeThreadId
      }
      this.notify()
    } else if (method === 'evt.thread.item') {
      const { threadId, item } = params
      const thread = this.snapshot.threads.find((t) => t.id === threadId)
      if (thread && item) {
        const norm = this.normalizeItem(item)
        const existIdx = thread.items.findIndex((i) => i.id === norm.id)
        if (existIdx >= 0) {
          thread.items[existIdx] = norm
        } else {
          thread.items.push(norm)
        }
        this.notify()
      }
    } else if (method === 'evt.running.state') {
      this.snapshot.running = !!params.running
      this.notify()
    } else if (method === 'evt.queue.updated') {
      const q = Array.isArray(params) ? params : params?.queue || []
      this.snapshot.queue = q
      this.notify()
    }
  }

  /**
   * 增量帧（`evt.state.patch`）：宿主**只推变过的 item**（见 `crates/agent-rpc/src/state/patch.rs`）。
   *
   * 为什么要有：一条长会话 822 个 item、全量帧实测 1.85 MB（合成负载 3.7 MB），
   * 16ms 一帧就是每秒几十 MB 的序列化 + IPC + 解析。增量把稳态每帧压到几十 KB。
   *
   * 合并规则：
   * - `baseSeq` 与本地最后应用的 seq 不连续 → **本地状态可能缺帧**，不猜，直接要一次全量自愈；
   * - 先用 `meta`（与快照同形，只是各会话不带 items）覆盖线程元数据与全部小字段；
   * - 再把 `upserted` 按 `(threadId, itemId)` 合并、按 `removed` 摘掉（否则会留幽灵卡片）。
   */
  private applyPatch(p: any) {
    if (!p) return
    if (typeof p.baseSeq === 'number' && p.baseSeq !== this.lastSnapshotSeq) {
      this.resyncFullSnapshot()
      return
    }

    // 🔴 **patch 只做增删改，绝不做覆盖**。
    //
    // 早先这里为了省事复用了 `applySnapshot(meta)`——而 `meta` 里各会话的 `items` 是**空数组**
    // （宿主刻意不带 items，正是为了省字节）。结果每来一帧增量，本地已有的卡片就被清空一次、
    // 再靠 upserted 一条条补回来：界面反复重建、未变更的卡片整帧丢失。
    // 用户实测就是"点批准/发消息响应非常慢"——这一条是那次止损的根因。
    const meta: any = p.meta || {}
    const { threads: metaThreads, ...metaRest } = meta

    // 1. 小字段（queue/running/log/config/ui/workspace/providers…）整体覆盖：它们本来就小且是权威值
    Object.assign(this.snapshot, metaRest)

    // 2. 线程**元数据**就地更新：已有线程保留自己的 items；新线程补空 items 等着被 upsert 填
    if (Array.isArray(metaThreads)) {
      const byId = new Map<string, any>(this.snapshot.threads.map((t: any) => [t.id, t]))
      const next: any[] = []
      for (const mt of metaThreads) {
        const cur = byId.get(mt.id)
        if (cur) {
          Object.assign(cur, mt, { items: Array.isArray(cur.items) ? cur.items : [] })
          next.push(cur)
        } else {
          next.push({ ...mt, items: [] })
        }
      }
      // 本地乐观创建、服务端还不知道的临时会话不能被这一帧冲掉
      for (const t of this.snapshot.threads) {
        if (t?.id?.startsWith('thread_tmp_') && !next.some((x: any) => x.id === t.id)) next.push(t)
      }
      this.snapshot.threads = next
    }

    // 3. 增删改 item（这是增量帧唯一携带的"大内容"）
    const upserted = Array.isArray(p.upserted) ? p.upserted : []
    for (const u of upserted) {
      if (!u?.item) continue
      const th = this.snapshot.threads.find((t: any) => t.id === u.threadId)
      if (!th) continue
      if (!Array.isArray(th.items)) th.items = []
      const norm = this.normalizeItem(u.item)
      const idx = th.items.findIndex((x: any) => x.id === norm.id)
      if (idx >= 0) th.items[idx] = norm
      else th.items.push(norm)
    }

    const removed = Array.isArray(p.removed) ? p.removed : []
    for (const r of removed) {
      const th = this.snapshot.threads.find((t: any) => t.id === r?.threadId)
      if (th && Array.isArray(th.items)) {
        th.items = th.items.filter((x: any) => x.id !== r.itemId)
      }
    }

    // 4. 激活会话：本地那个仍然有效时不被服务端的滞后值覆盖（与 applySnapshot 同一口径）
    const localActive = this.snapshot.activeThreadId
    const localActiveValid =
      localActive && this.snapshot.threads.some((t: any) => t.id === localActive)
    if (!localActiveValid && meta.activeThreadId) {
      this.snapshot.activeThreadId = meta.activeThreadId
    }

    if (typeof p.seq === 'number') {
      this.lastSnapshotSeq = p.seq
      ;(this.snapshot as any).seq = p.seq
    }
    this.notify()
  }

  /** 增量帧丢了（seq 不连续）时的自愈：要一次全量快照，而不是拿本地状态继续拼。 */
  private resyncFullSnapshot() {
    void this.request('session.snapshot', {})
      .then((snap) => {
        if (snap) this.applySnapshot(snap)
      })
      .catch(() => {
        /* 自愈失败就维持现状，下一次重连还会走全量快照 */
      })
  }

  public applySnapshot(snap: any) {
    if (!snap) return
    const payload = snap.payload || snap
    if (typeof payload.seq === 'number') this.lastSnapshotSeq = payload.seq

    // 替换前的本地线程集合：服务端快照会滞后（本地刚建的会话这一帧不一定带上），
    // 判定"本地激活会话是否还有效"时要把它们算进来，否则会被误判为已失效。
    const localThreadIds = new Set(this.snapshot.threads.map((t) => t.id))

    if (Array.isArray(payload.threads)) {
      // 先摘出本地乐观创建、服务端还不知道的临时会话（`thread_tmp_*`）。
      // 必须在覆盖 `this.snapshot.threads` **之前**取，否则它们已经不在数组里，
      // 后面那段"保留临时会话"就成了永远不会命中的死代码——临时会话被服务端快照
      // 冲掉、`activeThreadId` 却还指着它，界面就会落到别的会话上（看起来"没内容"）。
      const existingThreads = this.snapshot.threads
      const pendingTemps = existingThreads.filter((t) => t.id.startsWith('thread_tmp_'))
      const existingMap = new Map(existingThreads.map((t) => [t.id, t]))
      this.snapshot.threads = payload.threads.map((t: any) => {
        const prev = existingMap.get(t.id)
        const newItemsRaw = Array.isArray(t.items) ? t.items : []

        // 如果前后 items 数量一致，且每一个 item 的属性（id, text, thinking, streaming, tool, status）完全相同，
        // 则保留旧的 items 数组引用，防止未激活的会话因为其它会话吐字导致 items 引用每 16ms 变化一次进而打乱滚动位置！
        let items = prev?.items || []
        const isItemsUnchanged =
          Boolean(prev) &&
          prev!.items.length === newItemsRaw.length &&
          newItemsRaw.every((rawIt: any, idx: number) => {
            const prevIt = prev!.items[idx]
            if (!prevIt) return false
            const rawText = rawIt.text || (rawIt.kind === 'thinking' ? rawIt.text : '')
            const rawThinking = rawIt.thinking || (rawIt.kind === 'thinking' ? rawIt.text : undefined)
            return (
              prevIt.id === rawIt.id &&
              prevIt.text === rawText &&
              prevIt.thinking === rawThinking &&
              prevIt.streaming === rawIt.streaming &&
              (prevIt.tool || (prevIt as any).name) === (rawIt.tool || rawIt.name) &&
              (prevIt as any).status === rawIt.status
            )
          })

        if (!isItemsUnchanged) {
          items = newItemsRaw.map((it: any) => this.normalizeItem(it))
        }

        return {
          id: t.id,
          title: t.title || prev?.title || '新对话',
          workspace: t.workspace || prev?.workspace || this.snapshot.activeWorkspace,
          mode: t.mode || prev?.mode || this.snapshot.currentMode,
          createdAt: t.createdAt || t.created_at || prev?.createdAt || Date.now(),
          updatedAt: t.updatedAt || t.updated_at || prev?.updatedAt || Date.now(),
          parentId: t.parentId ?? t.parent_id ?? prev?.parentId,
          subagentId: t.subagentId ?? t.subagent_id ?? prev?.subagentId,
          isSubagent: Boolean(t.isSubagent ?? t.is_subagent ?? prev?.isSubagent),
          items,
        }
      })

      // 保留本地正在创建中的乐观临时会话，防止在等待 RPC 期间被服务端推送瞬态覆盖
      for (const pt of pendingTemps) {
        if (!this.snapshot.threads.some((t) => t.id === pt.id)) {
          this.snapshot.threads.unshift(pt)
        }
      }
    }

    if (Array.isArray(payload.queue)) {
      this.snapshot.queue = payload.queue
    }

    // 保护客户端正在查看的会话：
    // 只有在本地尚未设置 activeThreadId，或者当前 activeThreadId 在 threads 中已不存在时，才采用服务端推送的 activeThreadId
    //
    // 🔴 判据要把"替换前的本地线程集合"也算进来：快照整体替换 `threads` 之后，本地刚创建、
    // 这一帧还没被带上的会话就"消失了"，只看替换后的集合会把用户的激活选择误判为失效，
    // 从而抹回服务端的旧激活会话（新建的页签因此拿不到焦点）。
    const currentActiveExists =
      this.snapshot.activeThreadId &&
      (this.snapshot.activeThreadId.startsWith('thread_tmp_') ||
        this.snapshot.threads.some((t) => t.id === this.snapshot.activeThreadId) ||
        localThreadIds.has(this.snapshot.activeThreadId))

    if (!currentActiveExists) {
      if (payload.activeThreadId) {
        this.snapshot.activeThreadId = payload.activeThreadId
      } else if (payload.activeId) {
        this.snapshot.activeThreadId = payload.activeId
      } else if (this.snapshot.threads.length > 0) {
        this.snapshot.activeThreadId = this.snapshot.threads[0].id
      }
    }

    // 客户端激活的会话必须与服务端一致：服务端只给"它认为激活的 / 开着标签的 / 运行中的"
    // 会话发 `items`。两边一旦不一致，界面就会把**有历史的会话渲染成空会话**
    // （"新建对话点了没反应"也常常是这个原因——那个判据看的正是 items 长度）。
    // 这里如实把客户端的激活会话回告服务端一次，不一致就纠正（同一 id 只发一次，避免刷屏）。
    if (this.snapshot.activeThreadId === payload.activeThreadId) {
      this.lastActiveSyncSent = ''
    } else if (
      this.snapshot.activeThreadId &&
      !this.snapshot.activeThreadId.startsWith('thread_tmp_') &&
      payload.activeThreadId &&
      this.lastActiveSyncSent !== this.snapshot.activeThreadId
    ) {
      this.lastActiveSyncSent = this.snapshot.activeThreadId
      void this.request('ui.activeThread', { threadId: this.snapshot.activeThreadId }).catch(() => {})
    }

    if (Array.isArray(payload.runningThreadIds)) {
      this.snapshot.runningThreadIds = payload.runningThreadIds
      this.snapshot.running = payload.runningThreadIds.includes(this.snapshot.activeThreadId)
    } else {
      this.snapshot.runningThreadIds = []
      this.snapshot.running = false
    }

    // 正在等待用户操作（审批/提问）的会话：以前服务端发了、这里没接，界面因此看不见
    if (Array.isArray(payload.waitingThreadIds)) {
      this.snapshot.waitingThreadIds = payload.waitingThreadIds
    }

    if (payload.workspace?.project) {
      this.snapshot.activeWorkspace = payload.workspace.project
    }

    if (payload.config) {
      if (payload.config.mode) this.snapshot.currentMode = payload.config.mode
      if (payload.config.approval) this.snapshot.approvalMode = payload.config.approval
      if (payload.config.effort) this.snapshot.effort = payload.config.effort
      if (typeof payload.config.retryCount === 'number') {
        this.snapshot.retryCount = payload.config.retryCount
      }
      if (payload.config.model) {
        this.snapshot.providerConfig = {
          ...this.snapshot.providerConfig,
          model: payload.config.model,
          contextWindow: payload.config.contextWindow,
          supportsImages: payload.config.supportsImages,
        }
      }
    }

    if (Array.isArray(payload.providers)) {
      this.snapshot.providers = payload.providers
    }
    if (payload.activeProviderId) {
      this.snapshot.activeProviderId = payload.activeProviderId
    }

    this.notify()
  }

  private normalizeItem(it: any): Item {
    if (!it) return it
    const kind = it.kind || (it.role === 'user' ? 'user' : 'assistant')
    const role = it.role || (kind === 'user' ? 'user' : 'assistant')

    let question = it.question || it.details?.question
    if (question && typeof question === 'object') {
      const qCallId = question.callId || it.callId || it.id
      let options = Array.isArray(question.options) ? question.options : []
      if (options.length === 0 && Array.isArray(question.choices)) {
        options = question.choices.map((c: any) => ({
          // `id` 要**单独留着**：界面得把它显示在选项前面（用户照着 c1/c2 回话）
          id: typeof c === 'string' ? undefined : c.id || c.value,
          value: typeof c === 'string' ? c : c.id || c.value || c.label || String(c),
          label: typeof c === 'string' ? c : c.label || c.title || String(c),
          description: typeof c === 'string' ? undefined : c.description,
        }))
      }
      question = {
        ...question,
        callId: qCallId,
        options,
      }
    } else if (it.tool === 'ask_user' || it.name === 'ask_user') {
      const parsedArgs = typeof it.args === 'string'
        ? (() => { try { return JSON.parse(it.args) } catch { return {} } })()
        : (it.args || {})
      if (parsedArgs.question) {
        let options = Array.isArray(parsedArgs.options) ? parsedArgs.options : []
        if (options.length === 0 && Array.isArray(parsedArgs.choices)) {
          options = parsedArgs.choices.map((c: any) => ({
            id: typeof c === 'string' ? undefined : c.id || c.value,
            value: typeof c === 'string' ? c : c.id || c.value || c.label || String(c),
            label: typeof c === 'string' ? c : c.label || c.title || String(c),
            description: typeof c === 'string' ? undefined : c.description,
          }))
        }
        question = {
          callId: it.callId || it.id,
          question: parsedArgs.question,
          options,
        }
      }
    }

    let durationMs = it.durationMs ?? it.duration_ms
    let startedAt = it.startedAt ?? it.started_at
    let finishedAt = it.finishedAt ?? it.finished_at
    if (durationMs === undefined && typeof it.output === 'string' && it.output.trim().startsWith('{')) {
      try {
        const parsed = JSON.parse(it.output)
        if (parsed && typeof parsed === 'object') {
          durationMs = parsed.durationMs ?? parsed.duration_ms
          if (startedAt === undefined) startedAt = parsed.startedAt ?? parsed.started_at
          if (finishedAt === undefined) finishedAt = parsed.finishedAt ?? parsed.finished_at
        }
      } catch {}
    }

    return {
      ...it,
      id: it.id || `item_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      kind,
      role,
      at: it.at || it.createdAt || Date.now(),
      createdAt: it.at || it.createdAt || Date.now(),
      text: it.text || (kind === 'thinking' ? it.text : ''),
      thinking: it.thinking || (kind === 'thinking' ? it.text : undefined),
      question,
      durationMs,
      startedAt,
      finishedAt,
    }
  }

  public request<T = any>(method: string, params: Record<string, any> = {}): Promise<T> {
    return new Promise((resolve, reject) => {
      if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
        reject(new Error('WebSocket 尚未连接'))
        return
      }

      const id = this.nextId++
      this.pendingRequests.set(id, { resolve, reject, method })
      this.recordDebug({
        kind: 'request',
        method,
        payload: params,
      })
      this.ws.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }))

      setTimeout(() => {
        if (this.pendingRequests.has(id)) {
          this.pendingRequests.delete(id)
          this.recordDebug({
            kind: 'error',
            method,
            error: `请求超时: ${method}`,
          })
          reject(new Error(`请求超时: ${method}`))
        }
      }, 15000)
    })
  }

  // --- 常用便捷业务调用 ---

  public async sendPrompt(text: string, images?: string[]) {
    // 若当前正在异步创建会话，等待创建完毕获取真实 ID
    if (this.creatingThreadPromise) {
      try {
        const created = await this.creatingThreadPromise
        if (created?.threadId) {
          this.snapshot.activeThreadId = created.threadId
        }
      } catch {
        // 忽略异常，继续后续流程
      }
    }

    let targetThreadId = this.snapshot.activeThreadId
    if (!targetThreadId || targetThreadId.startsWith('thread_tmp_')) {
      if (this.snapshot.threads.length > 0 && !this.snapshot.threads[0].id.startsWith('thread_tmp_')) {
        targetThreadId = this.snapshot.threads[0].id
        this.snapshot.activeThreadId = targetThreadId
      } else {
        const res = await this.createThread()
        if (res?.threadId) {
          targetThreadId = res.threadId
          this.snapshot.activeThreadId = targetThreadId
        }
      }
    }

    const isCurrentlyRunning = (this.snapshot.runningThreadIds || []).includes(targetThreadId)
    let optimisticQueueId: string | null = null

    if (isCurrentlyRunning) {
      // 正在运行：乐观追加至本地 queue，使排队浮动面板立即弹起展示
      optimisticQueueId = `queued_${Date.now()}`
      const queuedItem: QueuedItem = {
        id: optimisticQueueId,
        threadId: targetThreadId,
        text,
        images,
        enqueuedAt: Date.now(),
      }
      this.snapshot.queue = [...(this.snapshot.queue || []), queuedItem]
      this.notify()
    } else {
      // 处于空闲：乐观在会话中追加用户消息，并切换为运行态
      const activeThread = this.snapshot.threads.find((t) => t.id === targetThreadId)
      if (activeThread) {
        const optimisticUserItem: Item = {
          id: `optimistic_${Date.now()}`,
          kind: 'user',
          role: 'user',
          text,
          images,
          at: Date.now(),
          createdAt: Date.now(),
        }
        activeThread.items.push(optimisticUserItem)
      }

      const currentRunning = this.snapshot.runningThreadIds || []
      if (!currentRunning.includes(targetThreadId)) {
        this.snapshot.runningThreadIds = [...currentRunning, targetThreadId]
      }
      this.snapshot.running = this.snapshot.runningThreadIds.includes(this.snapshot.activeThreadId)
      this.notify()
    }

    try {
      return await this.request('thread.send', {
        threadId: targetThreadId,
        text,
        images,
      })
    } catch (err) {
      if (isCurrentlyRunning && optimisticQueueId) {
        this.snapshot.queue = (this.snapshot.queue || []).filter((q) => q.id !== optimisticQueueId)
        this.notify()
      } else {
        this.snapshot.runningThreadIds = (this.snapshot.runningThreadIds || []).filter((id) => id !== targetThreadId)
        this.snapshot.running = this.snapshot.runningThreadIds.includes(this.snapshot.activeThreadId)
        this.notify()
      }
      throw err
    }
  }

  public abortCurrent(targetThreadId?: string) {
    const tid =
      typeof targetThreadId === 'string' && targetThreadId.trim().length > 0
        ? targetThreadId.trim()
        : this.snapshot.activeThreadId
    if (!tid) return
    this.snapshot.runningThreadIds = (this.snapshot.runningThreadIds || []).filter((id) => id !== tid)
    this.snapshot.running = this.snapshot.runningThreadIds.includes(this.snapshot.activeThreadId)
    // 同时也清除属于该会话的本地排队消息
    this.snapshot.queue = (this.snapshot.queue || []).filter((q) => q.threadId && q.threadId !== tid)
    this.notify()
    return this.request('thread.abort', { threadId: tid })
  }

  public async createThread(workspace?: string, mode?: AgentMode, agentId?: string) {
    // 优先采用当前激活会话的上级工作区，确保一个会话严格对应其所属工程目录
    const activeThread = this.snapshot.threads.find((t) => t.id === this.snapshot.activeThreadId)
    const ws = workspace || activeThread?.workspace || this.snapshot.activeWorkspace
    const m = mode || this.snapshot.currentMode
    // 产品只有一个；模式由 mode 表达（早先这里按模式把 agentId 写成 ada-pm，正是双身份的账）
    const resolvedAgentId = agentId || 'ada-coding'

    // 0ms 乐观创建新会话：让界面在点击的瞬间立即定位到新会话界面，杜绝等待 RPC 过程中的界面停滞
    const tempId = `thread_tmp_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`
    const optimisticThread: Thread = {
      id: tempId,
      title: m === 'pm' ? '新 PM 项目会话' : '新对话',
      workspace: ws,
      mode: m,
      agentId: resolvedAgentId,
      items: [],
      createdAt: Date.now(),
      updatedAt: Date.now(),
    }
    this.snapshot.threads.unshift(optimisticThread)
    this.snapshot.activeThreadId = tempId
    this.snapshot.activeWorkspace = ws
    this.snapshot.currentMode = m
    this.snapshot.running = false
    this.notify()

    const createPromise = this.request<{ threadId: string }>('thread.create', {
      workspace: ws,
      mode: m,
      agentId: resolvedAgentId,
    })
    this.creatingThreadPromise = createPromise

    try {
      const res = await createPromise
      if (res?.threadId) {
        // 将临时 ID 平滑替换为服务端真实 ID
        const targetIdx = this.snapshot.threads.findIndex((t) => t.id === tempId)
        if (targetIdx >= 0) {
          this.snapshot.threads[targetIdx].id = res.threadId
        } else if (!this.snapshot.threads.some((t) => t.id === res.threadId)) {
          optimisticThread.id = res.threadId
          this.snapshot.threads.unshift(optimisticThread)
        }
        if (this.snapshot.activeThreadId === tempId) {
          this.snapshot.activeThreadId = res.threadId
        }
        this.snapshot.activeWorkspace = ws
        this.snapshot.currentMode = m
        this.snapshot.running = (this.snapshot.runningThreadIds || []).includes(res.threadId)
        this.notify()
      }
      return res
    } catch (err) {
      // 若创建失败，回滚临时会话
      this.snapshot.threads = this.snapshot.threads.filter((t) => t.id !== tempId)
      if (this.snapshot.activeThreadId === tempId) {
        this.snapshot.activeThreadId = this.snapshot.threads[0]?.id || ''
      }
      this.notify()
      throw err
    } finally {
      if (this.creatingThreadPromise === createPromise) {
        this.creatingThreadPromise = null
      }
    }
  }

  public deleteThread(threadId: string) {
    this.snapshot.threads = this.snapshot.threads.filter((t) => t.id !== threadId)
    this.snapshot.runningThreadIds = (this.snapshot.runningThreadIds || []).filter((id) => id !== threadId)
    if (this.snapshot.activeThreadId === threadId) {
      const nextThread = this.snapshot.threads[0]
      this.snapshot.activeThreadId = nextThread?.id || ''
      if (nextThread?.workspace) {
        this.snapshot.activeWorkspace = nextThread.workspace
      }
    }
    this.snapshot.running = (this.snapshot.runningThreadIds || []).includes(this.snapshot.activeThreadId)
    this.notify()
    return this.request('thread.delete', { threadId })
  }

  public setActiveThread(threadId: string) {
    this.snapshot.activeThreadId = threadId
    this.snapshot.running = (this.snapshot.runningThreadIds || []).includes(threadId)
    const t = this.snapshot.threads.find((th) => th.id === threadId)
    if (t?.workspace) {
      this.snapshot.activeWorkspace = t.workspace
    }
    this.notify()
    return this.request('ui.activeThread', { threadId })
  }

  public editAndResend(threadId: string, itemId: string, text: string, images?: string[]) {
    return this.request('thread.editAndResend', { threadId, itemId, text, images })
  }

  /** 请求重试指定会话最近失败的请求 */
  public async retry(threadId?: string) {
    const tid = threadId || this.snapshot.activeThreadId
    try {
      return await this.request('thread.retry', { threadId: tid })
    } catch (e) {
      console.warn('thread.retry 请求异常，尝试通过最近用户指令降级恢复重试:', e)
      const thread = this.snapshot.threads?.find((t) => t.id === tid)
      const lastUserItem = thread?.items
        ?.slice()
        .reverse()
        .find((it) => it.kind === 'user' || it.role === 'user')
      if (lastUserItem) {
        return await this.editAndResend(
          tid,
          lastUserItem.id,
          lastUserItem.text || '',
          lastUserItem.images
        )
      }
      throw e
    }
  }

  public revertCard(threadId: string, cardId: string) {
    return this.request('change.revertCard', { threadId, cardId })
  }

  public setMode(mode: AgentMode) {
    this.snapshot.currentMode = mode
    this.notify()
    return this.request('thread.setMode', { mode, threadId: this.snapshot.activeThreadId })
  }

  public setProvider(config: ProviderConfig) {
    this.snapshot.providerConfig = config
    this.notify()
    return this.request('config.setProvider', { config })
  }

  // ── 多供应商与模型管理 RPC 接口 ──

  public listProviders() {
    return this.request<{ providers: ProviderEntry[]; activeProviderId: string }>('provider.list', {})
  }

  public saveProvider(provider: ProviderEntry) {
    return this.request<{ ok: boolean; provider: ProviderEntry }>('provider.save', { provider })
  }

  public deleteProvider(id: string) {
    return this.request<{ ok: boolean }>('provider.delete', { id })
  }

  public setActiveProvider(id: string, model?: string) {
    return this.request<{ ok: boolean }>('provider.setActive', { id, model })
  }

  public fetchRemoteModels(params: {
    providerId?: string
    protocol?: ModelProtocol
    baseUrl?: string
    apiKey?: string
    customHeaders?: Record<string, string>
    proxyUrl?: string
  }) {
    return this.request<{ models: ModelEntry[] }>('provider.fetchModels', params)
  }

  public fetchConfig() {
    return this.request<{ saved: Partial<ProviderConfig>; path: string }>('config.get', {})
  }

  public fetchPresets() {
    return this.request<ProviderPreset[]>('config.presets', {})
  }

  public checkProvider(config: ProviderConfig) {
    return this.request<{ message: string }>('config.checkProvider', { config })
  }

  public setApprovalMode(mode: ApprovalMode) {
    this.snapshot.approvalMode = mode
    this.notify()
    return this.request('config.setApproval', { mode })
  }

  public setEffort(effort: Effort) {
    this.snapshot.effort = effort
    this.notify()
    return this.request('config.setEffort', { effort })
  }

  /**
   * 设置「请求重试次数」（模型请求失败后的重试次数，默认 5）。
   *
   * 重试在适配器里按指数退避执行；次数用完即**断开本轮对话**（不再消费排队指令），
   * 错误卡片会说明"已重试 N 次仍失败"，由用户决定改设置还是手动重试。
   */
  public setRetryCount(retries: number) {
    const normalized = Math.max(0, Math.min(20, Math.floor(retries)))
    this.snapshot.retryCount = normalized
    this.notify()
    return this.request<{ retryCount: number }>('config.setRetryCount', { retries: normalized })
  }

  // ── 插件与扩展 RPC 接口 ──

  public fetchPlugins(workspace?: string) {
    return this.request<PluginListResponse>('plugin.list', { workspace: workspace || this.snapshot.activeWorkspace })
  }

  public fetchBuiltinCatalog() {
    return this.request<BuiltinToolInfo[]>('plugin.builtinCatalog', {})
  }

  public togglePlugin(pluginId: string, enabled: boolean) {
    return this.request('plugin.setEnabled', { pluginId, enabled })
  }

  public savePluginConfig(pluginId: string, values: Record<string, unknown>) {
    return this.request('plugin.config.set', { pluginId, values })
  }

  public savePluginSecret(pluginId: string, key: string, value: string) {
    return this.request('plugin.secret.set', { pluginId, key, value })
  }

  public savePluginCapabilities(patch: Partial<PluginCapabilities>) {
    return this.request('plugin.capabilities.set', { patch })
  }

  public deletePlugin(filePath: string, workspace?: string) {
    return this.request<{ ok: boolean }>('plugin.delete', {
      filePath,
      workspace: workspace || this.snapshot.activeWorkspace,
    })
  }

  public createPluginTemplate(params: { scope?: string; name: string; workspace?: string; code?: string }) {
    return this.request<{ filePath: string }>('plugin.createTemplate', {
      scope: params.scope || 'global',
      name: params.name,
      workspace: params.workspace || this.snapshot.activeWorkspace,
      code: params.code,
    })
  }

  public fetchSkills(workspace?: string) {
    return this.request<SkillSummary[]>('skill.list', { workspace: workspace || this.snapshot.activeWorkspace })
  }

  public toggleSkill(id: string, enabled: boolean) {
    return this.request('skill.setEnabled', { id, enabled })
  }

  public answerQuestion(callId: string, choice?: string, text?: string, choices?: string[]) {
    // 多选走 `choices: [...]`；单选仍走 `choice`（后端两条路都收）
    return this.request('question.answer', { callId, choice, text, choices })
  }

  public decideApproval(toolItemId: string, approved: boolean) {
    return this.request('approval.decide', { toolItemId, approved })
  }

  // ── 上下文压缩与优化 ──

  public compactThread(threadId?: string) {
    return this.request<{ success: boolean; reason?: string }>('thread.compact', {
      threadId: threadId || this.snapshot.activeThreadId,
    })
  }

  // ── 会话排队与子代理调度 ──

  public clearQueue(threadId?: string) {
    const tid = threadId || this.snapshot.activeThreadId
    if (tid) {
      this.snapshot.queue = (this.snapshot.queue || []).filter((q) => q.threadId && q.threadId !== tid)
    } else {
      this.snapshot.queue = []
    }
    this.notify()
    return this.request('queue.clear', { threadId: tid })
  }

  public promoteQueueItem(index: number, threadId?: string) {
    if (index > 0 && index < this.snapshot.queue.length) {
      const item = this.snapshot.queue.splice(index, 1)[0]
      this.snapshot.queue.unshift(item)
      this.notify()
    }
    return this.request('queue.promote', { index, threadId: threadId || this.snapshot.activeThreadId })
  }

  public async removeFromQueue(index: number, threadId?: string) {
    if (index >= 0 && index < this.snapshot.queue.length) {
      const removed = this.snapshot.queue.splice(index, 1)[0]
      this.notify()
      try {
        const res = await this.request<{ text: string; images?: string[] } | null>('queue.remove', {
          index,
          threadId: threadId || this.snapshot.activeThreadId,
        })
        return res || { text: removed.text, images: removed.images }
      } catch {
        return { text: removed.text, images: removed.images }
      }
    }
    return this.request<{ text: string; images?: string[] } | null>('queue.remove', {
      index,
      threadId: threadId || this.snapshot.activeThreadId,
    })
  }

  // W6-T1：`resumeSubagent` 已删除。
  //
  // 原因：它调用的 `subagent.resume` 是一个桩（只回 `{threadId, ok:true}`，什么都没恢复），
  // 却先把 `snapshot.running` 置为 true——**界面会显示"正在运行"，而实际什么都没发生**。
  // W4-T6 之后子智能体上下文刻意是临时的（一次性委派，不污染主会话），
  // 因此"恢复"在语义上不存在。协议声明与后端臂已一并删除（R3）。
  //
  // 唯一调用方是 Composer 的"恢复执行"按钮，而它的 `onResumeSubagent` 从来
  // 没有任何父组件传入（点了没反应）——该按钮也已删除。

  // ── 文件系统浏览器与工作区 ──

  public async fetchRoots(): Promise<FsRoot[]> {
    try {
      const res = await this.request<any>('gateway.fs.roots', {})
      if (Array.isArray(res)) return res
      if (res && Array.isArray(res.roots)) return res.roots
      return []
    } catch {
      return this.request<FsRoot[]>('fs.roots', {})
    }
  }

  public async listDirectory(path: string, showHidden = false, limit?: number, directoriesOnly = false): Promise<FsListing> {
    try {
      return await this.request<FsListing>('gateway.fs.listDirectory', {
        path,
        directoriesOnly,
        showHidden,
        limit,
      })
    } catch {
      return await this.request<FsListing>('fs.list', { path, showHidden, limit })
    }
  }

  public async makeDirectory(path: string) {
    try {
      return await this.request<{ path: string }>('gateway.fs.makeDirectory', { path })
    } catch {
      return await this.request<{ path: string }>('fs.mkdir', { path })
    }
  }

  public readFileBase64(path: string) {
    return this.request<{ dataUri: string; path: string }>('fs.read_base64', { path })
  }

  public fetchWorkspaceEntries(workspace?: string) {
    return this.request<string[]>('workspace.entries', { workspace: workspace || this.snapshot.activeWorkspace })
  }

  public setActiveProject(workspace: string) {
    this.snapshot.activeWorkspace = workspace
    this.notify()
    // 若在网关模式下，通知网关 attach/记录 该工作区
    void this.request('gateway.attach', { workspace }).catch(() => {})
    return this.request('ui.activeProject', { workspace })
  }

  public async listRecentWorkspaces() {
    try {
      return await this.request<{ workspaces: Array<{ workspace: string; name: string; lastAccessedAt: number }>; activeWorkspace?: string }>('gateway.workspaces.list', {})
    } catch {
      return { workspaces: [] }
    }
  }

  public async removeWorkspace(workspace: string) {
    if (!workspace) return { ok: false }
    // 优先通知网关移除
    try {
      await this.request('gateway.workspaces.remove', { workspace })
    } catch {
      // ignore
    }
    const res = await this.request<{ message?: string; error?: string }>('workspace.remove', { workspace })
    if (res?.message) {
      if (
        res.message.includes('至少保留') ||
        res.message.includes('正在运行') ||
        res.message.includes('不能移除')
      ) {
        throw new Error(res.message)
      }
    }
    // 本地同步更新 snapshot: 过滤掉该工作区下的所有会话
    this.snapshot.threads = this.snapshot.threads.filter((t) => t.workspace !== workspace)
    // 更新活跃工作区
    if (this.snapshot.activeWorkspace === workspace) {
      const nextThread = this.snapshot.threads[0]
      if (nextThread?.workspace) {
        this.snapshot.activeWorkspace = nextThread.workspace
      }
      if (nextThread?.id) {
        this.snapshot.activeThreadId = nextThread.id
      }
    }
    this.notify()
    return res
  }

  // ── 改动审查与代码撤销 ──

  public revertFile(threadId: string, path: string) {
    return this.request<{ ok: boolean }>('change.revertFile', { threadId, path })
  }

  public revertAllChanges(threadId: string) {
    return this.request<{ ok: boolean }>('change.revertAll', { threadId })
  }

  // ── 提示词与快捷指令模板 ──

  public fetchPrompts(workspace?: string) {
    return this.request<PromptItem[]>('prompt.list', { workspace: workspace || this.snapshot.activeWorkspace })
  }

  // ── 特化子智能体 Profile ──

  public fetchSubagentProfiles(workspace?: string) {
    return this.request<SubagentProfile[]>('subagentProfile.list', { workspace: workspace || this.snapshot.activeWorkspace })
  }

  // ── 会话修改（重命名等） ──

  public updateThreadTitle(threadId: string, title: string) {
    const thread = this.snapshot.threads.find((t) => t.id === threadId)
    if (thread) {
      thread.title = title
      this.notify()
    }
    return this.request('thread.update', { threadId, title })
  }
}

export const agentClient = new AgentWebSocketClient()
