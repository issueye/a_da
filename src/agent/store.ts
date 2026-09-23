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

import { existsSync } from 'node:fs'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import {
  configPath,
  readLlmConfig,
  readSavedConfig,
  readSavedAppearance,
  testConnection,
  writeSavedAppearance,
  writeSavedConfig,
  type ProviderConfig,
} from './config'
import { runAgentLoop } from './core/agent-loop'
import type {
  AgentMessage,
  AssistantMessage,
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
import type { SessionSummary } from './session/types'
import { defaultPromptManager } from './prompts/manager'
import {
  defaultSubagentManager,
  type SubagentProfile,
  type SubagentRunResult,
  type SubagentStepUpdate,
} from './subagents'
import { applyAppearance, appearance, shortPath, type Appearance } from '../theme'
import { computeThreadStats, type AgentMode, type DebugEntry, type Item, type Thread, type ThreadStats } from './types'
import {
  selectCompactSelection,
  executeCompaction,
  buildCompactSummaryMessage,
  shouldAutoCompact,
  estimateMessageTokens,
  getModelContextWindow,
} from './compact'
import { showCompletionNotification } from '../platform/notification'

export type ApprovalMode = 'auto' | 'ask' | 'readonly'
export type Effort = 'max' | 'high' | 'medium' | 'low'

export interface ConfirmModalOptions {
  title: string
  message: string
  confirmText?: string
  cancelText?: string
  onConfirm: () => void
}

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
    mode: 'code',
  }
}

type ToolCard = Extract<Item, { kind: 'tool' }>

export class AgentStore {
  threads: Thread[] = []
  activeId: string
  /**
   * 标签页：打开着的会话 id，顺序就是标签顺序。**这是视图，不是数据。**
   *
   * `threads` 是会话本体（数据），侧边栏列的是它；这里是"现在开着哪几个"。
   * 关标签只从这里去掉一个 id，会话和盘上的流水都不动——想再看到它，从侧边栏
   * 点一下就是。两者分开之后，关闭按钮才是可逆的。
   */
  openTabIds: string[] = []
  approval: ApprovalMode = 'auto'
  effort: Effort = 'max'
  /** 协作模式：code (敏捷编码) | plan (规划设计) | create (元开发/智能体自我进化) */
  mode: AgentMode = 'code'
  debugOpen = false
  settingsOpen = false
  pluginsOpen = false
  confirmModal: ConfirmModalOptions | null = null
  /** The installed light/dark mode. The palette in `theme.ts` mirrors this. */
  appearance: Appearance = appearance()
  log: DebugEntry[] = []
  /** Scan of the active project only; other projects are counted when opened. */
  workspaceInfo: { files: number; dirs: number; scanning: boolean } = {
    files: 0,
    dirs: 0,
    scanning: true,
  }
  /** Top-level project names, used by the composer's `+` picker. */
  entries: string[] = []
  /** 当前已配置的模型名称，展示在输入框工具栏等位置。 */
  currentModel: string = ''
  /** 配置的模型上下文 Token 上限（若为 0 或未配置则采用模型预设） */
  contextWindow: number = 0
  /** 是否开启多模态图片输入支持 */
  supportsImages: boolean = false
  /** 待填入 Composer 的草稿文本回调，用于提示词一键应用到当前输入框 */
  pendingDraft: string | null = null

  /** 切换当前协作模式 */
  setMode(mode: AgentMode) {
    this.mode = mode
    if (this.active) {
      this.active.mode = mode
    }
    this.notify()
  }

  /** 将提示词应用到当前对话输入框并自动关闭插件窗口 */
  applyPromptToComposer(content: string) {
    this.pendingDraft = content
    this.pluginsOpen = false
    this.notify()
  }

  /** 清空已消费的草稿文本 */
  clearPendingDraft() {
    this.pendingDraft = null
  }

  /** 当前激活会话的累计 Token 与耗时统计 */
  get activeThreadStats(): ThreadStats {
    return computeThreadStats(this.active)
  }

  get running(): boolean {
    return this.isThreadRunning(this.activeId)
  }
  set running(val: boolean) {
    if (val) {
      this.runningThreadIds.add(this.activeId)
    } else {
      this.runningThreadIds.delete(this.activeId)
    }
  }

  private listeners = new Set<() => void>()
  private approvals = new Map<string, (approved: boolean) => void>()
  /** 本轮每张工具卡片，按调用 id 找回去更新状态。 */
  private cards = new Map<string, ToolCard>()
  /** 每个会话独立的后续排队指令 */
  private queues = new Map<string, { thread: Thread; text: string; images?: string[]; item: Item }[]>()
  /** 每个会话独立的 AbortController */
  private aborts = new Map<string, AbortController>()
  /** 正在并发运行的会话集合 */
  private runningThreadIds = new Set<string>()
  /** 运行中子智能体的转向指令队列 */
  private steeringQueues = new Map<string, AgentMessage[]>()
  /** 最近一次工作区扩展加载：跑一轮之前要等它，工具表才完整。 */
  private extensionsReady: Promise<void> = Promise.resolve()
  private notifyTimer: ReturnType<typeof setTimeout> | null = null
  private logId = 0

  get abort(): AbortController | null {
    return this.aborts.get(this.activeId) ?? null
  }

  get queue(): { thread: Thread; text: string; images?: string[]; item: Item }[] {
    return this.queues.get(this.activeId) ?? []
  }
  set queue(items: { thread: Thread; text: string; images?: string[]; item: Item }[]) {
    if (items.length === 0) {
      this.queues.delete(this.activeId)
    } else {
      this.queues.set(this.activeId, items)
    }
  }

  constructor(workspace: string = process.env.A_DA_WORKSPACE || process.cwd()) {
    defaultExtensionLoader.bindHost((msg) => this.trace(msg))
    // 上次的会话要先摆回来，再决定当前项目是哪一个：恢复完就直接显示，而不是
    // 先给一个空会话、等异步任务回来再换掉。
    const restored = this.restore()
    const active = restored?.active ?? makeThread(workspace)
    if (!restored) this.threads = [active]
    this.activeId = active.id
    this.openTabIds = [active.id]
    // 选过的模式在第一帧之前就装好：装晚了深色用户每次启动都会先闪一下白。
    this.appearance = readSavedAppearance() ?? this.appearance
    applyAppearance(this.appearance)
    void this.refresh()
    void readSavedConfig().then((cfg) => {
      if (cfg.model) {
        this.currentModel = cfg.model
      }
      if (typeof cfg.contextWindow === 'number') {
        this.contextWindow = cfg.contextWindow
      }
      if (typeof cfg.supportsImages === 'boolean') {
        this.supportsImages = cfg.supportsImages
      }
      this.notify()
    }).catch(() => {})
  }

  /**
   * 把磁盘上的会话恢复回来：每个工作区一个项目，标签按打开时间重建。
   *
   * 同步的，理由和 `readSavedAppearance` 一样——第一帧就得是对的。读的是小文件，
   * 而且恢复之后当前项目、侧边栏、标签栏才都有东西可显示。
   *
   * 恢复的是**数据**（会话列表和它们的历史）；视图（开着哪些标签）下次启动从
   * 最近一次会话开始，因为「上次开着哪几个」没有单独记。关掉标签从来不删会话，
   * 所以想找回任何一个都在侧边栏里。
   *
   * @returns 有历史时返回恢复结果；没有任何历史时返回 null，调用方就新建一个。
   */
  private restore(): { active: Thread } | null {
    const saved = defaultSessionManager.listAllSessionsSync()
    if (saved.length === 0) return null

    // 新的在前，所以排序结果里第一个就是上次用的那个。
    const threads = saved.map((summary) => this.threadFrom(summary))

    // 自动纠偏与自愈：若子智能体会话的 parentId 错误记录为非主会话或未指向其调用者
    for (const thread of threads) {
      if (thread.isSubagent) {
        // 在所有主会话中查找谁在 invoke_subagent 卡片中调用了此子会话
        const callerThread = threads.find(
          (t) =>
            !t.isSubagent &&
            t.workspace === thread.workspace &&
            t.items.some(
              (it) =>
                it.kind === 'tool' &&
                it.name === 'invoke_subagent' &&
                ((it.details as any)?.subagent_thread_id === thread.id ||
                  it.output?.includes(thread.id))
            )
        )
        if (callerThread && thread.parentId !== callerThread.id) {
          thread.parentId = callerThread.id
          void defaultSessionManager
            .updateSessionMeta(thread.id, { parentId: callerThread.id }, thread.workspace)
            .catch(() => {})
        }
      }
    }

    this.threads = threads
    return { active: threads[0]! }
  }

  /** 一个会话摘要 + 它的消息流水 → 界面上的 Thread。 */
  private threadFrom(summary: SessionSummary): Thread {
    const entries = defaultSessionManager.loadSummaryEntriesSync(summary)
    const messages = defaultSessionManager.loadSummaryMessagesSync(summary)
    const toolResults = new Map<string, Extract<AgentMessage, { role: 'toolResult' }>>()
    for (const m of messages) {
      if (m.role === 'toolResult') {
        toolResults.set(m.toolCallId, m)
      }
    }

    let items: Item[] = []
    const renderedToolCallIds = new Set<string>()

    for (let index = 0; index < entries.length; index++) {
      const entry = entries[index]!
      if (entry.type === 'compact') {
        const pruned = [...items]
        items = [
          {
            kind: 'compact',
            id: entry.id,
            at: entry.timestamp,
            summary: entry.summary,
            preTokens: entry.preTokens,
            postTokens: entry.postTokens,
            savedTokens: entry.savedTokens,
            turnsSummarized: entry.turnsSummarized,
            customInstructions: entry.customInstructions,
            prunedItems: pruned,
          },
        ]
        continue
      }
      if (entry.type !== 'message') continue
      const message = entry.message
      const at = message.timestamp ?? summary.createdAt

      if (message.role === 'user') {
        const text = typeof message.content === 'string' ? message.content : ''
        // 若此消息是紧跟在 compact 后的结构化 continuation 消息，界面已有 compact 卡片呈现，无需重复展示多余气泡
        if (text.startsWith('This session is being continued from a previous conversation')) {
          continue
        }
        items.push({ kind: 'user', id: `${summary.id}_restored_user_${index}`, at, text })
      } else if (message.role === 'assistant') {
        // 恢复思考链
        if (message.thinking) {
          items.push({
            kind: 'thinking',
            id: `${summary.id}_restored_think_${index}`,
            at,
            text: message.thinking,
            endedAt: at,
          })
        }

        // 恢复工具调用卡片
        if (message.toolCalls && message.toolCalls.length > 0) {
          for (const call of message.toolCalls) {
            renderedToolCallIds.add(call.id)
            const res = toolResults.get(call.id)
            const isDenied = res?.content === DENIED_REASON
            const status: ToolCard['status'] = res
              ? res.isError
                ? isDenied
                  ? 'denied'
                  : 'error'
                : 'done'
              : 'done'
            items.push({
              kind: 'tool',
              id: `${summary.id}_restored_tool_${call.id}`,
              at: res?.timestamp ?? at,
              callId: call.id,
              name: call.name,
              args: call.arguments ?? {},
              rawArgs: call.rawArguments ?? JSON.stringify(call.arguments ?? {}),
              status,
              output: res?.content ?? '',
              patch: res?.patch,
              threadId: summary.id,
            })
          }
        }

        // 恢复文本回复
        if (message.content && message.content.trim()) {
          items.push({
            kind: 'assistant',
            id: `${summary.id}_restored_asst_${index}`,
            at,
            text: message.content,
            streaming: false,
            usage: message.usage,
            durationMs: message.durationMs,
            turnDurationMs: message.turnDurationMs,
          })
        }
      } else if (message.role === 'toolResult') {
        // 若存在孤立的工具结果（未挂载在 assistant.toolCalls 下），补充还原为工具卡片
        if (!renderedToolCallIds.has(message.toolCallId)) {
          renderedToolCallIds.add(message.toolCallId)
          const isDenied = message.content === DENIED_REASON
          const status: ToolCard['status'] = message.isError
            ? isDenied
              ? 'denied'
              : 'error'
            : 'done'
          items.push({
            kind: 'tool',
            id: `${summary.id}_restored_tool_${message.toolCallId}`,
            at,
            callId: message.toolCallId,
            name: message.toolName,
            args: {},
            rawArgs: '',
            status,
            output: message.content,
            patch: message.patch,
            threadId: summary.id,
          })
        }
      }
    }

    return {
      id: summary.id,
      title: summary.title,
      createdAt: summary.createdAt,
      workspace: summary.workspace,
      items,
      messages,
      parentId: summary.parentId,
      subagentId: summary.subagentId,
      isSubagent: Boolean(summary.parentId || summary.subagentId),
    }
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

  /**
   * 标签栏显示所有已打开的会话，按打开顺序排列。
   * 会话页签不再针对某个工作区过滤，所有点开的会话都在标签栏展示。
   */
  get openTabs(): Thread[] {
    const byId = new Map(this.threads.map((thread) => [thread.id, thread]))
    const tabs: Thread[] = []
    for (const id of this.openTabIds) {
      const thread = byId.get(id)
      if (thread) tabs.push(thread)
    }
    return tabs
  }

  /** 指定会话是否正在运行（支持多会话并发）。 */
  isThreadRunning(threadId: string): boolean {
    return this.runningThreadIds.has(threadId)
  }

  /**
   * 兼容旧版单一运行会话接口：若当前激活会话在运行则返回其 id，否则返回任一正在运行的会话 id。
   */
  get runningThreadId(): string | null {
    if (this.runningThreadIds.has(this.activeId)) return this.activeId
    return this.runningThreadIds.values().next().value ?? null
  }

  set runningThreadId(id: string | null) {
    if (id) {
      this.runningThreadIds.add(id)
    } else {
      this.runningThreadIds.clear()
    }
  }

  /**
   * 正在跑的会话 id，没有就是 null。
   */
  get runningId(): string | null {
    return this.runningThreadId
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

  /**
   * 切换明暗模式。
   *
   * 先把调色板装上再 notify：界面没有 memo 的组件，重渲染时读到的就是新颜色。
   * 落盘失败不打断切换——颜色已经变了，只是下次启动不一定记得。
   */
  setAppearance(next: Appearance): void {
    this.appearance = next
    applyAppearance(next)
    this.push({ kind: 'info', text: next === 'dark' ? '已切换到深色模式' : '已切换到浅色模式' })
    void writeSavedAppearance(next).catch(() => {})
    this.notify()
  }

  toggleAppearance(): void {
    this.setAppearance(this.appearance === 'dark' ? 'light' : 'dark')
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

  setPlugins(open: boolean): void {
    this.pluginsOpen = open
    this.notify()
  }

  showConfirm(options: ConfirmModalOptions): void {
    this.confirmModal = options
    this.notify()
  }

  closeConfirm(): void {
    this.confirmModal = null
    this.notify()
  }

  /** 重新扫描并加载所有启用的扩展插件与工具 */
  async reloadPlugins(): Promise<void> {
    this.extensionsReady = defaultExtensionLoader
      .autoLoadExtensions(this.project)
      .then((loaded) => {
        if (loaded.length > 0) {
          this.push({ kind: 'info', text: `已重新加载扩展工具：${loaded.join(', ')}` })
        } else {
          this.push({ kind: 'info', text: '已刷新插件列表' })
        }
      })
      .catch((err) => {
        this.push({ kind: 'error', text: `加载扩展失败：${(err as Error).message}` })
      })
    await this.extensionsReady
    this.notify()
  }

  /** Save the provider block and say where it landed, so the UI can confirm. */
  async saveProvider(config: ProviderConfig): Promise<string | null> {
    try {
      await writeSavedConfig(config)
      this.currentModel = config.model || ''
      this.contextWindow = config.contextWindow || 0
      this.supportsImages = Boolean(config.supportsImages)
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
    this.mode = thread.mode ?? 'code'
    this.threads = [thread, ...this.threads]
    this.activeId = thread.id
    this.openTab(thread.id)
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
      this.selectThread(existing.id)
      this.push({ kind: 'info', text: `切换到项目 ${workspace}` })
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

    const normalized = result.path
    const existing = this.projects.find(
      (p) => p.toLowerCase() === normalized.toLowerCase(),
    )
    if (existing) {
      this.selectProject(existing)
      this.push({ kind: 'info', text: `工作区已存在，已切换至「${shortPath(existing, 2)}」` })
      return null
    }

    this.newThread(normalized)
    this.push({ kind: 'info', text: `已添加并打开工作区「${shortPath(normalized, 2)}」` })
    return null
  }

  /** 选中一个会话，顺带把它的标签开出来（侧边栏点会话走的就是这里）。 */
  selectThread(id: string): void {
    const beforeThread = this.threads.find((t) => t.id === this.activeId)
    const nextThread = this.threads.find((t) => t.id === id)
    this.openTab(id)
    if (this.activeId === id) {
      // 已经选中的会话也可能是「标签被关了又点回来」，所以上面照样要开标签。
      this.notify()
      return
    }
    this.activeId = id
    if (nextThread) {
      this.mode = nextThread.mode ?? 'code'
    }
    if (beforeThread && nextThread && beforeThread.workspace !== nextThread.workspace) {
      void this.refresh()
    }
    this.notify()
  }

  /**
   * 为指定会话更换关联的工作区（未开始对话前在居中界面切换所属工作区）。
   */
  setThreadWorkspace(threadId: string, newWorkspace: string): void {
    const thread = this.threads.find((t) => t.id === threadId)
    if (!thread || thread.workspace === newWorkspace) return
    thread.workspace = newWorkspace
    void defaultSessionManager.createSession(thread.id, newWorkspace, thread.title).catch(() => {})
    if (this.activeId === threadId) {
      void this.refresh()
    }
    this.notify()
  }

  /** 把一个会话加进标签栏（已在就不动，保持原顺序）。 */
  openTab(id: string): void {
    if (this.openTabIds.includes(id)) return
    this.openTabIds = [...this.openTabIds, id]
  }

  /**
   * 关掉一个标签：**只是关掉视图，不删会话。**
   *
   * 会话还在 `threads` 里、流水还在盘上，侧边栏照样列着它，再点一下就重新开
   * 标签。关掉的如果是当前会话，就切到剩下最靠后的那个标签。
   *
   * 全局最后一个标签关不掉（`TabStrip` 也不显示它的 ×）：窗口总得显示
   * 点什么。要清空列表就先切到别的会话，或者用侧边栏的垃圾桶删掉这个会话。
   */
  closeTab(id: string): void {
    if (!this.openTabIds.includes(id)) return
    if (this.openTabs.length <= 1) return

    this.openTabIds = this.openTabIds.filter((candidate) => candidate !== id)
    if (this.activeId === id) {
      const remainingTabs = this.openTabs
      if (remainingTabs.length > 0) {
        this.selectThread(remainingTabs[remainingTabs.length - 1]!.id)
      }
    }
    this.notify()
  }

  /**
   * 删掉一个会话：从列表里去掉，盘上的流水也一起删掉。
   *
   * 项目是靠会话存在的，所以删掉某个工作区的最后一个会话时会补一个新的空会话，
   * 而不是让工作区从侧边栏消失——用户删的是一个会话，不是一个工作区。
   * 正在跑的那一轮不能删：它还在往这个会话里写。
   *
   * 删数据顺带把它的标签也去掉：会话都没了，标签留着没有意义。（反过来不成立：
   * `closeTab` 只关标签，不碰数据。）
   *
   * @returns 出错时返回要显示给用户的理由，成功返回 null。
   */
  deleteThread(id: string): string | null {
    const thread = this.threads.find((candidate) => candidate.id === id)
    if (!thread) return null
    if (this.isThreadRunning(id)) return '这个会话正在运行，先停止再删除'

    // 级联清理名下的所有子智能体会话
    const childIds = new Set(this.threads.filter((t) => t.parentId === id).map((t) => t.id))
    for (const cid of childIds) {
      if (this.isThreadRunning(cid)) {
        this.stop(cid)
      }
    }

    this.threads = this.threads.filter((candidate) => candidate.id !== id && !childIds.has(candidate.id))
    this.openTabIds = this.openTabIds.filter((candidate) => candidate !== id && !childIds.has(candidate))
    this.queues.delete(id)
    for (const cid of childIds) {
      this.queues.delete(cid)
    }

    this.push({ kind: 'info', text: `已删除会话「${thread.title}」` })
    void defaultSessionManager.deleteSession(id, thread.workspace).catch(() => {})

    if (this.activeId === id || childIds.has(this.activeId)) {
      const next = this.threads.find((candidate) => candidate.workspace === thread.workspace)
      if (next) {
        this.selectThread(next.id)
      } else {
        const fresh = makeThread(thread.workspace)
        this.threads = [fresh, ...this.threads]
        this.selectThread(fresh.id)
      }
    }

    this.notify()
    return null
  }

  /**
   * 移除一个工作区：清理其所属的所有会话、标签与任务队列，并删除磁盘落盘目录。
   *
   * 保护规则：
   * 1. 运行中保护：工作区内若有正在运行的会话，阻止移除；
   * 2. 最少保留保护：若只剩最后一个工作区，阻止移除，避免窗口失去有效项目。
   *
   * @returns 失败时返回错误提示，成功返回 null。
   */
  removeProject(workspace: string): string | null {
    if (!this.projects.includes(workspace)) return null

    const runningInWorkspace = this.threads.some(
      (candidate) => candidate.workspace === workspace && this.isThreadRunning(candidate.id),
    )
    if (runningInWorkspace) return '该工作区内有会话正在运行，先停止再移除'

    if (this.projects.length <= 1) return '至少保留一个工作区'

    const doomed = this.threads.filter((candidate) => candidate.workspace === workspace)
    const doomedIds = new Set(doomed.map((t) => t.id))

    this.threads = this.threads.filter((candidate) => candidate.workspace !== workspace)
    this.openTabIds = this.openTabIds.filter((candidate) => !doomedIds.has(candidate))
    for (const t of doomed) {
      this.queues.delete(t.id)
    }

    // 若移除的是当前激活的工作区，将焦点转移到剩余工作区
    if (this.project === workspace || doomedIds.has(this.activeId)) {
      const remainingProject = this.projects[0]!
      const nextThread =
        this.threads.find((candidate) => candidate.workspace === remainingProject) ?? this.threads[0]!
      this.activeId = nextThread.id
      this.openTab(nextThread.id)
      void this.refresh()
    }

    void defaultSessionManager.deleteWorkspace(workspace).catch(() => {})
    this.push({ kind: 'info', text: `已移除工作区「${shortPath(workspace, 2)}」` })
    this.notify()
    return null
  }

  // ------------------------------------------------------------------ messages

  send(text: string, images?: string[]): void {
    const prompt = text.trim()
    if (!prompt && (!images || images.length === 0)) return
    const thread = this.active
    if (thread.title === '新会话') {
      thread.title = titleFrom(prompt || '图片任务')
      void defaultSessionManager
        .updateSessionTitle(thread.id, thread.title, thread.workspace)
        .catch(() => {})
    }
    let threadQueue = this.queues.get(thread.id)
    if (!threadQueue) {
      threadQueue = []
      this.queues.set(thread.id, threadQueue)
    }

    const item: Item = {
      kind: 'user',
      id: nextId('item'),
      at: Date.now(),
      text: prompt,
      images: images && images.length > 0 ? [...images] : undefined,
    }

    if (this.isThreadRunning(thread.id)) {
      item.queued = true
      thread.items.push(item)
      threadQueue.push({ thread, text: prompt, images, item })
      this.push({ kind: 'info', text: `已排队第 ${threadQueue.length} 条后续指令` })
      this.notify()
      return
    }

    thread.items.push(item)
    threadQueue.push({ thread, text: prompt, images, item })
    void this.drain(thread)
  }

  stop(threadId?: string): void {
    const targetId = threadId ?? this.activeId
    if (!this.isThreadRunning(targetId)) return
    const controller = this.aborts.get(targetId)
    if (controller) {
      controller.abort()
    }
    const queued = this.queues.get(targetId)
    if (queued) {
      for (const q of queued) {
        if (q.item.kind === 'user' && q.item.queued) {
          delete q.item.queued
        }
      }
      this.queues.delete(targetId)
    }
    const thread = this.threads.find((t) => t.id === targetId)
    const title = thread ? `「${thread.title}」` : ''
    this.push({ kind: 'info', text: `用户停止了会话${title}的运行` })
    this.notify()
  }

  decide(toolItemId: string, approved: boolean): void {
    const resolve = this.approvals.get(toolItemId)
    if (!resolve) return
    this.approvals.delete(toolItemId)
    resolve(approved)
  }

  /**
   * 启动子智能体异步执行会话：
   * 在 store.threads 中作为 parentThread 的子会话挂载，自动在 TabStrip 中开启独立页签，
   * 启动独立的 runAgentLoop，将思考过程、工具调用与总结报告实时流式推送到子会话，
   * 同时向父会话的 onStepUpdate 回传进度。
   */
  async startSubagentThread(options: {
    parentThreadId?: string
    subagentId: string
    task: string
    additionalContext?: string
    signal?: AbortSignal
    onStepUpdate?: (update: SubagentStepUpdate) => void
  }): Promise<{
    thread: Thread
    resultPromise: Promise<SubagentRunResult>
  }> {
    let parentCandidate = options.parentThreadId
      ? this.threads.find((t) => t.id === options.parentThreadId)
      : null
    if (!parentCandidate) {
      // 避免误将当前聚焦的子智能体会话自身作为父级，向上回溯至主会话
      let curr: Thread | undefined = this.active
      while (curr && curr.isSubagent && curr.parentId) {
        curr = this.threads.find((t) => t.id === curr!.parentId)
      }
      parentCandidate = curr ?? this.active
    }
    const parentThread = parentCandidate
    const workspace = parentThread.workspace
    const profile = await defaultSubagentManager.getById(options.subagentId, workspace)
    if (!profile) {
      throw new Error(`未找到 ID 为 "${options.subagentId}" 的子智能体`)
    }
    if (!profile.enabled) {
      throw new Error(`子智能体 "${profile.name}" 当前处于禁用状态`)
    }

    const parentConfig = await readLlmConfig()
    const config: ProviderConfig = parentConfig
      ? {
          ...parentConfig,
          ...(profile.modelOverride?.model ? { model: profile.modelOverride.model } : {}),
        }
      : {
          baseUrl: 'http://localhost/v1',
          apiKey: '',
          model: profile.modelOverride?.model ?? 'test-model',
          source: 'offline',
        }

    const subId = nextId('subagent')
    const title = `${profile.name}: ${titleFrom(options.task)}`
    const subagentThread: Thread = {
      id: subId,
      title,
      createdAt: Date.now(),
      workspace,
      items: [],
      messages: [],
      parentId: parentThread.id,
      subagentId: profile.id,
      isSubagent: true,
    }

    void defaultSessionManager
      .createSession(subId, workspace, title, {
        parentId: parentThread.id,
        subagentId: profile.id,
      })
      .catch(() => {})

    // 挂载到会话列表中，并自动开启独立页签
    const parentIndex = this.threads.findIndex((t) => t.id === parentThread.id)
    if (parentIndex >= 0) {
      this.threads.splice(parentIndex + 1, 0, subagentThread)
    } else {
      this.threads.push(subagentThread)
    }
    this.openTab(subagentThread.id)

    // 写入首条用户提示词
    let userPromptText = `【委派任务】\n${options.task}`
    if (options.additionalContext?.trim()) {
      userPromptText += `\n\n【参考上下文】\n${options.additionalContext.trim()}`
    }
    userPromptText += '\n\n请针对上述任务要求，自主使用工具调研或处理。调研或处理完成后，请不要再调用工具，直接给出结构化、高信息密度的最终总结与建议。'

    const userItem: Item = {
      kind: 'user',
      id: nextId('item'),
      at: Date.now(),
      text: userPromptText,
    }
    subagentThread.items.push(userItem)
    const userMessage: AgentMessage = { role: 'user', content: userPromptText, timestamp: Date.now() }
    subagentThread.messages.push(userMessage)
    this.persist(subagentThread.id, userMessage)

    // 构建中止控制器与运行状态
    const controller = new AbortController()
    this.aborts.set(subagentThread.id, controller)
    if (options.signal) {
      if (options.signal.aborted) {
        controller.abort()
      } else {
        options.signal.addEventListener('abort', () => controller.abort(), { once: true })
      }
    }
    this.runningThreadIds.add(subagentThread.id)
    this.push({ kind: 'info', text: `已启动子智能体「${profile.name}」独立会话 (${subagentThread.id})` })
    this.notify()

    // 准备工具（白名单、黑名单、通配符、只读限制与递归防护）
    const allTools = defaultToolRegistry.getToolsForWorkspace(workspace)
    const allowedSet = new Set(profile.allowedTools)
    const disallowedSet = new Set(profile.disallowedTools ?? [])
    disallowedSet.add('invoke_subagent')
    disallowedSet.add('check_subagent')
    disallowedSet.add('send_subagent_message')
    disallowedSet.add('resume_subagent')

    const subagentTools = allTools.filter((t) => {
      if (disallowedSet.has(t.name)) return false
      if (!allowedSet.has('*') && !allowedSet.has(t.name)) return false
      if (profile.mode === 'readonly' && defaultToolRegistry.isWriteTool(t.name)) return false
      return true
    })

    const steeringQueue: AgentMessage[] = []
    this.steeringQueues.set(subagentThread.id, steeringQueue)

    const resultPromise = (async (): Promise<SubagentRunResult> => {
      const startTime = Date.now()
      const maxSteps = profile.maxSteps
      let stepsExecuted = 0
      let toolCallsCount = 0
      let lastAssistantMessage: any = null
      let latestSummary = ''

      let assistant: Extract<Item, { kind: 'assistant' }> | null = null
      let reasoning: Extract<Item, { kind: 'thinking' }> | null = null
      const endReasoning = () => {
        if (reasoning && reasoning.endedAt === undefined) reasoning.endedAt = Date.now()
      }

      options.onStepUpdate?.({
        threadId: subagentThread.id,
        step: 0,
        maxSteps,
        status: 'running',
        currentAction: `子智能体 [${profile.name}] 已启动`,
      })

      if (!parentConfig) {
        const fallbackText = `未配置 LLM 供应商，子智能体 [${profile.name}] 已建立独立会话，模拟任务执行完成。\n任务：${options.task}`
        const assistantItem: Extract<Item, { kind: 'assistant' }> = {
          kind: 'assistant',
          id: nextId('item'),
          at: Date.now(),
          text: fallbackText,
        }
        subagentThread.items.push(assistantItem)
        this.steeringQueues.delete(subagentThread.id)
        this.runningThreadIds.delete(subagentThread.id)
        this.aborts.delete(subagentThread.id)
        this.notify()
        return {
          ok: true,
          summary: fallbackText,
          stepsExecuted: 1,
          durationMs: 10,
          toolCallsCount: 0,
          messages: subagentThread.messages,
        }
      }

      try {
        const loop = runAgentLoop(subagentThread.messages, config, {
          systemPrompt: profile.systemPrompt,
          tools: subagentTools,
          maxSteps,
          effort: profile.modelOverride?.effort ?? 'high',
          toolExecution: 'sequential',
          signal: controller.signal,
          getSteeringMessages: async () => {
            if (steeringQueue.length === 0) return []
            return steeringQueue.splice(0, steeringQueue.length)
          },
          beforeToolCall: async (context) => {
            if (profile.mode === 'readonly' && defaultToolRegistry.isWriteTool(context.toolCall.name)) {
              return { block: true, reason: `子智能体 ${profile.name} 运行在只读安全模式下，禁止执行写操作。` }
            }
            return undefined
          },
        })

        for await (const event of loop) {
          if (controller.signal.aborted) break

          switch (event.type) {
            case 'llm_request':
              this.logLlmRequest(event)
              break

            case 'llm_response':
              this.logLlmResponse(event)
              break

            case 'turn_start':
              stepsExecuted += 1
              options.onStepUpdate?.({
                threadId: subagentThread.id,
                step: stepsExecuted,
                maxSteps,
                status: 'running',
                currentAction: maxSteps
                  ? `正在思考第 ${stepsExecuted}/${maxSteps} 步...`
                  : `正在思考第 ${stepsExecuted} 步...`,
              })
              break

            case 'message_start':
              if (event.message.role === 'assistant') {
                assistant = null
                reasoning = null
              }
              break

            case 'message_update':
              if (event.delta.usage && assistant) {
                assistant.usage = event.delta.usage
                this.notifySoon()
              }
              if (event.delta.thinking) {
                if (!reasoning) {
                  reasoning = { kind: 'thinking', id: nextId('item'), at: Date.now(), text: '' }
                  subagentThread.items.push(reasoning)
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
                  subagentThread.items.push(assistant)
                }
                assistant.text += event.delta.text
                latestSummary += event.delta.text
                this.notifySoon()
              }
              break

            case 'message_end': {
              const message = event.message
              if (message.role !== 'assistant') break
              endReasoning()
              if (assistant) {
                assistant.streaming = false
                if (message.usage) assistant.usage = message.usage
                if (message.durationMs) assistant.durationMs = message.durationMs
                assistant.turnDurationMs = Math.max(1, Date.now() - startTime)
                message.turnDurationMs = assistant.turnDurationMs
              }
              if (message.content) {
                latestSummary = message.content
              }
              lastAssistantMessage = message
              if (message.stopReason === 'error') {
                this.fail(subagentThread, `请求失败: ${message.errorMessage ?? '未知错误'}`)
              } else if (
                message.content.trim() ||
                message.thinking ||
                (message.toolCalls && message.toolCalls.length > 0)
              ) {
                this.persist(subagentThread.id, message)
              }
              this.notify()
              break
            }

            case 'tool_execution_start': {
              toolCallsCount += 1
              const cardId = event.toolCallId
              const desc = describeTool(event.toolName, event.args)
              const card: Item = {
                kind: 'tool',
                id: nextId('item'),
                at: Date.now(),
                callId: cardId,
                name: event.toolName,
                args: event.args,
                rawArgs: JSON.stringify(event.args),
                status: 'running',
              }
              subagentThread.items.push(card)
              this.cards.set(cardId, card as ToolCard)
              options.onStepUpdate?.({
                step: stepsExecuted,
                maxSteps,
                status: 'running',
                currentAction: `[${profile.name}] 执行工具: ${desc}`,
                toolCallSummary: desc,
              })
              this.notify()
              break
            }

            case 'tool_execution_update': {
              const card = this.cards.get(event.toolCallId)
              if (card && event.partialResult?.output) {
                card.output = (card.output ?? '') + event.partialResult.output
                this.notifySoon()
              }
              break
            }

            case 'tool_execution_end': {
              const card = this.cards.get(event.toolCallId)
              if (card) {
                card.status = event.result.ok ? 'done' : 'error'
                if (event.result.output !== undefined) {
                  card.output = event.result.output
                }
                if (event.result.patch) {
                  card.patch = event.result.patch
                }
              }
              this.notify()
              break
            }

            case 'agent_end':
              subagentThread.messages.length = 0
              subagentThread.messages.push(...event.messages)
              break
          }
        }

        const durationMs = Date.now() - startTime
        const isOk = !controller.signal.aborted && lastAssistantMessage?.stopReason !== 'error'
        const resultText =
          latestSummary.trim() ||
          (isOk
            ? `[${profile.name}] 任务执行完成（共 ${stepsExecuted} 步，调用工具 ${toolCallsCount} 次）。`
            : '任务未完成或异常中断。')

        let outputFile: string | undefined = undefined
        if (resultText.length > 3000) {
          try {
            const outDir = join(workspace, '.ada', 'subagent-outputs')
            if (!existsSync(outDir)) {
              await mkdir(outDir, { recursive: true })
            }
            outputFile = join(outDir, `${subId}.md`)
            await writeFile(outputFile, `# ${title}\n\n${resultText}`, 'utf8')
          } catch {
            // ignore save failure
          }
        }

        options.onStepUpdate?.({
          step: stepsExecuted,
          maxSteps,
          status: isOk ? 'done' : 'error',
          currentAction: `执行结束（耗时 ${(durationMs / 1000).toFixed(1)}s）`,
        })

        // 异步后台运行完成时，向父会话写入一条完成提示，以便父会话与用户立即感知
        if (options.onStepUpdate && parentThread && parentThread.id !== subagentThread.id) {
          const previewText = resultText.length > 180 ? `${resultText.slice(0, 180)}...` : resultText
          const fileInfo = outputFile ? `\n\n📄 完整报告已保存至：${outputFile}` : ''
          const noticeItem: Item = {
            kind: 'notice',
            level: 'info',
            id: nextId('item'),
            at: Date.now(),
            text: `子智能体「${profile.name}」已在后台完成执行（会话 ID: ${subagentThread.id}，耗时 ${(durationMs / 1000).toFixed(1)}s）。\n成果摘要：${previewText}${fileInfo}`,
          }
          parentThread.items.push(noticeItem)
          this.notify()
        }

        return {
          ok: isOk,
          summary: resultText,
          outputFile,
          stepsExecuted,
          durationMs,
          toolCallsCount,
          messages: subagentThread.messages,
        }
      } catch (error) {
        const durationMs = Date.now() - startTime
        const errorMessage = (error as Error).message || String(error)
        this.fail(subagentThread, `执行异常: ${errorMessage}`)
        return {
          ok: false,
          summary: `子智能体 [${profile.name}] 执行失败: ${errorMessage}`,
          stepsExecuted,
          durationMs,
          toolCallsCount,
          errorMessage,
          messages: subagentThread.messages,
        }
      } finally {
        this.steeringQueues.delete(subagentThread.id)
        this.runningThreadIds.delete(subagentThread.id)
        this.aborts.delete(subagentThread.id)
        this.notify()
      }
    })()

    return { thread: subagentThread, resultPromise }
  }

  /**
   * 向子智能体会话发送转向指导消息或追加新任务
   */
  async steerSubagentThread(options: {
    subagentThreadId: string
    message: string
    summary?: string
  }): Promise<{ status: 'steered' | 'resumed' | 'not_found'; text: string }> {
    const thread = this.threads.find((t) => t.id === options.subagentThreadId)
    if (!thread) {
      return { status: 'not_found', text: `未找到 ID 为 ${options.subagentThreadId} 的子智能体会话。` }
    }

    const isRunning = this.isThreadRunning(thread.id)
    const steeringQueue = this.steeringQueues.get(thread.id)

    const steeringText = options.summary?.trim()
      ? `【上层协调指令：${options.summary.trim()}】\n${options.message.trim()}`
      : `【上层协调指令】\n${options.message.trim()}`

    if (isRunning && steeringQueue) {
      // 正在运行：注入到转向队列，子智能体在当前工具结束后会立即吸收并转向
      const msg: AgentMessage = {
        role: 'user',
        content: steeringText,
        timestamp: Date.now(),
      }
      steeringQueue.push(msg)
      // 在界面上记录一条提示
      const noticeItem: Item = {
        kind: 'notice',
        level: 'info',
        id: nextId('item'),
        at: Date.now(),
        text: `已向运行中的子智能体发送转向指令：${options.summary ?? options.message}`,
      }
      thread.items.push(noticeItem)
      this.notify()
      return {
        status: 'steered',
        text: `指令已成功发送给运行中的子智能体「${thread.title}」，将在当前工具调用完成后立即生效转向。`,
      }
    }

    // 若子智能体已处于停止/完成状态：唤醒续跑
    const userItem: Item = {
      kind: 'user',
      id: nextId('item'),
      at: Date.now(),
      text: steeringText,
    }
    thread.items.push(userItem)
    const userMessage: AgentMessage = { role: 'user', content: steeringText, timestamp: Date.now() }
    thread.messages.push(userMessage)
    this.persist(thread.id, userMessage)

    let threadQueue = this.queues.get(thread.id)
    if (!threadQueue) {
      threadQueue = []
      this.queues.set(thread.id, threadQueue)
    }
    threadQueue.push({ thread, text: steeringText, item: userItem })
    void this.drain(thread)

    return {
      status: 'resumed',
      text: `已向子智能体「${thread.title}」追加新指令并重新启动执行。`,
    }
  }

  /**
   * 恢复因网络中断、超时或异常停止的子智能体会话
   */
  async resumeSubagentThread(options: {
    subagentThreadId: string
    instruction?: string
    signal?: AbortSignal
    onStepUpdate?: (update: SubagentStepUpdate) => void
  }): Promise<{ thread: Thread; resultPromise: Promise<SubagentRunResult> }> {
    const subagentThread = this.threads.find((t) => t.id === options.subagentThreadId)
    if (!subagentThread) {
      throw new Error(`未找到 ID 为 ${options.subagentThreadId} 的子智能体会话。`)
    }

    if (this.isThreadRunning(subagentThread.id)) {
      throw new Error(
        `子智能体「${subagentThread.title}」当前正在运行中，无需恢复。若需要追加指导，请使用 send_subagent_message 工具。`
      )
    }

    const profile =
      (await defaultSubagentManager.getById(subagentThread.subagentId ?? '', subagentThread.workspace)) ??
      (await defaultSubagentManager.getById('general_purpose', subagentThread.workspace))

    if (!profile) {
      throw new Error(`未能识别子智能体角色配置 (${subagentThread.subagentId ?? '未知'})。`)
    }

    const parentThread = subagentThread.parentId
      ? this.threads.find((t) => t.id === subagentThread.parentId)
      : undefined

    const resumePromptText = options.instruction?.trim()
      ? `【恢复执行指示】\n${options.instruction.trim()}`
      : `【系统恢复提示】网络或连接已恢复。请检查当前执行进度与上下文，从上次中断处继续推进任务，并产出完整成果报告。`

    const resumeItem: Item = {
      kind: 'user',
      id: nextId('item'),
      at: Date.now(),
      text: resumePromptText,
    }
    subagentThread.items.push(resumeItem)
    const userMessage: AgentMessage = { role: 'user', content: resumePromptText, timestamp: Date.now() }
    subagentThread.messages.push(userMessage)
    this.persist(subagentThread.id, userMessage)

    this.push({ kind: 'info', text: `已恢复子智能体「${profile.name}」(${subagentThread.id}) 的运行` })
    this.notify()

    const workspace = subagentThread.workspace
    const parentConfig = await readLlmConfig()
    const config: ProviderConfig = parentConfig
      ? {
          ...parentConfig,
          ...(profile.modelOverride?.model ? { model: profile.modelOverride.model } : {}),
        }
      : {
          baseUrl: 'http://localhost/v1',
          apiKey: '',
          model: profile.modelOverride?.model ?? 'test-model',
          source: 'offline',
        }

    const controller = new AbortController()
    this.aborts.set(subagentThread.id, controller)
    if (options.signal) {
      if (options.signal.aborted) {
        controller.abort()
      } else {
        options.signal.addEventListener('abort', () => controller.abort(), { once: true })
      }
    }
    this.runningThreadIds.add(subagentThread.id)
    this.notify()

    const allTools = defaultToolRegistry.getToolsForWorkspace(workspace)
    const allowedSet = new Set(profile.allowedTools)
    const disallowedSet = new Set(profile.disallowedTools ?? [])
    disallowedSet.add('invoke_subagent')
    disallowedSet.add('check_subagent')
    disallowedSet.add('send_subagent_message')
    disallowedSet.add('resume_subagent')

    const subagentTools = allTools.filter((t) => {
      if (disallowedSet.has(t.name)) return false
      if (!allowedSet.has('*') && !allowedSet.has(t.name)) return false
      if (profile.mode === 'readonly' && defaultToolRegistry.isWriteTool(t.name)) return false
      return true
    })

    const steeringQueue: AgentMessage[] = []
    this.steeringQueues.set(subagentThread.id, steeringQueue)

    const resultPromise = (async (): Promise<SubagentRunResult> => {
      const startTime = Date.now()
      const maxSteps = profile.maxSteps
      let stepsExecuted = 0
      let toolCallsCount = 0
      let lastAssistantMessage: any = null
      let latestSummary = ''

      let assistant: Extract<Item, { kind: 'assistant' }> | null = null
      let reasoning: Extract<Item, { kind: 'thinking' }> | null = null
      const endReasoning = () => {
        if (reasoning && reasoning.endedAt === undefined) reasoning.endedAt = Date.now()
      }

      options.onStepUpdate?.({
        threadId: subagentThread.id,
        step: 0,
        maxSteps,
        status: 'running',
        currentAction: `子智能体 [${profile.name}] 已恢复运行`,
      })

      if (!parentConfig) {
        const fallbackText = `未配置 LLM 供应商，子智能体 [${profile.name}] 模拟恢复执行完成。`
        const assistantItem: Extract<Item, { kind: 'assistant' }> = {
          kind: 'assistant',
          id: nextId('item'),
          at: Date.now(),
          text: fallbackText,
        }
        subagentThread.items.push(assistantItem)
        this.notify()
        this.steeringQueues.delete(subagentThread.id)
        this.runningThreadIds.delete(subagentThread.id)
        this.aborts.delete(subagentThread.id)
        return {
          ok: true,
          summary: fallbackText,
          stepsExecuted: 1,
          durationMs: 10,
          toolCallsCount: 0,
          messages: subagentThread.messages,
        }
      }

      try {
        const loop = runAgentLoop(subagentThread.messages, config, {
          systemPrompt: profile.systemPrompt,
          tools: subagentTools,
          maxSteps,
          effort: profile.modelOverride?.effort ?? 'high',
          toolExecution: 'sequential',
          signal: controller.signal,
          getSteeringMessages: async () => {
            if (steeringQueue.length === 0) return []
            return steeringQueue.splice(0, steeringQueue.length)
          },
          beforeToolCall: async (context) => {
            if (profile.mode === 'readonly' && defaultToolRegistry.isWriteTool(context.toolCall.name)) {
              return { block: true, reason: `子智能体 ${profile.name} 运行在只读安全模式下，禁止执行写操作。` }
            }
            return undefined
          },
        })

        for await (const event of loop) {
          if (controller.signal.aborted) break

          switch (event.type) {
            case 'llm_request':
              this.logLlmRequest(event)
              break

            case 'llm_response':
              this.logLlmResponse(event)
              break

            case 'turn_start':
              stepsExecuted += 1
              options.onStepUpdate?.({
                threadId: subagentThread.id,
                step: stepsExecuted,
                maxSteps,
                status: 'running',
                currentAction: maxSteps
                  ? `正在思考第 ${stepsExecuted}/${maxSteps} 步...`
                  : `正在思考第 ${stepsExecuted} 步...`,
              })
              break

            case 'message_start':
              if (event.message.role === 'assistant') {
                assistant = null
                reasoning = null
              }
              break

            case 'message_update':
              if (event.delta.usage && assistant) {
                assistant.usage = event.delta.usage
                this.notifySoon()
              }
              if (event.delta.thinking) {
                if (!reasoning) {
                  reasoning = { kind: 'thinking', id: nextId('item'), at: Date.now(), text: '' }
                  subagentThread.items.push(reasoning)
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
                  subagentThread.items.push(assistant)
                }
                assistant.text += event.delta.text
                latestSummary += event.delta.text
                this.notifySoon()
              }
              break

            case 'message_end': {
              const message = event.message
              if (message.role !== 'assistant') break
              endReasoning()
              if (assistant) {
                assistant.streaming = false
                if (message.usage) assistant.usage = message.usage
                if (message.durationMs) assistant.durationMs = message.durationMs
                assistant.turnDurationMs = Math.max(1, Date.now() - startTime)
                message.turnDurationMs = assistant.turnDurationMs
              }
              if (message.content) {
                latestSummary = message.content
              }
              lastAssistantMessage = message
              if (message.stopReason === 'error') {
                this.fail(subagentThread, `请求失败: ${message.errorMessage ?? '未知错误'}`)
              } else if (
                message.content.trim() ||
                message.thinking ||
                (message.toolCalls && message.toolCalls.length > 0)
              ) {
                this.persist(subagentThread.id, message)
              }
              this.notify()
              break
            }

            case 'tool_execution_start': {
              toolCallsCount += 1
              const cardId = event.toolCallId
              const desc = describeTool(event.toolName, event.args)
              const card: Item = {
                kind: 'tool',
                id: nextId('item'),
                at: Date.now(),
                callId: cardId,
                name: event.toolName,
                args: event.args,
                rawArgs: JSON.stringify(event.args),
                status: 'running',
              }
              subagentThread.items.push(card)
              this.cards.set(cardId, card as ToolCard)
              options.onStepUpdate?.({
                step: stepsExecuted,
                maxSteps,
                status: 'running',
                currentAction: `[${profile.name}] 执行工具: ${desc}`,
                toolCallSummary: desc,
              })
              this.notify()
              break
            }

            case 'tool_execution_update': {
              const card = this.cards.get(event.toolCallId)
              if (card && event.partialResult?.output) {
                card.output = (card.output ?? '') + event.partialResult.output
                this.notifySoon()
              }
              break
            }

            case 'tool_execution_end': {
              const card = this.cards.get(event.toolCallId)
              if (card) {
                card.status = event.result.ok ? 'done' : 'error'
                if (event.result.output !== undefined) {
                  card.output = event.result.output
                }
                if (event.result.patch) {
                  card.patch = event.result.patch
                }
              }
              this.notify()
              break
            }

            case 'agent_end':
              subagentThread.messages.length = 0
              subagentThread.messages.push(...event.messages)
              break
          }
        }

        const durationMs = Date.now() - startTime
        const isOk = !controller.signal.aborted && lastAssistantMessage?.stopReason !== 'error'
        const resultText =
          latestSummary.trim() ||
          (isOk
            ? `[${profile.name}] 任务恢复执行完成（共 ${stepsExecuted} 步，调用工具 ${toolCallsCount} 次）。`
            : '任务未完成或异常中断。')

        let outputFile: string | undefined = undefined
        if (resultText.length > 3000) {
          try {
            const outDir = join(workspace, '.ada', 'subagent-outputs')
            if (!existsSync(outDir)) {
              await mkdir(outDir, { recursive: true })
            }
            outputFile = join(outDir, `${subagentThread.id}.md`)
            await writeFile(outputFile, `# ${subagentThread.title}\n\n${resultText}`, 'utf8')
          } catch {
            // ignore save failure
          }
        }

        options.onStepUpdate?.({
          step: stepsExecuted,
          maxSteps,
          status: isOk ? 'done' : 'error',
          currentAction: `执行结束（耗时 ${(durationMs / 1000).toFixed(1)}s）`,
        })

        // 异步后台运行完成时，向父会话写入一条完成提示，以便父会话与用户立即感知
        if (options.onStepUpdate && parentThread && parentThread.id !== subagentThread.id) {
          const previewText = resultText.length > 180 ? `${resultText.slice(0, 180)}...` : resultText
          const fileInfo = outputFile ? `\n\n📄 完整报告已保存至：${outputFile}` : ''
          const noticeItem: Item = {
            kind: 'notice',
            level: 'info',
            id: nextId('item'),
            at: Date.now(),
            text: `子智能体「${profile.name}」已在后台恢复完成执行（会话 ID: ${subagentThread.id}，耗时 ${(durationMs / 1000).toFixed(1)}s）。\n成果摘要：${previewText}${fileInfo}`,
          }
          parentThread.items.push(noticeItem)
          this.notify()
        }

        return {
          ok: isOk,
          summary: resultText,
          outputFile,
          stepsExecuted,
          durationMs,
          toolCallsCount,
          messages: subagentThread.messages,
        }
      } catch (error) {
        const durationMs = Date.now() - startTime
        const errorMessage = (error as Error).message || String(error)
        this.fail(subagentThread, `执行异常: ${errorMessage}`)
        return {
          ok: false,
          summary: `子智能体 [${profile.name}] 执行失败: ${errorMessage}`,
          stepsExecuted,
          durationMs,
          toolCallsCount,
          errorMessage,
          messages: subagentThread.messages,
        }
      } finally {
        this.steeringQueues.delete(subagentThread.id)
        this.runningThreadIds.delete(subagentThread.id)
        this.aborts.delete(subagentThread.id)
        this.notify()
      }
    })()

    return { thread: subagentThread, resultPromise }
  }

  /**
   * 对指定会话执行上下文压缩与结构化摘要
   */
  async compactThread(
    threadId: string = this.activeId,
    options: {
      customInstructions?: string
      trigger?: 'manual' | 'auto'
    } = {},
  ): Promise<{ success: boolean; reason?: string }> {
    const thread = this.threads.find((t) => t.id === threadId)
    if (!thread) {
      return { success: false, reason: '未找到指定会话。' }
    }

    if (this.isThreadRunning(thread.id)) {
      return { success: false, reason: '当前会话正在运行中，请等待本轮执行完成后再执行压缩。' }
    }

    // 检查是否有足够的轮次进行压缩
    const selection = selectCompactSelection(thread.messages, thread.items)
    if (selection.messagesToSummarize.length === 0) {
      const noticeText = '当前会话历史较短（少于 2 轮），暂无需压缩的历史消息。'
      thread.items.push({
        kind: 'notice',
        id: nextId('item'),
        at: Date.now(),
        text: noticeText,
        level: 'info',
      })
      this.notify()
      return { success: false, reason: noticeText }
    }

    const config = await readLlmConfig()
    if (!config) {
      this.fail(thread, '未配置模型接口，无法执行上下文压缩。')
      return { success: false, reason: '未配置模型接口。' }
    }

    // 标记会话运行状态，避免并发冲突
    this.runningThreadIds.add(thread.id)
    const noticeId = nextId('item')
    const triggerLabel = options.trigger === 'auto' ? '自动' : '手动'
    thread.items.push({
      kind: 'notice',
      id: noticeId,
      at: Date.now(),
      text: `正在执行${triggerLabel}上下文压缩与结构化摘要提取...`,
      level: 'info',
    })
    this.notify()

    try {
      const systemPrompt = await defaultPromptManager.getCompositeSystemPrompt(
        thread.workspace,
        thread.mode ?? this.mode ?? 'code',
      )

      const result = await executeCompaction(thread, config, {
        customInstructions: options.customInstructions,
        systemPrompt,
      })

      // 移除临时 notice
      thread.items = thread.items.filter((it) => it.id !== noticeId)

      // 构造 compact item
      const compactId = nextId('compact')
      const compactItem: Item = {
        kind: 'compact',
        id: compactId,
        at: Date.now(),
        summary: result.summary,
        preTokens: result.preTokens,
        postTokens: result.postTokens,
        savedTokens: result.savedTokens,
        turnsSummarized: result.turnsSummarized,
        customInstructions: options.customInstructions,
        prunedItems: result.prunedItems,
      }

      // 重构 thread.items：紧凑卡片 + 保留的近期卡片
      thread.items = [compactItem, ...result.preservedItems]

      // 构造 continuation 消息并重组 thread.messages
      const continuationMsg: AgentMessage = {
        role: 'user',
        content: buildCompactSummaryMessage(result.summary, {
          recentMessagesPreserved: result.preservedMessages.length > 0,
        }),
        timestamp: Date.now(),
      }
      thread.messages = [continuationMsg, ...result.preservedMessages]

      // 持久化到 JSONL 流水
      await defaultSessionManager.appendCompactEntry(
        thread.id,
        {
          id: compactId,
          timestamp: Date.now(),
          summary: result.summary,
          preTokens: result.preTokens,
          postTokens: result.postTokens,
          savedTokens: result.savedTokens,
          turnsSummarized: result.turnsSummarized,
          customInstructions: options.customInstructions,
        },
        thread.workspace,
      )

      this.push({
        kind: 'info',
        text: `会话上下文压缩成功：节约约 ${result.savedTokens} Tokens（压缩比率 ${Math.round((result.savedTokens / Math.max(1, result.preTokens)) * 100)}%）`,
      })
      this.notify()
      return { success: true }
    } catch (error) {
      // 移除临时 notice 并提示错误
      thread.items = thread.items.filter((it) => it.id !== noticeId)
      const errText = `上下文压缩失败：${(error as Error).message}`
      this.fail(thread, errText)
      return { success: false, reason: errText }
    } finally {
      this.runningThreadIds.delete(thread.id)
      this.notify()
    }
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

  clearLog(): void {
    this.log = []
    this.notify()
  }

  private push(entry: {
    kind: DebugEntry['kind']
    text: string
    payload?: unknown
    raw?: string
    model?: string
    durationMs?: number
  }): void {
    this.logId += 1
    const raw =
      entry.raw ??
      (entry.payload !== undefined ? JSON.stringify(entry.payload, null, 2) : undefined)
    this.log = [
      ...this.log.slice(-MAX_LOG),
      { id: this.logId, at: Date.now(), ...entry, raw },
    ]
  }

  private logLlmRequest(event: {
    model: string
    baseUrl: string
    messages: any[]
    tools?: any[]
  }): void {
    const msgCount = event.messages?.length ?? 0
    const toolCount = event.tools?.length ?? 0
    const summary = `${event.model} @ ${event.baseUrl}（${msgCount} 条消息${toolCount > 0 ? ` · ${toolCount} 个工具` : ''}）`
    this.push({
      kind: 'request',
      model: event.model,
      text: summary,
      payload: {
        model: event.model,
        baseUrl: event.baseUrl,
        messagesCount: msgCount,
        toolsCount: toolCount,
        messages: event.messages,
        tools: event.tools,
      },
    })
    this.notifySoon()
  }

  private logLlmResponse(event: {
    model: string
    message: AssistantMessage
  }): void {
    const m = event.message
    const summaryParts: string[] = []
    if (m.durationMs) summaryParts.push(`${(m.durationMs / 1000).toFixed(1)}s`)
    if (m.usage?.totalTokens) summaryParts.push(`${m.usage.totalTokens} tok`)
    if (m.usage?.cachedTokens) summaryParts.push(`缓存 ${m.usage.cachedTokens}`)
    if (m.toolCalls && m.toolCalls.length > 0) {
      summaryParts.push(`调用 ${m.toolCalls.map((c: ToolCallBlock) => c.name).join(', ')}`)
    } else if (m.content) {
      summaryParts.push('回复完成')
    } else if (m.thinking) {
      summaryParts.push('思考完成')
    }

    const summary = `${event.model} 响应 · ${summaryParts.join(' · ') || '完成'}`
    this.push({
      kind: 'response',
      model: event.model,
      durationMs: m.durationMs,
      text: summary,
      payload: {
        model: event.model,
        content: m.content || undefined,
        thinking: m.thinking || undefined,
        toolCalls: m.toolCalls && m.toolCalls.length > 0 ? m.toolCalls : undefined,
        usage: m.usage,
        durationMs: m.durationMs,
        stopReason: m.stopReason,
        errorMessage: m.errorMessage,
      },
    })
    this.notifySoon()
  }

  /** 会话 JSONL 是追加写的流水账，写不进去也不该打断这一轮。 */
  private persist(threadId: string, message: AgentMessage): void {
    // 带着工作区：会话按「工作区/会话」分目录存，带上它就不用每次扫目录找。
    const workspace = this.threads.find((thread) => thread.id === threadId)?.workspace
    void defaultSessionManager.appendMessage(threadId, message, workspace).catch(() => {})
  }

  private fail(thread: Thread, text: string): void {
    thread.items.push({ kind: 'notice', id: nextId('item'), at: Date.now(), text, level: 'error' })
    this.push({ kind: 'error', text })
    this.notify()
  }

  private async drain(thread: Thread): Promise<void> {
    if (this.isThreadRunning(thread.id)) return
    this.runningThreadIds.add(thread.id)
    this.notify()
    try {
      const q = this.queues.get(thread.id)
      while (q && q.length > 0) {
        const next = q.shift()!
        if (next.item.kind === 'user' && next.item.queued) {
          delete next.item.queued
          this.notify()
        }
        await this.turn(next.thread, next.text, next.images)
      }

      // 主会话队列处理完毕时，触发完成通知窗口
      if (!thread.parentId && !thread.isSubagent) {
        const lastAssistant = thread.items.slice().reverse().find((i) => i.kind === 'assistant')
        const summaryText =
          lastAssistant && 'text' in lastAssistant && lastAssistant.text
            ? lastAssistant.text.slice(0, 120).replace(/\n+/g, ' ').trim()
            : '会话任务已处理完成。'
        void showCompletionNotification({
          title: `任务完成：${thread.title}`,
          body: summaryText,
          threadId: thread.id,
        })
      }
    } catch (error) {
      if ((error as Error).name !== 'AbortError') {
        this.fail(thread, `运行失败：${(error as Error).message ?? String(error)}`)
      }
    } finally {
      this.runningThreadIds.delete(thread.id)
      this.aborts.delete(thread.id)
      if (this.queues.get(thread.id)?.length === 0) {
        this.queues.delete(thread.id)
      }
      this.notify()
    }
  }

  /**
   * 一轮对话：消息交给 core 的循环，这里只把事件流翻译成界面状态。
   *
   * 与模型来回的完整历史由循环维护（它会在 agent_end 交还整份 messages），
   * 所以 thread.messages 永远是真正发出去过的那份。
   */
  private async turn(thread: Thread, prompt: string, images?: string[]): Promise<void> {
    const config = await readLlmConfig()
    if (!config) {
      await this.offlineTurn(thread, prompt)
      return
    }

    const trimmed = prompt.trim()
    if (
      trimmed === '/compact' ||
      trimmed.startsWith('/compact ') ||
      trimmed === '/summary' ||
      trimmed.startsWith('/summary ')
    ) {
      const customInstructions = trimmed.replace(/^\/(?:compact|summary)\s*/i, '').trim() || undefined
      await this.compactThread(thread.id, { customInstructions, trigger: 'manual' })
      return
    }

    const userMessage: AgentMessage = {
      role: 'user',
      content: prompt,
      images: images && images.length > 0 ? [...images] : undefined,
      timestamp: Date.now(),
    }
    thread.messages.push(userMessage)
    this.persist(thread.id, userMessage)

    const turnStartTime = userMessage.timestamp || Date.now()
    const controller = new AbortController()
    this.aborts.set(thread.id, controller)

    // 扩展得先注册完，这一轮的工具表才不会漏掉它们（刚启动就开始打字也不会漏）。
    await this.extensionsReady

    const currentMode = thread.mode ?? this.mode ?? 'code'
    // 每轮按当前协作模式重新获取工具：plan 模式只读防写，create 模式激活元开发 CRUD 工具
    const tools = defaultToolRegistry.getToolsForMode(thread.workspace, currentMode, {
      parentThreadId: thread.id,
    })
    // 助手行和思考行都等到第一段真的到了才建：思考先行，所以思考行会排在回答上
    // 面；只调工具、不说一句话的那一轮则一行都不留（和以前一样什么都不显示）。
    let assistant: Extract<Item, { kind: 'assistant' }> | null = null
    let reasoning: Extract<Item, { kind: 'thinking' }> | null = null
    /** 思考结束的时刻：回答开始、或这一条消息收尾时定下来。 */
    const endReasoning = (): void => {
      if (reasoning && reasoning.endedAt === undefined) reasoning.endedAt = Date.now()
    }

    const systemPrompt = await defaultPromptManager.getCompositeSystemPrompt(thread.workspace, currentMode)

    const loop = runAgentLoop(thread.messages, config, {
      tools,
      systemPrompt,
      workspace: thread.workspace,
      effort: EFFORT_VALUE[this.effort],
      // 顺序执行：审批一次只该问一件事，命令之间也不该互相抢工作目录。
      toolExecution: 'sequential',
      signal: controller.signal,
      beforeToolCall: (context: BeforeToolCallContext) =>
        this.gate(thread, context.toolCall, controller.signal),
    })

    try {
      for await (const event of loop) {
        // 扩展脚本订阅了生命周期点位，事件原样转发一份
        defaultExtensionLoader.dispatchAgentEvent(event)

        switch (event.type) {
          case 'llm_request':
            this.logLlmRequest(event)
            break

          case 'llm_response':
            this.logLlmResponse(event)
            break

          case 'message_start':
            if (event.message.role === 'assistant') {
              assistant = null
              reasoning = null
            }
            break

          case 'message_update':
            if (event.delta.usage && assistant) {
              assistant.usage = event.delta.usage
              this.notifySoon()
            }
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
                  turnDurationMs: Math.max(1, Date.now() - turnStartTime),
                }
                thread.items.push(assistant)
              }
              assistant.text += event.delta.text
              assistant.turnDurationMs = Math.max(1, Date.now() - turnStartTime)
              this.notifySoon()
            }
            break

          case 'message_end': {
            const message = event.message
            if (message.role !== 'assistant') break
            endReasoning()
            if (assistant) {
              assistant.streaming = false
              if (message.usage) assistant.usage = message.usage
              if (message.durationMs) assistant.durationMs = message.durationMs
              assistant.turnDurationMs = Math.max(1, Date.now() - turnStartTime)
              message.turnDurationMs = assistant.turnDurationMs
            }
            if (message.stopReason === 'error') {
              this.fail(thread, `模型请求失败：${message.errorMessage ?? '未知错误'}`)
            } else if (
              message.content.trim() ||
              message.thinking ||
              (message.toolCalls && message.toolCalls.length > 0)
            ) {
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
              if (event.partialResult.details !== undefined) {
                card.details = { ...card.details, ...(event.partialResult.details as Record<string, any>) }
              }
              this.notifySoon()
            }
            break
          }

          case 'tool_execution_end':
            this.finishToolCall(thread, event.toolCallId, event.result)
            break

          case 'agent_end': {
            thread.messages = event.messages
            const totalTurnDuration = Math.max(1, Date.now() - turnStartTime)
            const lastAssistantItem = thread.items.slice().reverse().find((it): it is Extract<Item, { kind: 'assistant' }> => it.kind === 'assistant')
            if (lastAssistantItem) {
              lastAssistantItem.turnDurationMs = totalTurnDuration
            }
            const lastAssistantMsg = thread.messages.slice().reverse().find((m): m is AssistantMessage => m.role === 'assistant')
            if (lastAssistantMsg) {
              lastAssistantMsg.turnDurationMs = totalTurnDuration
            }
            if (event.reason === 'max_steps') {
              thread.items.push({
                kind: 'notice',
                id: nextId('item'),
                at: Date.now(),
                text: '达到单轮步数上限，已停下。可以继续输入让它接着做。',
                level: 'info',
              })
            }
            this.notify()

            // 检查是否达到自动上下文压缩阈值
            const contextLimit = this.contextWindow > 0 ? this.contextWindow : getModelContextWindow(this.currentModel)
            const currentTokens = lastAssistantItem?.usage?.promptTokens || estimateMessageTokens(thread.messages)
            const compactDecision = shouldAutoCompact({
              messages: thread.messages,
              currentTokens,
              config: { contextWindow: contextLimit },
            })
            if (compactDecision.shouldCompact) {
              setTimeout(() => {
                void this.compactThread(thread.id, { trigger: 'auto' })
              }, 600)
            }
            break
          }
        }
      }
    } finally {
      if (this.aborts.get(thread.id) === controller) {
        this.aborts.delete(thread.id)
      }
    }
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

    const offlineStartTime = Date.now()
    const callId = nextId('call')
    // 离线也要真的跑一次工具：沙箱和界面都被走通了，而不是只留一句说明。
    await this.runToolDirect(thread, {
      id: callId,
      name: 'list_files',
      arguments: { depth: 2 },
      rawArguments: '{"depth":2}',
    })

    const durationMs = Math.max(1, Date.now() - offlineStartTime)
    const info = this.workspaceInfo
    const assistantText = `【离线模式】我扫描了项目 \`${thread.workspace}\`：${info.files} 个文件、${info.dirs} 个目录。配置模型接口后，我会按你的任务在这个目录里读写文件、执行命令。`
    const userMessage: AgentMessage = { role: 'user', content: prompt, timestamp: Date.now() }
    thread.items.push({
      kind: 'assistant',
      id: nextId('item'),
      at: Date.now(),
      text: assistantText,
      durationMs,
      turnDurationMs: durationMs,
    })
    const assistantMessage: AgentMessage = {
      role: 'assistant',
      content: assistantText,
      durationMs,
      turnDurationMs: durationMs,
      toolCalls: [
        {
          id: callId,
          name: 'list_files',
          arguments: { depth: 2 },
          rawArguments: '{"depth":2}',
        },
      ],
      timestamp: Date.now(),
    }
    thread.messages.push(userMessage)
    thread.messages.push(assistantMessage)
    // 离线这一轮也要落盘：历史不该因为没配接口就消失，用户配好之后再打开还得看见。
    this.persist(thread.id, userMessage)
    this.persist(thread.id, assistantMessage)
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
  private async gate(
    thread: Thread,
    call: ToolCallBlock,
    signal?: AbortSignal
  ): Promise<BeforeToolCallResult | undefined> {
    const currentMode = thread.mode ?? this.mode ?? 'code'
    if (currentMode === 'plan' && defaultToolRegistry.isWriteTool(call.name)) {
      const card: ToolCard = {
        kind: 'tool',
        id: nextId('item'),
        at: Date.now(),
        callId: call.id,
        name: call.name,
        args: call.arguments,
        rawArgs: call.rawArguments,
        status: 'denied',
        output: '当前处于 Plan 规划模式，只允许只读分析与方案设计，禁止修改工作区或执行外部命令。请输出方案后提示用户切换到 Code 模式。',
        threadId: thread.id,
      }
      thread.items.push(card)
      this.cards.set(call.id, card)
      this.notify()
      return {
        block: true,
        reason: '当前处于 Plan 规划模式，只允许只读分析与方案设计，禁止修改工作区或执行外部命令。请输出方案后提示用户切换到 Code 模式。',
      }
    }

    const card: ToolCard = {
      kind: 'tool',
      id: nextId('item'),
      at: Date.now(),
      callId: call.id,
      name: call.name,
      args: call.arguments,
      rawArgs: call.rawArguments,
      status: this.needsApproval(call.name) ? 'awaiting' : 'running',
      threadId: thread.id,
    }
    thread.items.push(card)
    this.cards.set(call.id, card)
    this.notify()

    if (card.status !== 'awaiting') return undefined

    const approved = await new Promise<boolean>((resolve) => {
      const finish = (result: boolean) => {
        this.approvals.delete(card.id)
        resolve(result)
      }
      this.approvals.set(card.id, finish)
      if (signal?.aborted) {
        finish(false)
      } else if (signal) {
        signal.addEventListener('abort', () => finish(false), { once: true })
      }
    })
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
    result: { output: string; ok: boolean; patch?: string; details?: any }
  ): void {
    const card = this.cards.get(callId)
    this.cards.delete(callId)

    if (card) {
      card.status = result.ok ? 'done' : 'error'
      card.output = result.output
      card.patch = result.patch
      if (result.details !== undefined) {
        card.details = { ...card.details, ...(result.details as Record<string, any>) }
      }
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
