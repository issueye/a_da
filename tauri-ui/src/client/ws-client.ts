/**
 * JSON-RPC 2.0 WebSocket 客户端。
 * 负责与本地 agent_core (或 a-da host) 建立安全双向隧道。
 * 支持在 Tauri 环境下通过 get_core_info 动态发现同进程分配的端口与认证令牌，
 * 在非 Tauri 环境下无缝回落至默认端口。
 */

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
} from '../types'

export type Listener = (snapshot: ClientSnapshot) => void

interface CoreInfo {
  alive: boolean
  port: number
  token: string
  url: string
}

/**
 * 探测并获取本地核心服务的连接配置
 */
async function resolveCoreConnection(
  defaultUrl = 'ws://127.0.0.1:52353/rpc',
  defaultToken = ''
): Promise<{ url: string; token: string }> {
  try {
    const isTauri = typeof window !== 'undefined' && Boolean((window as any).__TAURI_INTERNALS__ || (window as any).__TAURI__)
    if (isTauri) {
      const { invoke } = await import('@tauri-apps/api/core')
      const info = await invoke<CoreInfo>('get_core_info')
      if (info && info.alive && info.port > 0) {
        return {
          url: info.url || `ws://127.0.0.1:${info.port}/rpc`,
          token: info.token || '',
        }
      }
    }
  } catch (err) {
    console.warn('[AgentWS] 获取 Tauri 核心服务信息失败，回退到默认端口:', err)
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

  public debugLogs: DebugEntry[] = []
  private debugListeners = new Set<(logs: DebugEntry[]) => void>()

  public recordDebug(entry: Omit<DebugEntry, 'id' | 'at'>) {
    const item: DebugEntry = {
      ...entry,
      id: `dbg_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`,
      at: Date.now(),
    }
    this.debugLogs.unshift(item)
    if (this.debugLogs.length > 300) {
      this.debugLogs.pop()
    }
    for (const listener of this.debugListeners) {
      listener([...this.debugLogs])
    }
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
    running: false,
    queue: [],
    providers: [],
    activeProviderId: '',
  }

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

    try {
      const fullUrl = this.token ? `${this.url}?token=${encodeURIComponent(this.token)}` : this.url
      this.ws = new WebSocket(fullUrl)

      this.ws.onopen = async () => {
        console.log('[AgentWS] WebSocket 已连接:', this.url)
        this._connected = true
        try {
          await this.request('session.initialize', {
            token: this.token,
            protocolVersion: '1.0',
            client: { name: 'a-da-tauri', version: '0.1.0', platform: 'tauri' },
          })
          const snap = await this.request('session.snapshot', {})
          if (snap) {
            this.applySnapshot(snap)
          }
        } catch (err) {
          console.error('[AgentWS] 初始化握手失败:', err)
        }
      }

      this.ws.onmessage = (event) => {
        try {
          const msg = JSON.parse(event.data)
          this.handleMessage(msg)
        } catch (e) {
          console.error('[AgentWS] 解析消息失败:', e, event.data)
        }
      }

      this.ws.onclose = () => {
        this._connected = false
        this.notify()
        this.reconnectTimer = setTimeout(async () => {
          const conn = await resolveCoreConnection(this.url, this.token)
          this.url = conn.url
          this.token = conn.token
          this.connect()
        }, 2000)
      }

      this.ws.onerror = (err) => {
        console.warn('[AgentWS] 连接异常:', err)
      }
    } catch (err) {
      console.error('[AgentWS] 创建 WebSocket 失败:', err)
      this.reconnectTimer = setTimeout(async () => {
        const conn = await resolveCoreConnection(this.url, this.token)
        this.url = conn.url
        this.token = conn.token
        this.connect()
      }, 2000)
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
    if (method === 'evt.state.snapshot' || method === 'evt.snapshot') {
      const snap = params?.payload || params
      this.applySnapshot(snap)
    } else if (method === 'evt.thread.updated') {
      if (params.threads) this.snapshot.threads = params.threads
      if (params.activeThreadId) this.snapshot.activeThreadId = params.activeThreadId
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

  public applySnapshot(snap: any) {
    if (!snap) return
    const payload = snap.payload || snap

    if (Array.isArray(payload.threads)) {
      this.snapshot.threads = payload.threads.map((t: any) => ({
        id: t.id,
        title: t.title || '新对话',
        workspace: t.workspace || this.snapshot.activeWorkspace,
        mode: t.mode || this.snapshot.currentMode,
        createdAt: t.createdAt || t.created_at || Date.now(),
        updatedAt: t.updatedAt || t.updated_at || Date.now(),
        parentId: t.parentId ?? t.parent_id,
        subagentId: t.subagentId ?? t.subagent_id,
        isSubagent: Boolean(t.isSubagent ?? t.is_subagent),
        items: Array.isArray(t.items) ? t.items.map((it: any) => this.normalizeItem(it)) : [],
      }))
    }

    if (Array.isArray(payload.queue)) {
      this.snapshot.queue = payload.queue
    }

    if (payload.activeThreadId) {
      this.snapshot.activeThreadId = payload.activeThreadId
    } else if (payload.activeId) {
      this.snapshot.activeThreadId = payload.activeId
    } else if (!this.snapshot.activeThreadId && this.snapshot.threads.length > 0) {
      this.snapshot.activeThreadId = this.snapshot.threads[0].id
    }

    if (Array.isArray(payload.runningThreadIds)) {
      this.snapshot.running =
        payload.runningThreadIds.includes(this.snapshot.activeThreadId) ||
        payload.runningThreadIds.length > 0
    }

    if (payload.workspace?.project) {
      this.snapshot.activeWorkspace = payload.workspace.project
    }

    if (payload.config) {
      if (payload.config.mode) this.snapshot.currentMode = payload.config.mode
      if (payload.config.approval) this.snapshot.approvalMode = payload.config.approval
      if (payload.config.effort) this.snapshot.effort = payload.config.effort
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

    return {
      ...it,
      id: it.id || `item_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      kind,
      role,
      at: it.at || it.createdAt || Date.now(),
      createdAt: it.at || it.createdAt || Date.now(),
      text: it.text || (kind === 'thinking' ? it.text : ''),
      thinking: it.thinking || (kind === 'thinking' ? it.text : undefined),
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
    let targetThreadId = this.snapshot.activeThreadId
    if (!targetThreadId) {
      if (this.snapshot.threads.length > 0) {
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

    // 乐观更新：在客户端立即追加一条用户消息，保证界面瞬间变化
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

    this.snapshot.running = true
    this.notify()

    try {
      return await this.request('thread.send', {
        threadId: targetThreadId,
        text,
        images,
      })
    } catch (err) {
      this.snapshot.running = false
      this.notify()
      throw err
    }
  }

  public abortCurrent() {
    if (!this.snapshot.activeThreadId) return
    this.snapshot.running = false
    this.notify()
    return this.request('thread.abort', { threadId: this.snapshot.activeThreadId })
  }

  public async createThread(workspace?: string, mode?: AgentMode) {
    // 优先采用当前激活会话的上级工作区，确保一个会话严格对应其所属工程目录
    const activeThread = this.snapshot.threads.find((t) => t.id === this.snapshot.activeThreadId)
    const ws = workspace || activeThread?.workspace || this.snapshot.activeWorkspace
    const m = mode || this.snapshot.currentMode
    const res = await this.request<{ threadId: string }>('thread.create', { workspace: ws, mode: m })
    if (res?.threadId) {
      this.snapshot.activeThreadId = res.threadId
      this.snapshot.activeWorkspace = ws
      // 服务端的即时快照可能先于本次响应抵达（id 已在列表中）。
      // 此时以服务端数据为准，避免同一个会话在本地出现两份、被分到两个工作区组里
      if (!this.snapshot.threads.some((t) => t.id === res.threadId)) {
        const newThread: Thread = {
          id: res.threadId,
          title: '新对话',
          workspace: ws,
          mode: m,
          items: [],
          createdAt: Date.now(),
          updatedAt: Date.now(),
        }
        this.snapshot.threads.unshift(newThread)
      }
      this.notify()
    }
    return res
  }

  public deleteThread(threadId: string) {
    this.snapshot.threads = this.snapshot.threads.filter((t) => t.id !== threadId)
    if (this.snapshot.activeThreadId === threadId) {
      const nextThread = this.snapshot.threads[0]
      this.snapshot.activeThreadId = nextThread?.id || ''
      if (nextThread?.workspace) {
        this.snapshot.activeWorkspace = nextThread.workspace
      }
    }
    this.notify()
    return this.request('thread.delete', { threadId })
  }

  public setActiveThread(threadId: string) {
    this.snapshot.activeThreadId = threadId
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

  public answerQuestion(callId: string, choice?: string, text?: string) {
    return this.request('question.answer', { callId, choice, text })
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
    this.snapshot.queue = []
    this.notify()
    return this.request('queue.clear', { threadId: threadId || this.snapshot.activeThreadId })
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

  public resumeSubagent(subagentThreadId: string, instruction?: string) {
    this.snapshot.running = true
    this.notify()
    return this.request<{ threadId: string }>('subagent.resume', {
      subagentThreadId,
      instruction,
    })
  }

  // ── 文件系统浏览器与工作区 ──

  public fetchRoots() {
    return this.request<FsRoot[]>('fs.roots', {})
  }

  public listDirectory(path: string, showHidden = false, limit?: number) {
    return this.request<FsListing>('fs.list', { path, showHidden, limit })
  }

  public makeDirectory(path: string) {
    return this.request<{ path: string }>('fs.mkdir', { path })
  }

  public fetchWorkspaceEntries(workspace?: string) {
    return this.request<string[]>('workspace.entries', { workspace: workspace || this.snapshot.activeWorkspace })
  }

  public setActiveProject(workspace: string) {
    this.snapshot.activeWorkspace = workspace
    this.notify()
    return this.request('ui.activeProject', { workspace })
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
