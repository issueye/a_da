/**
 * Agent 运行时与桌面 UI 状态层
 *
 * 参考 @earendil-works/pi-agent-core 与 @earendil-works/pi-coding-agent 架构设计。
 * 模型侧的一切（多轮循环、工具调度、事件流）都在 src/agent/core 里，这一层只做
 * 三件事：把事件翻译成界面卡片、把审批闸门挂在 beforeToolCall 上、把消息追加到
 * 会话 JSONL。
 *
 * 界面层没有状态管理库：store 是模块级单例，异步轮次直接改它，React 通过
 * subscribe 收到通知后重渲染，所以流式回复在到达过程中就是对的。
 */

import {
  configPath,
  readLlmConfig,
  testConnection,
  writeSavedConfig,
  type ProviderConfig,
} from './config'
import { runAgentLoop } from './core/agent-loop'
import type {
  AgentMessage,
  BeforeToolCallContext,
  BeforeToolCallResult,
  ToolCallBlock,
} from './core/types'
import {
  describeTool,
  isWriteTool,
  resolveProjectPath,
  runTool,
  scanWorkspace,
  defaultToolRegistry,
  defaultExtensionLoader,
} from './tools'
import { defaultSessionManager } from './session/manager'
import type { DebugEntry, Item, Thread } from './types'

export type ApprovalMode = 'auto' | 'ask' | 'readonly'
export type Effort = 'max' | 'high' | 'medium' | 'low'

export const APPROVAL_OPTIONS: { value: ApprovalMode; label: string }[] = [
  { value: 'auto', label: '自动批准' },
  { value: 'ask', label: '每次询问' },
  { value: 'readonly', label: '只读' },
]

export const EFFORT_OPTIONS: { value: Effort; label: string }[] = [
  { value: 'max', label: '最高' },
  { value: 'high', label: '高' },
  { value: 'medium', label: '中' },
  { value: 'low', label: '低' },
]

const EFFORT_VALUE: Record<Effort, string> = {
  max: 'high',
  high: 'high',
  medium: 'medium',
  low: 'low',
}

const MAX_STEPS = 24
const MAX_LOG = 120

/** 拒绝后回给模型的说明：说清楚行为，而不是只报一个 no。 */
const DENIED_REASON = '用户拒绝了这次调用。不要重试同样的调用，先说明原因或换一种做法。'

let counter = 0
const nextId = (prefix: string) => `${prefix}_${Date.now().toString(36)}_${++counter}`

function titleFrom(text: string): string {
  const line = text.trim().split('\n')[0]!.trim()
  return line.length > 24 ? `${line.slice(0, 24)}…` : line || '新会话'
}

function makeThread(workspace: string): Thread {
  const id = nextId('thread')
  // 异步在会话管理器中注册
  void defaultSessionManager.createSession(id, workspace, '新会话').catch(() => {})
  return {
    id,
    title: '新会话',
    createdAt: Date.now(),
    workspace,
    items: [],
    messages: [],
  }
}

type ToolCard = Extract<Item, { kind: 'tool' }>

export class AgentStore {
  threads: Thread[] = []
  activeId: string
  running = false
  approval: ApprovalMode = 'auto'
  effort: Effort = 'max'
  debugOpen = false
  settingsOpen = false
  log: DebugEntry[] = []
  /** Scan of the active project only; other projects are counted when opened. */
  workspaceInfo: { files: number; dirs: number; scanning: boolean } = {
    files: 0,
    dirs: 0,
    scanning: true,
  }
  /** Top-level project names, used by the composer's `+` picker. */
  entries: string[] = []

  private listeners = new Set<() => void>()
  private approvals = new Map<string, (approved: boolean) => void>()
  /** 本轮每张工具卡片，按调用 id 找回去更新状态。 */
  private cards = new Map<string, ToolCard>()
  private queue: { thread: Thread; text: string; item: Item }[] = []
  private abort: AbortController | null = null
  /** 正在跑的那一轮属于哪个会话：它不能在自己运行的时候被删掉。 */
  private runningThreadId: string | null = null
  /** 最近一次工作区扩展加载：跑一轮之前要等它，工具表才完整。 */
  private extensionsReady: Promise<void> = Promise.resolve()
  private notifyTimer: ReturnType<typeof setTimeout> | null = null
  private logId = 0

  constructor(workspace: string) {
    defaultExtensionLoader.bindHost((msg) => this.trace(msg))
    const thread = makeThread(workspace)
    this.threads = [thread]
    this.activeId = thread.id
    void this.refresh()
  }

  // ---------------------------------------------------------------- react glue

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
      if (!this.listeners.size && this.notifyTimer) {
        clearTimeout(this.notifyTimer)
        this.notifyTimer = null
      }
    }
  }

  get active(): Thread {
    return this.threads.find((thread) => thread.id === this.activeId) ?? this.threads[0]!
  }

  /** The project the window is showing. A thread is pinned to one for its whole life. */
  get project(): string {
    return this.active.workspace
  }

  /**
   * Every workspace an open thread points at, newest first. A project exists
   * exactly as long as it has a thread, so this needs no second store.
   */
  get projects(): string[] {
    const newest = new Map<string, number>()
    for (const thread of this.threads) {
      const seen = newest.get(thread.workspace)
      if (seen === undefined || thread.createdAt > seen) newest.set(thread.workspace, thread.createdAt)
    }
    return [...newest.entries()].sort((a, b) => b[1] - a[1]).map(([path]) => path)
  }

  /** Threads of the open project. `threads` is newest first already. */
  get projectThreads(): Thread[] {
    return this.threads.filter((thread) => thread.workspace === this.project)
  }

  private notify(): void {
    for (const listener of [...this.listeners]) listener()
  }

  /** Text arrives in many small deltas; one repaint per 40ms reads better than one per token. */
  private notifySoon(): void {
    if (this.notifyTimer) return
    this.notifyTimer = setTimeout(() => {
      this.notifyTimer = null
      this.notify()
    }, 40)
  }

  // ------------------------------------------------------------------- setters

  setApproval(mode: ApprovalMode): void {
    this.approval = mode
    this.notify()
  }

  setEffort(effort: Effort): void {
    this.effort = effort
    this.notify()
  }

  toggleDebug(): void {
    this.debugOpen = !this.debugOpen
    this.notify()
  }

  /** Append a line to the debug log from anywhere in the UI. */
  trace(text: string): void {
    this.push({ kind: 'info', text })
    this.notify()
  }

  setSettings(open: boolean): void {
    this.settingsOpen = open
    this.notify()
  }

  /** Save the provider block and say where it landed, so the UI can confirm. */
  async saveProvider(config: ProviderConfig): Promise<string | null> {
    try {
      await writeSavedConfig(config)
    } catch (error) {
      const message = `保存失败：${(error as Error).message}`
      this.push({ kind: 'error', text: message })
      this.notify()
      return message
    }
    this.push({ kind: 'info', text: `已保存供应商设置到 ${configPath()}` })
    this.notify()
    return null
  }

  /** One round trip to the endpoint, reported through the debug log as well. */
  async checkProvider(config: ProviderConfig): Promise<string> {
    this.push({ kind: 'info', text: `测试连接 ${config.baseUrl}` })
    const result = await testConnection(config)
    this.push({ kind: result.ok ? 'info' : 'error', text: result.detail })
    this.notify()
    return result.detail
  }

  // ------------------------------------------------------------------- threads

  /** A new conversation in the open project, unless another one is named. */
  newThread(workspace: string = this.project): Thread {
    const thread = makeThread(workspace)
    this.threads = [thread, ...this.threads]
    this.activeId = thread.id
    this.push({ kind: 'info', text: `新建会话 · ${workspace}` })
    void this.refresh()
    this.notify()
    return thread
  }

  /** Open a project: its newest thread, or a fresh one the first time. */
  selectProject(workspace: string): void {
    if (workspace === this.project) {
      void this.refresh()
      return
    }
    const existing = this.threads.find((thread) => thread.workspace === workspace)
    if (existing) {
      this.activeId = existing.id
      this.push({ kind: 'info', text: `切换到项目 ${workspace}` })
      void this.refresh()
      this.notify()
      return
    }
    this.newThread(workspace)
  }

  /**
   * Add a project from a path the user typed. The path is checked before it
   * joins the list, and the message is what the sidebar shows on failure.
   */
  async addProject(path: string): Promise<string | null> {
    const result = await resolveProjectPath(path)
    if ('error' in result) return result.error
    this.newThread(result.path)
    return null
  }

  selectThread(id: string): void {
    if (this.activeId === id) return
    this.activeId = id
    this.notify()
  }

  /**
   * 删掉一个会话：从列表里去掉，盘上的流水也一起删掉。
   *
   * 项目是靠会话存在的，所以删掉某个工作区的最后一个会话时会补一个新的空会话，
   * 而不是让工作区从侧边栏消失——用户删的是一个会话，不是一个工作区。
   * 正在跑的那一轮不能删：它还在往这个会话里写。
   *
   * @returns 出错时返回要显示给用户的理由，成功返回 null。
   */
  deleteThread(id: string): string | null {
    const thread = this.threads.find((candidate) => candidate.id === id)
    if (!thread) return null
    if (this.runningThreadId === id) return '这个会话正在运行，先停止再删除'

    this.threads = this.threads.filter((candidate) => candidate.id !== id)
    this.queue = this.queue.filter((item) => item.thread.id !== id)
    this.push({ kind: 'info', text: `已删除会话「${thread.title}」` })
    void defaultSessionManager.deleteSession(id).catch(() => {})

    if (this.activeId === id) {
      const next = this.threads.find((candidate) => candidate.workspace === thread.workspace)
      if (next) {
        this.activeId = next.id
      } else {
        const fresh = makeThread(thread.workspace)
        this.threads = [fresh, ...this.threads]
        this.activeId = fresh.id
      }
      void this.refresh()
    }

    this.notify()
    return null
  }

  // ------------------------------------------------------------------ messages

  send(text: string): void {
    const prompt = text.trim()
    if (!prompt) return
    const thread = this.active
    if (thread.title === '新会话') {
      thread.title = titleFrom(prompt)
      void defaultSessionManager.updateSessionTitle(thread.id, thread.title).catch(() => {})
    }
    if (this.running) {
      const item: Item = { kind: 'user', id: nextId('item'), at: Date.now(), text: prompt, queued: true }
      thread.items.push(item)
      this.queue.push({ thread, text: prompt, item })
      this.push({ kind: 'info', text: `已排队第 ${this.queue.length} 条后续指令` })
      this.notify()
      return
    }
    thread.items.push({ kind: 'user', id: nextId('item'), at: Date.now(), text: prompt })
    this.queue.push({ thread, text: prompt, item: thread.items[thread.items.length - 1]! })
    void this.drain()
  }

  stop(): void {
    if (!this.running) return
    this.abort?.abort()
    this.push({ kind: 'info', text: '用户停止了本轮' })
    this.notify()
  }

  decide(toolItemId: string, approved: boolean): void {
    const resolve = this.approvals.get(toolItemId)
    if (!resolve) return
    this.approvals.delete(toolItemId)
    resolve(approved)
  }

  /** Re-scan the workspace so the sidebar can show what the agent sees. */
  async refresh(): Promise<void> {
    this.workspaceInfo = { ...this.workspaceInfo, scanning: true }
    this.notify()

    // 扩展先加载、扫描后跑：扫描是这里最慢的一步（几千个文件），而工具表在下一轮
    // 开始前就得齐——这两件事原来反着来，第一轮会漏掉扩展工具。
    this.extensionsReady = defaultExtensionLoader
      .autoLoadExtensions(this.project)
      .then((loaded) => {
        if (loaded.length > 0) {
          this.push({ kind: 'info', text: `已加载扩展工具：${loaded.join(', ')}` })
        }
      })
      .catch(() => {})

    try {
      const info = await scanWorkspace(this.project)
      this.workspaceInfo = { files: info.files, dirs: info.dirs, scanning: false }
      this.entries = info.entries
      this.push({ kind: 'info', text: `已索引 ${info.files} 个文件` })
    } catch (error) {
      this.workspaceInfo = { ...this.workspaceInfo, scanning: false }
      this.push({ kind: 'error', text: `索引工作区失败：${(error as Error).message}` })
    }
    this.notify()
  }

  // --------------------------------------------------------------------- loop

  private push(entry: { kind: DebugEntry['kind']; text: string }): void {
    this.logId += 1
    this.log = [...this.log.slice(-MAX_LOG), { id: this.logId, at: Date.now(), ...entry }]
  }

  /** 会话 JSONL 是追加写的流水账，写不进去也不该打断这一轮。 */
  private persist(threadId: string, message: AgentMessage): void {
    void defaultSessionManager.appendMessage(threadId, message).catch(() => {})
  }

  private fail(thread: Thread, text: string): void {
    thread.items.push({ kind: 'notice', id: nextId('item'), at: Date.now(), text, level: 'error' })
    this.push({ kind: 'error', text })
    this.notify()
  }

  private async drain(): Promise<void> {
    if (this.running) return
    this.running = true
    this.notify()
    try {
      while (this.queue.length) {
        const next = this.queue.shift()!
        this.runningThreadId = next.thread.id
        await this.turn(next.thread, next.text)
      }
    } catch (error) {
      if ((error as Error).name !== 'AbortError') {
        this.fail(this.active, `运行失败：${(error as Error).message ?? String(error)}`)
      }
    } finally {
      this.running = false
      this.runningThreadId = null
      this.abort = null
      this.notify()
    }
  }

  /**
   * 一轮对话：消息交给 core 的循环，这里只把事件流翻译成界面状态。
   *
   * 与模型来回的完整历史由循环维护（它会在 agent_end 交还整份 messages），
   * 所以 thread.messages 永远是真正发出去过的那份。
   */
  private async turn(thread: Thread, prompt: string): Promise<void> {
    const config = await readLlmConfig()
    if (!config) {
      await this.offlineTurn(thread, prompt)
      return
    }
    this.push({ kind: 'request', text: `${config.model} @ ${config.baseUrl}（${config.source}）` })

    const userMessage: AgentMessage = { role: 'user', content: prompt, timestamp: Date.now() }
    thread.messages.push(userMessage)
    this.persist(thread.id, userMessage)

    const controller = new AbortController()
    this.abort = controller
    this.cards.clear()

    // 扩展得先注册完，这一轮的工具表才不会漏掉它们（刚启动就开始打字也不会漏）。
    await this.extensionsReady

    // 每轮重新问注册中心要一次工具：刚加载的扩展工具这一轮就要能被模型看见。
    const tools = defaultToolRegistry.getToolsForWorkspace(thread.workspace)
    // 助手行和思考行都等到第一段真的到了才建：思考先行，所以思考行会排在回答上
    // 面；只调工具、不说一句话的那一轮则一行都不留（和以前一样什么都不显示）。
    let assistant: Extract<Item, { kind: 'assistant' }> | null = null
    let reasoning: Extract<Item, { kind: 'thinking' }> | null = null
    /** 思考结束的时刻：回答开始、或这一条消息收尾时定下来。 */
    const endReasoning = (): void => {
      if (reasoning && reasoning.endedAt === undefined) reasoning.endedAt = Date.now()
    }

    const loop = runAgentLoop(thread.messages, config, {
      tools,
      effort: EFFORT_VALUE[this.effort],
      maxSteps: MAX_STEPS,
      // 顺序执行：审批一次只该问一件事，命令之间也不该互相抢工作目录。
      toolExecution: 'sequential',
      signal: controller.signal,
      beforeToolCall: (context: BeforeToolCallContext) => this.gate(thread, context.toolCall),
    })

    for await (const event of loop) {
      // 扩展脚本订阅了生命周期点位，事件原样转发一份
      defaultExtensionLoader.dispatchAgentEvent(event)

      switch (event.type) {
        case 'message_start':
          if (event.message.role === 'assistant') {
            assistant = null
            reasoning = null
          }
          break

        case 'message_update':
          if (event.delta.thinking) {
            if (!reasoning) {
              reasoning = { kind: 'thinking', id: nextId('item'), at: Date.now(), text: '' }
              thread.items.push(reasoning)
            }
            reasoning.text += event.delta.thinking
            this.notifySoon()
          }
          if (event.delta.text) {
            endReasoning()
            if (!assistant) {
              assistant = {
                kind: 'assistant',
                id: nextId('item'),
                at: Date.now(),
                text: '',
                streaming: true,
              }
              thread.items.push(assistant)
            }
            assistant.text += event.delta.text
            this.notifySoon()
          }
          break

        case 'message_end': {
          const message = event.message
          if (message.role !== 'assistant') break
          endReasoning()
          if (assistant) assistant.streaming = false
          if (message.stopReason === 'error') {
            this.fail(thread, `模型请求失败：${message.errorMessage ?? '未知错误'}`)
          } else if (message.content.trim()) {
            this.persist(thread.id, message)
          }
          if (message.toolCalls?.length) {
            this.push({ kind: 'tools', text: message.toolCalls.map((call) => call.name).join(', ') })
          }
          this.notify()
          break
        }

        case 'tool_execution_start':
          this.setCardStatus(event.toolCallId, 'running')
          break

        case 'tool_execution_update': {
          const card = this.cards.get(event.toolCallId)
          if (card) {
            card.output = event.partialResult.output
            this.notifySoon()
          }
          break
        }

        case 'tool_execution_end':
          this.finishToolCall(thread, event.toolCallId, event.result)
          break

        case 'agent_end':
          thread.messages = event.messages
          if (event.reason === 'max_steps') {
            thread.items.push({
              kind: 'notice',
              id: nextId('item'),
              at: Date.now(),
              text: `达到单轮 ${MAX_STEPS} 步上限，已停下。可以继续输入让它接着做。`,
              level: 'info',
            })
          }
          this.notify()
          break
      }
    }
    this.cards.clear()
  }

  /** No API key configured: exercise the tool loop anyway so the UI still works. */
  private async offlineTurn(thread: Thread, prompt: string): Promise<void> {
    thread.items.push({
      kind: 'notice',
      id: nextId('item'),
      at: Date.now(),
      text: `未配置模型接口。在侧边栏底部打开「设置」填写供应商，或设置 A_DA_API_KEY 与 A_DA_MODEL（可选 A_DA_BASE_URL），或写入 ${configPath()}。当前运行在离线模式。`,
      level: 'error',
    })
    this.notify()

    // 离线也要真的跑一次工具：沙箱和界面都被走通了，而不是只留一句说明。
    await this.runToolDirect(thread, {
      id: nextId('call'),
      name: 'list_files',
      arguments: { depth: 2 },
      rawArguments: '{"depth":2}',
    })

    const info = this.workspaceInfo
    thread.items.push({
      kind: 'assistant',
      id: nextId('item'),
      at: Date.now(),
      text: `【离线模式】我扫描了项目 \`${thread.workspace}\`：${info.files} 个文件、${info.dirs} 个目录。配置模型接口后，我会按你的任务在这个目录里读写文件、执行命令。`,
    })
    thread.messages.push({ role: 'user', content: prompt, timestamp: Date.now() })
    thread.messages.push({ role: 'assistant', content: '离线模式：仅扫描工作区，未调用模型。', timestamp: Date.now() })
    this.notify()
  }

  private needsApproval(name: string): boolean {
    if (this.approval === 'ask') return true
    if (this.approval === 'readonly') return isWriteTool(name)
    return false
  }

  /**
   * 审批闸门：卡片在这里出现，等待也在这里发生。
   *
   * 挂在 beforeToolCall 上，所以拒绝走的不是「工具执行失败」而是 block——循环会把
   * reason 当成工具结果回给模型，模型知道是被拒绝了，不会以为调用成功了。
   */
  private async gate(thread: Thread, call: ToolCallBlock): Promise<BeforeToolCallResult | undefined> {
    const card: ToolCard = {
      kind: 'tool',
      id: nextId('item'),
      at: Date.now(),
      callId: call.id,
      name: call.name,
      args: call.arguments,
      rawArgs: call.rawArguments,
      status: this.needsApproval(call.name) ? 'awaiting' : 'running',
    }
    thread.items.push(card)
    this.cards.set(call.id, card)
    this.notify()

    if (card.status !== 'awaiting') return undefined

    const approved = await new Promise<boolean>((resolve) => this.approvals.set(card.id, resolve))
    if (approved) return undefined

    card.status = 'denied'
    card.output = '已拒绝执行'
    this.cards.delete(call.id)
    this.persist(thread.id, {
      role: 'toolResult',
      toolCallId: call.id,
      toolName: call.name,
      content: DENIED_REASON,
      isError: true,
      timestamp: Date.now(),
    })
    this.push({ kind: 'tool', text: `${call.name} 被拒绝` })
    this.notify()
    return { block: true, reason: DENIED_REASON }
  }

  private setCardStatus(callId: string, status: ToolCard['status']): void {
    const card = this.cards.get(callId)
    if (!card || card.status === status) return
    card.status = status
    this.notify()
  }

  /** 一次工具调用收尾：卡片落状态、结果进历史、流水记账。 */
  private finishToolCall(
    thread: Thread,
    callId: string,
    result: { output: string; ok: boolean; patch?: string }
  ): void {
    const card = this.cards.get(callId)
    this.cards.delete(callId)

    if (card) {
      card.status = result.ok ? 'done' : 'error'
      card.output = result.output
      card.patch = result.patch
    }

    const name = card?.name ?? 'tool'
    this.persist(thread.id, {
      role: 'toolResult',
      toolCallId: callId,
      toolName: name,
      content: result.output,
      patch: result.patch,
      isError: !result.ok,
      timestamp: Date.now(),
    })
    this.push({
      kind: 'tool',
      text: `${name} ${describeTool(name, card?.args ?? {})} → ${result.ok ? 'ok' : '失败'}`,
    })
    this.notify()
  }

  /** 不经过模型的一次工具调用（离线模式），审批与卡片和在线时完全一样。 */
  private async runToolDirect(thread: Thread, call: ToolCallBlock): Promise<void> {
    const blocked = await this.gate(thread, call)
    if (blocked) return
    const outcome = await runTool(thread.workspace, { name: call.name, args: call.arguments })
    this.finishToolCall(thread, call.id, {
      output: outcome.output,
      ok: outcome.ok,
      patch: outcome.patch,
    })
  }
}

export const store = new AgentStore(process.env.A_DA_WORKSPACE || process.cwd())
