/**
 * The agent runtime.
 *
 * One store owns every thread, the running turn, the approval gate and the
 * debug log. React subscribes to it and re-renders on `notify()`; the streaming
 * loop mutates the store directly, which keeps the transcript correct while a
 * model reply is still arriving.
 */

import {
  configPath,
  readLlmConfig,
  testConnection,
  writeSavedConfig,
  type ProviderConfig,
} from './config'
import { parseToolArgs, streamChat } from './llm'
import { describeTool, isWriteTool, resolveProjectPath, runTool, scanWorkspace } from './tools'
import { TOOL_SPECS, type ChatMessage, type DebugEntry, type Item, type Thread, type ToolCall } from './types'

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

let counter = 0
const nextId = (prefix: string) => `${prefix}_${Date.now().toString(36)}_${++counter}`

function titleFrom(text: string): string {
  const line = text.trim().split('\n')[0]!.trim()
  return line.length > 24 ? `${line.slice(0, 24)}…` : line || '新会话'
}

function makeThread(workspace: string): Thread {
  return {
    id: nextId('thread'),
    title: '新会话',
    createdAt: Date.now(),
    workspace,
    items: [],
    messages: [],
  }
}

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
  private queue: { thread: Thread; text: string; item: Item }[] = []
  private abort: AbortController | null = null
  private notifyTimer: ReturnType<typeof setTimeout> | null = null
  private logId = 0

  constructor(workspace: string) {
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

  // ------------------------------------------------------------------ messages

  send(text: string): void {
    const prompt = text.trim()
    if (!prompt) return
    const thread = this.active
    if (thread.title === '新会话') thread.title = titleFrom(prompt)
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

  private async drain(): Promise<void> {
    if (this.running) return
    this.running = true
    this.notify()
    try {
      while (this.queue.length) {
        const next = this.queue.shift()!
        next.thread.messages.push({ role: 'user', content: next.text })
        await this.turn(next.thread, next.text)
      }
    } catch (error) {
      if ((error as Error).name !== 'AbortError') {
        const message = (error as Error).message ?? String(error)
        this.active.items.push({
          kind: 'notice',
          id: nextId('item'),
          at: Date.now(),
          text: `运行失败：${message}`,
          level: 'error',
        })
        this.push({ kind: 'error', text: message })
      }
    } finally {
      this.running = false
      this.abort = null
      this.notify()
    }
  }

  private async turn(thread: Thread, prompt: string): Promise<void> {
    const config = await readLlmConfig()
    if (!config) {
      await this.offlineTurn(thread, prompt)
      return
    }
    this.push({ kind: 'request', text: `${config.model} @ ${config.baseUrl}（${config.source}）` })

    for (let step = 0; step < MAX_STEPS; step++) {
      this.abort = new AbortController()
      const item: Item = { kind: 'assistant', id: nextId('item'), at: Date.now(), text: '', streaming: true }
      thread.items.push(item)
      const index = thread.items.length - 1
      this.notify()

      const calls: ToolCall[] = []
      for await (const event of streamChat(config, thread.messages, {
        tools: TOOL_SPECS,
        effort: EFFORT_VALUE[this.effort],
        signal: this.abort.signal,
      })) {
        if (event.type === 'text') {
          const live = thread.items[index]
          if (live?.kind === 'assistant') live.text += event.text
          this.notifySoon()
        } else {
          calls.push(...event.calls)
        }
      }
      const live = thread.items[index]
      if (live?.kind === 'assistant') live.streaming = false
      this.notify()

      if (!calls.length) {
        const text = live?.kind === 'assistant' ? live.text : ''
        if (text.trim()) thread.messages.push({ role: 'assistant', content: text })
        return
      }

      thread.messages.push({
        role: 'assistant',
        content: live?.kind === 'assistant' ? live.text : '',
        tool_calls: calls.map((call) => ({
          id: call.id,
          type: 'function' as const,
          function: { name: call.name, arguments: call.args },
        })),
      })
      this.push({ kind: 'tools', text: calls.map((call) => call.name).join(', ') })
      await this.runCalls(thread, calls)
    }
    thread.items.push({
      kind: 'notice',
      id: nextId('item'),
      at: Date.now(),
      text: `达到单轮 ${MAX_STEPS} 步上限，已停下。可以继续输入让它接着做。`,
      level: 'info',
    })
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
    await this.runCalls(thread, [{ id: nextId('call'), name: 'list_files', args: '{"depth":2}' }])
    const info = this.workspaceInfo
    thread.items.push({
      kind: 'assistant',
      id: nextId('item'),
      at: Date.now(),
      text: `【离线模式】我扫描了项目 \`${thread.workspace}\`：${info.files} 个文件、${info.dirs} 个目录。配置模型接口后，我会按你的任务在这个目录里读写文件、执行命令。`,
    })
    thread.messages.push({ role: 'user', content: prompt })
    thread.messages.push({ role: 'assistant', content: '离线模式：仅扫描工作区，未调用模型。' })
    this.notify()
  }

  private needsApproval(name: string): boolean {
    if (this.approval === 'ask') return true
    if (this.approval === 'readonly') return isWriteTool(name)
    return false
  }

  private async runCalls(thread: Thread, calls: ToolCall[]): Promise<void> {
    for (const call of calls) {
      const args = parseToolArgs(call.args)
      const item: Item = {
        kind: 'tool',
        id: nextId('item'),
        at: Date.now(),
        callId: call.id,
        name: call.name,
        args,
        rawArgs: call.args,
        status: this.needsApproval(call.name) ? 'awaiting' : 'running',
      }
      thread.items.push(item)
      const index = thread.items.length - 1
      this.notify()

      if (item.status === 'awaiting') {
        const approved = await new Promise<boolean>((resolve) => this.approvals.set(item.id, resolve))
        const live = thread.items[index]
        if (live?.kind !== 'tool') continue
        if (!approved) {
          live.status = 'denied'
          live.output = '已拒绝执行'
          thread.messages.push({
            role: 'tool',
            tool_call_id: call.id,
            content: '用户拒绝了这次调用。不要重试同样的调用，先说明原因或换一种做法。',
          })
          this.push({ kind: 'tool', text: `${call.name} 被拒绝` })
          this.notify()
          continue
        }
        live.status = 'running'
        this.notify()
      }

      const outcome = await runTool(thread.workspace, { name: call.name, args })
      const live = thread.items[index]
      if (live?.kind !== 'tool') continue
      live.status = outcome.ok ? 'done' : 'error'
      live.output = outcome.output
      live.patch = outcome.patch
      thread.messages.push({ role: 'tool', tool_call_id: call.id, content: outcome.output })
      this.push({
        kind: 'tool',
        text: `${call.name} ${describeTool(call.name, args)} → ${outcome.ok ? 'ok' : '失败'}`,
      })
      this.notify()
    }
  }
}

export const store = new AgentStore(process.env.A_DA_WORKSPACE || process.cwd())
