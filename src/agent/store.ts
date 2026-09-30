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
  readPluginCapabilities,
  readSavedConfig,
  readSavedAppearance,
  testConnection,
  writeSavedAppearance,
  writeSavedConfig,
  type ProviderConfig,
} from './config'
import { runAgentLoop } from './core/agent-loop'
import type { AgentHooks, BeforeApprovalContext, BeforeCompactionResult, SubagentEndContext } from './core/events'
import { composePluginHooks } from './plugins/hook-runtime'
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
import { createNotifyParentTool } from './tools/builtins/subagent'
import { defaultSessionManager } from './session/manager'
import type { SessionSummary } from './session/types'
import { defaultCheckpointManager } from './checkpoint'
import { checkWorkspaceSandbox } from './tools/workspace'
import { patchStats } from './patch'
import { defaultHooks } from './hooks'
import { defaultPromptManager } from './prompts/manager'
import {
  defaultSubagentManager,
  type SubagentProfile,
  type SubagentRunResult,
  type SubagentStepUpdate,
} from './subagents'
import { resolveSubagentTools, runSubagentGate, type SubagentGateOutcome } from './subagents/access'
import { getLoadedPlugins } from './plugins/registry'
import { applyCompactionVerdict } from './compact/verdict'
import { applyAppearance, appearance, type Appearance } from '../theme'
import { getPublicWorkspace, isPublicWorkspace, workspaceLabel } from './home'
import { computeThreadStats, type AgentMode, type AgentQuestion, type DebugEntry, type Item, type Thread, type ThreadStats } from './types'
import {
  selectCompactSelection,
  executeCompaction,
  buildCompactSummaryMessage,
  shouldAutoCompact,
  estimateMessageTokens,
  getModelContextWindow,
} from './compact'
import { showCompletionNotification } from '../platform/notification'

/**
 * `ApprovalMode` / `Effort` / `QueuedItem` 已搬到契约层 `src/shared/protocol`（协议设计 §7.1）：
 * 它们要跨进程交给 UI，不该寄生在实现模块里。这里原样再导出，既有调用点不受影响。
 * 依赖方向是「实现 → 契约」，所以 `shared/protocol` **不会**反向 import 本文件。
 */
import type { ApprovalMode, Effort, QueuedItem } from '../shared/protocol'
export type { ApprovalMode, Effort, QueuedItem }

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

/** 等待子智能体唤醒的默认超时：仅作防死锁兜底，正常由子智能体完成时自动唤醒。 */
export const DEFAULT_SUBAGENT_WAIT_MS = 60 * 60 * 1000

/**
 * 子智能体送回给父智能体的一份唤醒负载。
 *
 * `report` 是子智能体中途主动唤醒（要求主智能体做下一步决策）；`done`/`error` 是它
 * 执行结束时的自动唤醒，保证主智能体不会因为子智能体忘了调用唤醒工具而永久挂起。
 */
export interface SubagentWake {
  /** 子会话 id */
  threadId: string
  subagentId?: string
  /** 子智能体的展示名 */
  name?: string
  /** 带回主智能体的内容 */
  summary: string
  status: 'report' | 'done' | 'error'
  at: number
}

/** 会做检查点的内置写工具：执行前把目标文件快照一份，才有「撤销此次改动」。 */
const CHECKPOINT_TOOLS = new Set(['write_file', 'edit_file', 'edit_files'])

/**
 * 从一次写工具调用里取出它要触碰的所有文件路径。
 *
 * 单文件工具用 args.path；批量工具（edit_files）用 args.files，一次会改多个文件，
 * 每个都得进快照，否则批量改动就没有回滚的退路。
 */
function checkpointPathsOf(name: string, args: Record<string, unknown>): string[] {
  if (name === 'edit_files') {
    const files = Array.isArray(args.files) ? args.files : []
    return files
      .map((entry) => String((entry as { path?: unknown })?.path ?? '').trim())
      .filter(Boolean)
  }
  const single = String(args.path ?? '').trim()
  return single ? [single] : []
}

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
  /**
   * 公共区目录（a-da 自带的工作区），构造时定下来。
   *
   * 存在实例上而不是每次问 `getPublicWorkspace()`：后者读进程级 `A_DA_HOME`，
   * 会让「同一个 store 在不同时刻对公共区的看法」不稳定。
   */
  publicWorkspace: string
  debugOpen = false
  settingsOpen = false
  pluginsOpen = false
  /** 改动审阅面板（逐文件保留/恢复原状）的开关；纯视图状态，不落盘 */
  changesOpen = false
  /** 命令面板（Ctrl+K）；纯视图状态 */
  paletteOpen = false
  /** 侧边栏与搜索框的展开状态：从 AgentWindow 的局部 state 上收过来，
   * 这样窗口级快捷键（Ctrl+B / Ctrl+F）才够得着。纯视图状态。 */
  sidebarOpen = true
  searchOpen = false
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

  /** 开关改动审阅面板 */
  setChangesOpen(open: boolean) {
    this.changesOpen = open
    this.notify()
  }

  /** 开关命令面板 */
  setPaletteOpen(open: boolean) {
    this.paletteOpen = open
    this.notify()
  }

  /** 开关侧边栏（快捷键 Ctrl+B 与标题栏按钮共用） */
  toggleSidebar() {
    this.sidebarOpen = !this.sidebarOpen
    this.notify()
  }

  /** 设置侧边栏搜索框状态 */
  setSearchOpen(open: boolean) {
    this.searchOpen = open
    if (open) this.sidebarOpen = true
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
  /**
   * 等用户回答的 `ask_user` 提问：工具调用 id → 收尾函数。
   *
   * 与 `approvals` 分开：审批问的是"要不要执行"，提问问的是"给个信息"，
   * 两者的卡片与语义都不同，合在一张表里会让"谁在等什么"变成谜。
   */
  private pendingQuestions = new Map<
    string,
    (answer: { answeredBy: 'user' | 'aborted'; choice?: string; text?: string }) => void
  >()
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
  /**
   * 每个父会话至多一个「等待子智能体唤醒」的挂起。
   *
   * 主智能体派发完后台子智能体后不再轮询 check_subagent，而是调 await_subagents
   * 阻塞在这里，等子智能体把结论送回来。挂起是短暂的、只存在于内存：进程退出即
   * 消失，会话本身不受影响。
   */
  private subagentWaits = new Map<
    string,
    {
      watching: Set<string>
      wakes: SubagentWake[]
      /** 已被显式唤醒（子智能体主动要求主智能体做下一步），可立即结束等待 */
      woken: boolean
      startedAt: number
      timer: ReturnType<typeof setTimeout> | null
      resolve: (outcome: { wakes: SubagentWake[]; timedOut: boolean; aborted: boolean }) => void
    }
  >()
  /**
   * 主智能体还没开始等就到达的唤醒负载。
   *
   * 并发派发时子智能体可能远早于主智能体调 await_subagents 就完成了（离线或极快的
   * 任务尤其如此）。这些结论先缓冲，等挂起发生的那一刻立即交付——丢掉它们会让主
   * 智能体误以为子任务还在跑。
   */
  private bufferedWakes = new Map<string, SubagentWake[]>()
  /** 主智能体真正停在那儿等子智能体（仍在 runningThreadIds 里，但不在思考） */
  private waitingThreadIds = new Set<string>()
  /** 最近一次工作区扩展加载：跑一轮之前要等它，工具表才完整。 */
  private extensionsReady: Promise<void> = Promise.resolve()
  private notifyTimer: ReturnType<typeof setTimeout> | null = null
  private logId = 0

  get abort(): AbortController | null {
    return this.aborts.get(this.activeId) ?? null
  }

  get queue(): QueuedItem[] {
    return this.queues.get(this.active.id) ?? []
  }
  set queue(items: QueuedItem[]) {
    const activeId = this.active.id
    if (items.length === 0) {
      this.queues.delete(activeId)
    } else {
      this.queues.set(activeId, items)
    }
  }

  constructor(
    workspace: string = process.env.A_DA_WORKSPACE || process.cwd(),
    publicWorkspace: string = getPublicWorkspace(),
  ) {
    // 公共区路径**在构造时定下来**，之后只认这一份：`A_DA_HOME` 是进程级变量，
    // 运行期反复读取既不必要，也会让并发跑在同一进程里的测试互相踩。
    this.publicWorkspace = publicWorkspace
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
              checkpointId: res?.checkpointId,
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
            checkpointId: message.checkpointId,
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
    const found = this.threads.find((thread) => thread.id === this.activeId)
    if (found) return found
    const fallback = this.threads[0]!
    this.activeId = fallback.id
    return fallback
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
   * 指定会话是否正停在那儿等子智能体唤醒。
   *
   * 这种会话仍在 runningThreadIds 里（那一轮还没结束），但它不在思考、也没在执行
   * 工具，所以界面该把它和「运行中」区分开。
   */
  isThreadWaiting(threadId: string): boolean {
    return this.waitingThreadIds.has(threadId)
  }

  // ------------------------------------------------ 子智能体等待 / 唤醒

  /**
   * 阻塞等待子智能体把结论送回来。
   *
   * 主智能体派发完后台子智能体后调这个（await_subagents 工具），而不是反复
   * check_subagent 轮询——每一次轮询都是一整轮模型请求，要把整个上下文重发一遍。
   *
   * 返回条件（任一满足即返回）：
   * - 某个子智能体**显式唤醒**：它主动要求主智能体做下一步，不必等其余子任务
   * - 看护的子智能体**全部终止**：全部有了结论，等下去没有意义
   * - 超时：仅防死锁兜底
   * - 主轮次被中止
   *
   * 没有任何可等待对象时立即返回，而不是干等到超时——那只会白等一小时。
   */
  suspendForSubagents(
    thread: Thread,
    options: { threadIds?: string[]; timeoutMs?: number; signal?: AbortSignal } = {}
  ): Promise<{ wakes: SubagentWake[]; timedOut: boolean; aborted: boolean }> {
    // 同一父会话同时只允许一个挂起。工具本身是阻塞的，走到这里说明有并发调用，
    // 直接返回而不是覆盖掉前一个等待（覆盖会让前一个 promise 永远悬着）。
    if (this.subagentWaits.has(thread.id)) {
      return Promise.resolve({ wakes: [], timedOut: false, aborted: false })
    }

    // 看护范围：显式指定的子会话；未指定则取该父会话当前所有运行中的子会话。
    // 一个都没在跑时退而取它的全部子会话——这样「事后才想起来等」也能从缓冲或子会话
    // 的最终报告里把结论捞回来，而不是空手返回。
    const watching = new Set<string>()
    if (options.threadIds && options.threadIds.length > 0) {
      for (const id of options.threadIds) {
        const target = this.threads.find((t) => t.id === id)
        if (target) watching.add(target.id)
      }
    } else {
      const children = this.threads.filter((t) => t.parentId === thread.id)
      for (const candidate of children) {
        if (this.isThreadRunning(candidate.id)) watching.add(candidate.id)
      }
      if (watching.size === 0) {
        for (const candidate of children) watching.add(candidate.id)
      }
    }

    // 先收取缓冲里属于看护范围的结论：子智能体可能早就跑完了。
    const buffered = this.takeBufferedWakes(thread.id, watching)
    if (buffered.length > 0) {
      return Promise.resolve({ wakes: buffered, timedOut: false, aborted: false })
    }

    // 没有可等的对象：立刻返回，别干等。
    if (watching.size === 0) {
      return Promise.resolve({ wakes: [], timedOut: false, aborted: false })
    }

    // 看护对象全都已经不在运行了：没有可等的事件，但结论可能已经躺在各自的会话里
    // （自动唤醒只对有等待的父会话投递，其余情况落在缓冲或被丢掉）。这里直接就地
    // 采集，免得调用方为早已完成的子任务再等一次超时。
    const stillRunning = [...watching].filter((id) => this.isThreadRunning(id))
    if (stillRunning.length === 0) {
      const harvested = this.harvestFinishedWakes(watching)
      return Promise.resolve({ wakes: harvested, timedOut: false, aborted: false })
    }

    return new Promise((resolve) => {
      const finish = (outcome: { wakes: SubagentWake[]; timedOut: boolean; aborted: boolean }) => {
        const entry = this.subagentWaits.get(thread.id)
        if (!entry) return
        if (entry.timer) clearTimeout(entry.timer)
        this.subagentWaits.delete(thread.id)
        this.waitingThreadIds.delete(thread.id)
        this.notify()
        resolve(outcome)
      }

      const entry = {
        watching,
        wakes: [] as SubagentWake[],
        woken: false,
        startedAt: Date.now(),
        timer: null as ReturnType<typeof setTimeout> | null,
        resolve: finish,
      }
      this.subagentWaits.set(thread.id, entry)
      this.waitingThreadIds.add(thread.id)
      this.notify()

      const timeoutMs = options.timeoutMs && options.timeoutMs > 0 ? options.timeoutMs : DEFAULT_SUBAGENT_WAIT_MS
      entry.timer = setTimeout(() => {
        const current = this.subagentWaits.get(thread.id)
        if (!current || current !== entry) return
        finish({ wakes: entry.wakes.splice(0, entry.wakes.length), timedOut: true, aborted: false })
      }, timeoutMs)

      if (options.signal) {
        if (options.signal.aborted) {
          finish({ wakes: entry.wakes.splice(0, entry.wakes.length), timedOut: false, aborted: true })
        } else {
          options.signal.addEventListener(
            'abort',
            () => {
              const current = this.subagentWaits.get(thread.id)
              if (!current || current !== entry) return
              finish({ wakes: entry.wakes.splice(0, entry.wakes.length), timedOut: false, aborted: true })
            },
            { once: true }
          )
        }
      }
    })
  }

  /** 从缓冲里取出属于指定看护范围的唤醒负载，取走即删除。 */
  private takeBufferedWakes(parentThreadId: string, watching: Set<string>): SubagentWake[] {
    const buffered = this.bufferedWakes.get(parentThreadId)
    if (!buffered || buffered.length === 0) return []
    const taken: SubagentWake[] = []
    const remaining: SubagentWake[] = []
    for (const wake of buffered) {
      if (watching.has(wake.threadId)) taken.push(wake)
      else remaining.push(wake)
    }
    if (remaining.length > 0) this.bufferedWakes.set(parentThreadId, remaining)
    else this.bufferedWakes.delete(parentThreadId)
    return taken
  }

  /**
   * 就地采集已结束子智能体的结论。
   *
   * 用于「挂起时看护对象都已经结束了」：没有事件可等，但结论未必还在缓冲里（自动唤醒
   * 只投递给有等待的父会话，未等待时那份会进缓冲，但也有可能已被消费）。所以先翻缓冲，
   * 再退一步从子会话的最终报告里取，尽量不让调用方空手而归。
   */
  private harvestFinishedWakes(watching: Set<string>): SubagentWake[] {
    const found = new Map<string, SubagentWake>()
    for (const wake of this.bufferedWakes.values()) {
      for (const item of wake) {
        if (watching.has(item.threadId) && !found.has(item.threadId)) found.set(item.threadId, item)
      }
    }
    for (const id of watching) {
      if (found.has(id)) continue
      const thread = this.threads.find((t) => t.id === id)
      if (!thread) continue
      // 取最后一条助手消息作为它的报告（和 check_subagent 的取法一致）
      const reports = thread.items.filter(
        (it): it is Extract<Item, { kind: 'assistant' }> => it.kind === 'assistant'
      )
      const summary = reports.length > 0 ? reports[reports.length - 1]!.text : ''
      if (!summary) continue
      found.set(id, {
        threadId: id,
        subagentId: thread.subagentId,
        name: thread.title,
        summary,
        status: 'done',
        at: Date.now(),
      })
    }
    return [...found.values()]
  }

  /**
   * 子智能体唤醒父智能体。这是「无需轮询」的另一半：结论由子智能体推回来。
   *
   * 父智能体没在等待时**不启动新轮次**——它可能已经正常收尾，凭空开一轮会烧掉
   * 用户没预期的 token，还会和「任务完成」通知打架。这种情况只缓冲结论 + 留一条
   * 可见记录，并如实告知子智能体。
   */
  wakeParent(wake: SubagentWake): { delivered: boolean; reason: string } {
    const child = this.threads.find((t) => t.id === wake.threadId)
    const parentId = child?.parentId
    if (!parentId) {
      return { delivered: false, reason: '该子会话没有父会话，唤醒请求无处投递。' }
    }
    if (!this.threads.some((t) => t.id === parentId)) {
      return { delivered: false, reason: '父会话已不存在，唤醒请求已丢弃。' }
    }

    const entry = this.subagentWaits.get(parentId)
    if (!entry || !this.waitingThreadIds.has(parentId)) {
      // 父智能体没在等：只回报，不启动新轮次。
      const list = this.bufferedWakes.get(parentId) ?? []
      list.push(wake)
      // 缓冲上限：同父会话最多留 50 条，防止极端情况下无限堆积
      if (list.length > 50) list.splice(0, list.length - 50)
      this.bufferedWakes.set(parentId, list)

      const parentThread = this.threads.find((t) => t.id === parentId)
      if (parentThread) {
        this.push({
          kind: 'info',
          text: `子智能体「${wake.name ?? wake.subagentId ?? wake.threadId}」回报：${wake.summary.slice(0, 80)}`,
        })
        const preview = wake.summary.length > 200 ? `${wake.summary.slice(0, 200)}...` : wake.summary
        parentThread.items.push({
          kind: 'notice',
          level: wake.status === 'error' ? 'error' : 'info',
          id: nextId('item'),
          at: Date.now(),
          text: `子智能体「${wake.name ?? wake.subagentId ?? wake.threadId}」回报内容已记录（父智能体当前未在等待，不会因此启动新一轮）。\n${preview}`,
        })
        this.notify()
      }
      return {
        delivered: false,
        reason: '父智能体当前未挂起等待；内容已记录在父会话中，待其下次等待时交付。',
      }
    }

    // 父智能体正在等：收下这份结论。
    entry.wakes.push(wake)

    // 显式唤醒（子智能体主动要求主智能体做下一步）立即结束等待；完成类唤醒则等
    // 看护对象全部终止——这样主智能体拿到的是一整批结论，而不是零散半份。
    if (wake.status === 'report') {
      entry.woken = true
    }
    const allSettled = [...entry.watching].every((id) => !this.isThreadRunning(id))
    if (entry.woken || allSettled) {
      entry.resolve({ wakes: entry.wakes.splice(0, entry.wakes.length), timedOut: false, aborted: false })
      return { delivered: true, reason: '已唤醒父智能体。' }
    }

    // 还有子任务在跑：结论先攒着，等全部结束一并交付。进度推给父会话卡片。
    this.notify()
    return {
      delivered: true,
      reason: `内容已送达父智能体的等待队列（尚有 ${[...entry.watching].filter((id) => this.isThreadRunning(id)).length} 个子任务在运行，将一并交付）。`,
    }
  }

  /** 清理某个会话的等待与缓冲（会话被删除/移除时调用），避免留下悬挂的 promise。 */
  private discardSubagentWaits(threadId: string): void {
    const entry = this.subagentWaits.get(threadId)
    if (entry) {
      // 交给它自己的 finish 收尾：finish 会清 timer、删表项并 resolve。
      // 这里不能先删表项——finish 开头就是查表，查不到就直接 return，promise 会永远悬着。
      entry.resolve({ wakes: entry.wakes.splice(0, entry.wakes.length), timedOut: false, aborted: true })
    }
    this.waitingThreadIds.delete(threadId)
    this.bufferedWakes.delete(threadId)
    // 子会话被删掉时，父会话缓冲里它的那份也一并清掉
    for (const [parentId, list] of this.bufferedWakes) {
      const remaining = list.filter((wake) => wake.threadId !== threadId)
      if (remaining.length !== list.length) {
        if (remaining.length > 0) this.bufferedWakes.set(parentId, remaining)
        else this.bufferedWakes.delete(parentId)
      }
    }
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

  /**
   * 直接设定（而不是切换）事件日志面板的开合。
   *
   * 为什么需要绝对设定：M3 起 UI 可能从**另一个进程**改这个状态，切换语义会与
   * "客户端以为的当前值"打架（两个客户端同时切就是互相抵消）。切换只留给本机快捷键用。
   */
  setDebugOpen(open: boolean): void {
    if (this.debugOpen === open) return
    this.debugOpen = open
    this.notify()
  }

  toggleDebug(): void {
    this.setDebugOpen(!this.debugOpen)
  }

  /** Append a line to the debug log from anywhere in the UI. */
  trace(text: string): void {
    this.push({ kind: 'info', text })
    this.notify()
  }

  /**
   * 子智能体的统一收尾：退出运行集合 → 跑 `afterSubagentEnd` → 唤醒父会话。
   *
   * **顺序是硬要求**（设计文档 §11 风险 3，也是本次唯一可能造成卡死的缺陷）：
   * 唤醒必须发生在 `runningThreadIds.delete()` **之后**，否则 `suspendForSubagents`
   * 的"看护对象是否全部结束"会算错，父会话可能永远等不到唤醒。`afterSubagentEnd`
   * 插在两者之间，它的异常**绝不能阻断唤醒**——插件是可选增强，不是唤醒链路上的一环。
   *
   * 四处收尾（启动/恢复 × 正常 finally/离线兜底）原先各写了一遍，这份实现把它们收成
   * 一处：任何一处漏掉顺序或漏掉唤醒，都是"父会话永久挂起"级别的故障。
   */
  private async finishSubagent(options: {
    thread: Thread
    profile: SubagentProfile
    parentThread?: Thread | null
    /** 有值才唤醒（离线兜底与"父会话没在等"的情况不唤醒） */
    wake?: { summary: string; status: 'done' | 'error' }
    stepsExecuted?: number
    durationMs?: number
    gate?: SubagentEndContext['gate']
  }): Promise<void> {
    const { thread, profile, wake } = options
    const parentThread = options.parentThread ?? null
    // 收窄成对象：TS 无法从布尔量推出 `wake` 非空
    const pendingWake =
      wake && parentThread && parentThread.id !== thread.id ? { wake, summary: wake.summary } : null

    let summary = pendingWake?.summary ?? ''
    // 钩子跑在**退出运行集合之前**：此时子会话仍算"在跑"，于是父会话若在这段时间
    // 开始等待，会正常等这一次唤醒，而不是走 `suspendForSubagents` 的"看对象都结束
    // 了、直接就地采集"捷径——那条捷径读的是子会话自己的报告，我们这个带复核旁注的
    // 唤醒内容就会被绕过去（实测过：离线兜底路径下子任务几乎瞬时结束，必定撞上）。
    if (pendingWake) {
      try {
        const hooks = await this.composeHooks({ workspace: thread.workspace, kind: 'subagent' })
        if (hooks.afterSubagentEnd) {
          const result = await hooks.afterSubagentEnd({
            kind: 'subagent',
            workspace: thread.workspace,
            threadId: thread.id,
            subagentId: profile.id,
            profileId: profile.id,
            subagentThreadId: thread.id,
            status: pendingWake.wake.status,
            summary: pendingWake.summary,
            stepsExecuted: options.stepsExecuted ?? 0,
            durationMs: options.durationMs ?? 0,
            gate: options.gate,
            trace: (message) => this.trace(message),
          })
          const note = result?.appendParentNote?.trim()
          if (note) {
            // 旁注并进唤醒内容：父智能体是在收到唤醒时才看到它的；只 push 进日志
            // 只有人能看到，模型看不到
            summary = `${summary}\n\n【子智能体复核旁注】\n${note}`
            this.trace(`[插件] afterSubagentEnd 追加了复核旁注（${note.length} 字）`)
          }
        }
      } catch (error) {
        // 绝不让插件打断唤醒
        this.trace(`[插件] afterSubagentEnd 抛错，已忽略：${(error as Error).message}`)
      }
    }

    // 退出运行集合必须在**唤醒之前**：`suspendForSubagents` 靠它判断"看护对象是否
    // 全部结束"，顺序反了父会话可能永远等不到唤醒（设计文档 §11 风险 3）
    this.steeringQueues.delete(thread.id)
    this.runningThreadIds.delete(thread.id)
    this.aborts.delete(thread.id)

    if (pendingWake) {
      this.wakeParent({
        threadId: thread.id,
        subagentId: profile.id,
        name: profile.name,
        summary,
        status: pendingWake.wake.status,
        at: Date.now(),
      })
    }
    this.notify()
  }

  /**
   * 这个工作区里有没有插件在用会话生命周期点位。
   *
   * **同步判断**（读的是已加载插件索引，不是配置文件）：`deleteThread` 在绝大多数
   * 情况下要维持"同步删完"的语义——几十处调用点都是不等它的。有插件参与时才走
   * 异步链路，没有时行为与改动前逐字节相同。
   */
  private usesThreadLifecycleHooks(workspace: string): boolean {
    return getLoadedPlugins(workspace).some((plugin) => {
      const hooks = plugin.contributions.hooks
      if (!hooks) return false
      return Boolean(hooks.beforeThreadDelete || hooks.afterThreadDelete)
    })
  }

  /**
   * 会话建立后的钩子：建议标题、写 pluginData、通知 afterThreadCreate。
   *
   * 异步执行（`newThread` 是同步 API，被几十处调用）：效果在随后一瞬间落地——标题
   * 先显示默认值、随即改成插件建议的，pluginData 也是稍后写进会话与落盘文件。这样做
   * 而不是把 `newThread` 改成 async，是因为后者要改动所有调用点，收益不抵风险。
   */
  private async runThreadCreateHooks(thread: Thread): Promise<void> {
    try {
      const hooks = await this.composeHooks({
        workspace: thread.workspace,
        kind: 'main',
        threadId: thread.id,
        mode: thread.mode ?? this.mode ?? 'code',
      })
      if (hooks.beforeThreadCreate) {
        const verdict = await hooks.beforeThreadCreate({
          kind: 'main',
          workspace: thread.workspace,
          threadId: thread.id,
          isSubagent: false,
          trace: (message) => this.trace(message),
        })
        // 两次头部写入必须**串行**：它们各自是"读文件 → 改第一行 → 写回"，
        // 并发跑会互相把对方那次修改覆盖掉（实测：标题写进去了、pluginData 丢了）
        if (verdict?.title) {
          thread.title = verdict.title
          await defaultSessionManager
            .updateSessionTitle(thread.id, verdict.title, thread.workspace)
            .catch(() => {})
          this.trace(`[插件] 会话标题由插件建议：${verdict.title}`)
        }
        if (verdict?.data) {
          // 插件自己的数据，按插件 id 分键；核心只负责搬运，不解释
          const data = verdict.data as Record<string, unknown>
          thread.pluginData = { ...(thread.pluginData ?? {}), ...data }
          await defaultSessionManager
            .updateSessionMeta(thread.id, { pluginData: data }, thread.workspace)
            .catch(() => {})
          this.trace(`[插件] 已写入 ${Object.keys(data).length} 个插件数据键`)
        }
        this.notify()
      }

      if (hooks.afterThreadCreate) {
        await hooks.afterThreadCreate({
          kind: 'main',
          workspace: thread.workspace,
          threadId: thread.id,
          title: thread.title,
          isSubagent: false,
          trace: (message) => this.trace(message),
        })
      }
    } catch (error) {
      // 插件是可选增强：创建会话这件事不该因为插件出问题而失败
      this.trace(`[插件] 会话创建钩子出错，已忽略：${(error as Error).message}`)
    }
  }

  /**
   * 机械删除：把原 `deleteThread` 的主体原样搬过来，不含任何钩子。
   *
   * 与它配对的 `deleteThread` 负责"要不要删"，这里只负责"怎么删"。
   */
  private deleteThreadNow(id: string): string | null {
    const thread = this.threads.find((candidate) => candidate.id === id)
    if (!thread) return null

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
    // 等待与缓冲是内存态：会话没了就一并清掉，别留下永远不 resolve 的 promise
    this.discardSubagentWaits(id)
    for (const cid of childIds) {
      this.discardSubagentWaits(cid)
    }

    this.push({ kind: 'info', text: `已删除会话「${thread.title}」` })
    void defaultSessionManager.deleteSession(id, thread.workspace).catch(() => {})
    // 检查点流水是会话的附属品，会话没了就一起清
    void defaultCheckpointManager.discard(id).catch(() => {})
    for (const cid of childIds) {
      void defaultCheckpointManager.discard(cid).catch(() => {})
    }

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
   * 会话删除（设计文档 §6.7.2）。
   *
   * **没有插件参与时是同步完成的**：开头那次判断不涉及 await，所以整个删除动作在
   * 第一次 `await` 之前就做完了——几十处不等返回值的调用点因此完全不受影响。
   * 只有真的有插件在用 `beforeThreadDelete`/`afterThreadDelete` 时才走异步链路：
   * 先问（可 block、可先归档），再删，最后通知。
   */
  async deleteThread(id: string): Promise<string | null> {
    const thread = this.threads.find((candidate) => candidate.id === id)
    if (!thread) return null
    if (this.isThreadRunning(id)) return '这个会话正在运行，先停止再删除'

    // 级联的子会话（勾子要对每一个都调一次，见 §10 的验收）
    const children = this.threads.filter((candidate) => candidate.parentId === id)

    if (!this.usesThreadLifecycleHooks(thread.workspace)) {
      this.deleteThreadNow(id)
      return null
    }

    const hooks = await this.composeHooks({
      workspace: thread.workspace,
      kind: 'main',
      threadId: thread.id,
    })

    const targets: Array<{ thread: Thread; cascaded: boolean }> = [
      { thread, cascaded: false },
      ...children.map((child) => ({ thread: child, cascaded: true })),
    ]

    let archiveRequested = false
    let archived = false
    const blocked: Array<{ title: string; reason: string }> = []

    for (const target of targets) {
      if (!hooks.beforeThreadDelete) break
      const verdict = await hooks.beforeThreadDelete({
        kind: 'main',
        workspace: target.thread.workspace,
        threadId: target.thread.id,
        title: target.thread.title,
        isSubagent: Boolean(target.thread.isSubagent),
        cascaded: target.cascaded,
        trace: (message) => this.trace(message),
      })
      if (!verdict) continue
      if (verdict.archiveBeforeDelete) archiveRequested = true
      if (verdict.block) {
        blocked.push({
          title: target.thread.title,
          reason: verdict.blockReason ?? `插件「${verdict.blockedBy ?? '未知'}」阻止了删除`,
        })
      }
    }

    if (blocked.length > 0) {
      const reason = blocked[0]!.reason
      this.push({ kind: 'info', text: `删除被插件拦下：${reason}` })
      // 拦下也要跑 after*：插件可能在事前分配了资源
      if (hooks.afterThreadDelete) {
        for (const target of targets) {
          await hooks.afterThreadDelete({
            kind: 'main',
            workspace: target.thread.workspace,
            threadId: target.thread.id,
            title: target.thread.title,
            isSubagent: Boolean(target.thread.isSubagent),
            cascaded: target.cascaded,
            blocked: true,
            archived: false,
            trace: (message) => this.trace(message),
          })
        }
      }
      this.notify()
      return reason
    }

    if (archiveRequested) {
      for (const target of targets) {
        const ok = await defaultSessionManager
          .archiveSession(target.thread.id, target.thread.workspace)
          .catch(() => false)
        archived = archived || ok
      }
      if (archived) this.trace('[插件] 已在删除前归档会话副本（*.jsonl.archived）')
    }

    this.deleteThreadNow(id)

    if (hooks.afterThreadDelete) {
      for (const target of targets) {
        await hooks.afterThreadDelete({
          kind: 'main',
          workspace: target.thread.workspace,
          threadId: target.thread.id,
          title: target.thread.title,
          isSubagent: Boolean(target.thread.isSubagent),
          cascaded: target.cascaded,
          blocked: false,
          archived,
          trace: (message) => this.trace(message),
        })
      }
    }
    return null
  }

  /**
   * 跑一次子智能体启动门禁（设计文档 §6.3）。
   *
   * 判定本身交给插件的 `beforeSubagentStart`（核心不该内置"怎么判断"），核心只做两件事：
   * 把工具集授权集合交给判定方（结果只能收窄），以及决定**拿不到判定时**的失败方向。
   *
   * 返回 `undefined` 表示这个 profile 没配 `gate.criteria`——正常情况，不是失败。
   */
  private async gateSubagent(
    profile: SubagentProfile,
    task: string,
    workspace: string
  ): Promise<SubagentGateOutcome | undefined> {
    if (!profile.gate?.criteria?.trim()) return undefined

    const hooks = await this.composeHooks({ workspace, kind: 'subagent' })
    const outcome = await runSubagentGate({
      profile,
      task,
      // 授权集合用同一个解析函数算，保证"门禁能挑的"与"子智能体本来能用的"完全一致
      authorizedTools: resolveSubagentTools(profile, workspace),
      hooks,
      workspace,
      notice: (message) => this.trace(message),
    })

    if (outcome && !outcome.allowed) {
      // 门禁拦下：**在建会话之前**抛出，所以不会有子会话、不会进 runningThreadIds，
      // 也就不存在"父会话等着一个永远不会来的唤醒"（设计文档 §11 风险 3）。
      throw new Error(
        `子智能体「${profile.name}」未通过启动门禁：${outcome.reason ?? '判定不通过'}。` +
          `（判定依据：${profile.gate.criteria}）如需放行，请调整该子智能体的 gate 配置。`
      )
    }
    if (outcome) {
      this.trace(
        `[子智能体门禁] 「${profile.name}」通过（judged=${outcome.judged}${
          outcome.calibrated === undefined ? '' : `, calibrated=${outcome.calibrated}`
        }）`
      )
    }
    return outcome
  }

  /**
   * 恢复执行时给门禁的判定输入：**原始任务 + 本次恢复指示**（设计文档 §6.3）。
   *
   * 首轮的委派任务在 `startSubagentThread` 里就被消耗掉了，而恢复指示通常只有一句
   * "网络恢复了，继续"——判定方光看它无从判断这件事该不该接着跑。所以要把会话里
   * 第一条 user 消息（原始委派任务）翻出来一起给；原始任务可能很长（含参考上下文），
   * 截断到判定方真正需要的量级。
   */
  private resumeGateTask(thread: Thread, instruction?: string): string {
    const original = thread.messages.find((message) => message.role === 'user')?.content ?? ''
    const resume = instruction?.trim()
    return [
      original ? `【原始任务】\n${original.slice(0, 4000)}` : '',
      resume ? `【本次恢复指示】\n${resume}` : '',
    ]
      .filter(Boolean)
      .join('\n\n')
  }

  /**
   * 合成这一轮要用的插件钩子（设计文档 §6.4）。
   *
   * **每轮重新合成**：插件可能中途被启停、能力开关可能被改，这些都不该要求重启应用。
   * 合成本身很便宜（过滤 + 闭包），贵的是插件自己的钩子体，那部分由运行层计时与超时。
   *
   * 出错时返回空对象——所有点位整体跳过，等于"没有插件"，且会说一声。插件是可选
   * 增强，绝不该让主循环起不来。
   */
  private async composeHooks(options: {
    workspace: string
    kind: 'main' | 'subagent'
    threadId?: string
    subagentId?: string
    mode?: AgentMode
  }): Promise<AgentHooks> {
    try {
      const capabilities = await readPluginCapabilities(options.workspace)
      for (const key of capabilities.invalid) {
        this.trace(`[插件] 能力开关 ${key} 的值不可用，已按默认值处理`)
      }
      return composePluginHooks({
        ...options,
        capabilities,
        trace: (message) => this.trace(message),
      })
    } catch (error) {
      this.trace(`[插件] 钩子合成失败，本轮按无钩子运行：${(error as Error).message}`)
      return {}
    }
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
    this.push({ kind: 'info', text: `新建会话 · ${this.labelFor(workspace)}` })
    void this.refresh()
    // 建会话的钩子异步落地：标题先显示默认值、随即可能被插件建议改掉，
    // pluginData 也是稍后写进会话与落盘文件。保持 newThread 同步是有意的——
    // 它有几十处调用点，改成 async 的收益不抵风险
    void this.runThreadCreateHooks(thread)
    this.notify()
    return thread
  }

  /** 工作区展示名（公共区显示为「公共区」，其余是短路径）。 */
  labelFor(workspace: string): string {
    return workspaceLabel(workspace, this.publicWorkspace)
  }

  /** 这个路径是不是公共区。 */
  isPublic(workspace: string): boolean {
    return isPublicWorkspace(workspace, this.publicWorkspace)
  }

  /**
   * 打开公共区：a-da 自带的工作区，首次使用会把目录建出来。
   *
   * 它和普通工作区走同一条路（同一份会话落盘、同一套工具沙箱），区别只在目录由
   * a-da 提供。`threadId` 给了就把那个会话绑过去（居中新建界面上换工作区就是这种），
   * 不给就新开一个会话——两者都对应「新建对话时选公共区」。
   */
  async openPublicWorkspace(threadId?: string): Promise<void> {
    const target = this.publicWorkspace
    try {
      await mkdir(target, { recursive: true })
    } catch (error) {
      this.push({ kind: 'error', text: `无法创建公共区目录：${(error as Error).message}` })
      this.notify()
      return
    }
    if (threadId) this.setThreadWorkspace(threadId, target)
    else this.newThread(target)
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
      this.push({ kind: 'info', text: `工作区已存在，已切换至「${this.labelFor(existing)}」` })
      return null
    }

    this.newThread(normalized)
    this.push({ kind: 'info', text: `已添加并打开工作区「${this.labelFor(normalized)}」` })
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
    if (nextThread) void this.runThreadSwitchHook(nextThread)
    this.notify()
  }

  /**
   * 会话切换的**纯通知**钩子：没有返回值、不能阻止切换。
   *
   * 刻意不成对（§6.0 的边界）：切换是瞬时事件，没有"后续状态"可观察。
   */
  private async runThreadSwitchHook(thread: Thread): Promise<void> {
    try {
      const hooks = await this.composeHooks({
        workspace: thread.workspace,
        kind: 'main',
        threadId: thread.id,
      })
      if (!hooks.onThreadSwitch) return
      await hooks.onThreadSwitch({
        kind: 'main',
        workspace: thread.workspace,
        threadId: thread.id,
        isSubagent: Boolean(thread.isSubagent),
        trace: (message) => this.trace(message),
      })
    } catch (error) {
      this.trace(`[插件] onThreadSwitch 抛错，已忽略：${(error as Error).message}`)
    }
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

    // 公共区由 a-da 提供，不是一个可增删的项目：给界面兜底，别让调用方绕过 UI 把它删了。
    if (isPublicWorkspace(workspace, this.publicWorkspace)) return '公共区由 a-da 提供，不能移除'

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
      // 内存里的等待/缓冲同样要清，否则被移除会话的等待会一直悬着
      this.discardSubagentWaits(t.id)
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
    // 该工作区所有会话的检查点流水一并清理
    for (const t of doomed) {
      void defaultCheckpointManager.discard(t.id).catch(() => {})
    }
    this.push({ kind: 'info', text: `已移除工作区「${this.labelFor(workspace)}」` })
    this.notify()
    return null
  }

  // ------------------------------------------------------------------ messages

  send(text: string, images?: string[]): void {
    const prompt = text.trim()
    if (!prompt && (!images || images.length === 0)) return
    const thread = this.active

    // 子智能体会话**不接受直接输入**（拍板结论）：那条路既不过门禁、也拿不到 profile 白名单，
    // 让用户以为"能跟子智能体直接说话"只会得到一次静默越权。挡在这里并指路，
    // 而不是让 `send → drain → turn` 用主会话工具表把它跑掉。
    // （`turn()` 里还有第二道，见那里的注释：命令通道也挡得住。）
    if (thread.isSubagent) {
      this.push({
        kind: 'info',
        text:
          `「${thread.title}」是子智能体专属执行会话，不接受直接输入。` +
          `请在主会话里让它用 send_subagent_message / resume_subagent 工具传达指令，` +
          `或用子智能体卡片上的「恢复执行」。`,
      })
      this.trace(`[子智能体] 拒绝向子智能体会话直接发送输入（threadId=${thread.id}）`)
      this.notify()
      return
    }

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

  // ---------------------------------------------------------------- 智能体向用户提问

  /**
   * 智能体通过 `ask_user` 工具提问并**挂起等待**用户作答。
   *
   * 与审批闸门（`waitForUserApproval`）刻意分开两套等待：审批是"要不要执行这个
   * 调用"，提问是"这个调用需要用户给个信息"。两者的卡片、语义、超时策略都不同，
   * 混在一起会让"谁在等什么"变成谜。
   *
   * 几条刻意的规则：
   * - **子智能体不能直接问用户**：并发派发时多个子智能体同时提问会让用户不知道
   *   在回答谁，且它们本来就该经 `notify_parent` 把问题交给主智能体。这里如实拒绝，
   *   并告诉它正确的做法。
   * - **不设超时**：与审批一致，用户没答就是没答；会话中止（`signal`）会收尾，
   *   不会留下悬挂的 promise。
   * - 问题记在**工具卡片**的 `details.question` 上（不新增 Item 种类）：卡片本来
   *   就属于这次调用，问题与答案都随它留在会话历史里。
   */
  async requestUserAnswer(options: {
    callId: string
    question: string
    choices?: Array<{ id: string; label: string; description?: string }>
    allowText?: boolean
    signal?: AbortSignal
  }): Promise<{ answeredBy: 'user' | 'aborted'; choice?: string; text?: string }> {
    const card = this.cards.get(options.callId)
    const thread = card?.threadId
      ? this.threads.find((candidate) => candidate.id === card.threadId)
      : undefined

    // 两种情况分开报：合并成一句会让"没有卡片"（例如工具在非会话上下文里被调用）
    // 被误读成"你是子智能体"，用户与模型都会照着一个错误的归因去改。
    if (!thread) {
      throw new Error(
        '这次调用没有关联到会话，无法向用户提问。若你是子智能体，请改用 notify_parent ' +
          '把问题与选项交给主智能体，由它来问。'
      )
    }
    if (thread.isSubagent) {
      throw new Error(
        '子智能体不能直接向用户提问：并发派发时用户无法知道在回答哪一个。' +
          '请改用 notify_parent 把问题与选项交给主智能体，由它来问。'
      )
    }

    const record: AgentQuestion = {
      question: options.question,
      choices: options.choices,
      allowText: options.allowText,
      status: 'pending',
      askedAt: Date.now(),
    }
    if (card) {
      card.details = { ...card.details, question: record }
      this.notify()
    }

    return new Promise((resolve) => {
      const finish = (answer: { answeredBy: 'user' | 'aborted'; choice?: string; text?: string }) => {
        this.pendingQuestions.delete(options.callId)
        if (card) {
          // 把终态写回卡片：界面据此从"等你回答"切到"已回答"。
          // 卡片随后会被 finishToolCall 收尾，但 details 会留在会话历史里。
          card.details = {
            ...card.details,
            question: { ...record, status: answer.answeredBy === 'user' ? 'answered' : 'aborted', answer },
          }
        }
        this.notify()
        resolve(answer)
      }

      this.pendingQuestions.set(options.callId, finish)
      if (options.signal?.aborted) {
        finish({ answeredBy: 'aborted' })
      } else {
        options.signal?.addEventListener('abort', () => finish({ answeredBy: 'aborted' }), { once: true })
      }
    })
  }

  /** 用户在界面上作答（点选项或填文本框）。没有待答的问题时静默忽略。 */
  answerQuestion(callId: string, answer: { choice?: string; text?: string }): void {
    const resolve = this.pendingQuestions.get(callId)
    if (!resolve) return
    this.pendingQuestions.delete(callId)
    resolve({ answeredBy: 'user', ...answer })
  }

  /** 这次调用是否正等着用户回答（界面据此展示可交互的问题卡）。 */
  isAwaitingAnswer(callId: string): boolean {
    return this.pendingQuestions.has(callId)
  }

  /**
   * 当前会话里正挂着的提问（`ask_user`），按发起顺序返回。
   *
   * 界面用它在输入框上方浮动出问答卡——与审批不同，提问可能在**同一会话里
   * 同时存在多个**（一次问全几件事），所以返回列表而不是单个。
   *
   * 判定的事实来源是 `pendingQuestions`（"工具真的还挂着"），而不是卡片的
   * `status`：卡片状态要到工具收尾才改写，中间任何一次误标都会让界面上多出一个
   * 点不动的问答卡——而它对应的等待其实早已结束。反过来，只有卡片能提供问题
   * 文案，所以两者都要：句柄定"还在等"，卡片取"问的是什么"。
   *
   * 面板挂在输入框上、属于**当前**会话，因此按卡片上的 `threadId` 过滤：
   * 并发会话各自的问题在各自标签页里等，不该窜到别人的输入框上方。
   */
  get pendingAnswerQuestions(): Array<{ callId: string; question: AgentQuestion }> {
    if (this.pendingQuestions.size === 0) return []
    const activeId = this.active.id
    const pending: Array<{ callId: string; question: AgentQuestion }> = []
    for (const callId of this.pendingQuestions.keys()) {
      const card = this.cards.get(callId)
      if (!card) continue
      if (card.threadId && card.threadId !== activeId) continue
      const question = (card.details as { question?: AgentQuestion } | undefined)?.question
      if (!question || question.status !== 'pending') continue
      pending.push({ callId, question })
    }
    return pending
  }

  // ---------------------------------------------------------------- 改动回滚

  /** 会话里有多少个「仍有效」的文件改动（审阅入口的角标用）。 */
  getThreadChangeCount(threadId: string): number {
    return this.getThreadFileChanges(threadId).filter((change) => !change.reverted).length
  }

  /**
   * 汇总一个会话的文件改动（write_file / edit_file 卡片），按文件聚合：
   * 每个文件保留最近一次的 patch 与累计改动量，供改动审阅面板逐文件
   * 「保留 / 恢复原状」。run_command 里的改动不在此列。
   */
  getThreadFileChanges(threadId: string): Array<{
    path: string
    latestPatch: string
    additions: number
    deletions: number
    editsCount: number
    reverted: boolean
    cardIds: string[]
  }> {
    const thread = this.threads.find((t) => t.id === threadId)
    if (!thread) return []
    const byPath = new Map<
      string,
      { path: string; latestPatch: string; additions: number; deletions: number; editsCount: number; reverted: boolean; cardIds: string[] }
    >()

    /** 把一次调用里某个文件的 patch 累加进聚合表。 */
    const accumulate = (
      path: string,
      patch: string | undefined,
      item: ToolCard
    ): void => {
      if (!path) return
      const stats = patch ? patchStats(patch) : { added: 0, removed: 0 }
      const existing = byPath.get(path)
      if (existing) {
        existing.editsCount += 1
        existing.additions += stats.added
        existing.deletions += stats.removed
        existing.reverted = existing.reverted && Boolean(item.reverted)
        if (!existing.cardIds.includes(item.id)) existing.cardIds.push(item.id)
        if (patch) existing.latestPatch = patch
      } else {
        byPath.set(path, {
          path,
          latestPatch: patch ?? '',
          additions: stats.added,
          deletions: stats.removed,
          editsCount: 1,
          reverted: Boolean(item.reverted),
          cardIds: [item.id],
        })
      }
    }

    for (const item of thread.items) {
      if (item.kind !== 'tool') continue
      if (item.name !== 'write_file' && item.name !== 'edit_file' && item.name !== 'edit_files') continue
      if (item.status !== 'done' && item.status !== 'error') continue

      // 批量编辑：一次调用动多个文件，每个文件带自己的分段 patch，
      // 拆开逐文件入账，改动审阅面板才能逐个文件看 diff 与回滚。
      if (item.name === 'edit_files') {
        const files = Array.isArray((item.details as any)?.files) ? ((item.details as any).files as any[]) : []
        if (files.length > 0) {
          for (const entry of files) {
            accumulate(String(entry?.path ?? '').replace(/\\/g, '/'), entry?.patch, item)
          }
          continue
        }
        // 老流水没有 details.files：退回按参数里的路径记账，至少不丢文件
        for (const relative of checkpointPathsOf(item.name, item.args)) {
          accumulate(relative.replace(/\\/g, '/'), undefined, item)
        }
        continue
      }

      const path = String(item.args?.path ?? '').replace(/\\/g, '/')
      accumulate(path, item.patch, item)
    }
    return [...byPath.values()]
  }

  private revertGuard(threadId: string): Thread | null {
    const thread = this.threads.find((t) => t.id === threadId)
    if (!thread) return null
    if (this.isThreadRunning(threadId)) {
      this.push({ kind: 'info', text: '会话正在运行，先停止再回滚改动。' })
      this.notify()
      return null
    }
    return thread
  }

  /** 把回滚结果标回卡片（checkpointId 命中被作废的记录就算已撤销）并通知界面。 */
  private markCardsReverted(thread: Thread, invalidated: string[]): void {
    if (invalidated.length === 0) return
    const set = new Set(invalidated)
    for (const item of thread.items) {
      if (item.kind === 'tool' && item.checkpointId && set.has(item.checkpointId)) {
        item.reverted = true
      }
    }
  }

  /** 撤销单次写工具调用：恢复那张卡片快照里的文件内容。 */
  async revertCard(threadId: string, cardId: string): Promise<boolean> {
    const thread = this.revertGuard(threadId)
    if (!thread) return false
    const card = thread.items.find((it): it is ToolCard => it.kind === 'tool' && it.id === cardId)
    if (!card?.checkpointId || card.reverted) return false

    const outcome = await defaultCheckpointManager.revertCheckpoint(threadId, card.checkpointId)
    if (!outcome) return false
    this.markCardsReverted(thread, outcome.invalidated)
    const summary = this.describeRevertOutcome(outcome)
    this.push({ kind: 'info', text: `已撤销 ${card.name} 的改动${summary}` })
    this.notify()
    return true
  }

  /** 把一个文件恢复到 Agent 第一次修改它之前的样子。 */
  async revertFile(threadId: string, path: string): Promise<boolean> {
    const thread = this.revertGuard(threadId)
    if (!thread) return false
    let absolute: string
    try {
      absolute = checkWorkspaceSandbox(thread.workspace, path)
    } catch (error) {
      // 文件可能已被删除（新建后又回滚），沙箱仍能按真实落点判断；彻底越界才拦
      this.push({ kind: 'error', text: `无法回滚 ${path}：${(error as Error).message}` })
      this.notify()
      return false
    }
    const outcome = await defaultCheckpointManager.revertFile(threadId, absolute)
    if (!outcome) return false
    this.markCardsReverted(thread, outcome.invalidated)
    this.push({ kind: 'info', text: `已把 ${path} 恢复到改动前${this.describeRevertOutcome(outcome)}` })
    this.notify()
    return true
  }

  /** 一键恢复：撤销本会话 Agent 造成的一切被跟踪的文件改动。 */
  async revertAllChanges(threadId: string): Promise<boolean> {
    const thread = this.revertGuard(threadId)
    if (!thread) return false
    const outcome = await defaultCheckpointManager.revertAll(threadId)
    if (!outcome) {
      this.push({ kind: 'info', text: '没有可回滚的改动。' })
      this.notify()
      return false
    }
    this.markCardsReverted(thread, outcome.invalidated)
    this.push({ kind: 'info', text: `已恢复本会话的全部文件改动${this.describeRevertOutcome(outcome)}` })
    this.notify()
    return true
  }

  private describeRevertOutcome(outcome: { restored: string[]; deleted: string[]; skipped: string[] }): string {
    const parts: string[] = []
    if (outcome.restored.length) parts.push(`恢复 ${outcome.restored.length} 个文件`)
    if (outcome.deleted.length) parts.push(`删除 ${outcome.deleted.length} 个新文件`)
    if (outcome.skipped.length) parts.push(`${outcome.skipped.length} 个文件过大未能还原`)
    return parts.length ? `（${parts.join('，')}）` : ''
  }

  /**
   * 立即发送队列中的指定消息：
   * 将选中的排队消息提升至队首，并中止当前正在运行的轮次，使执行引擎立即处理该消息。
   */
  sendQueuedImmediately(index: number, threadId?: string): void {
    const targetId = threadId ?? this.active.id
    const thread = this.threads.find((t) => t.id === targetId)
    if (!thread) return
    const threadQueue = this.queues.get(targetId)
    if (!threadQueue || index < 0 || index >= threadQueue.length) return

    // 取出指定排队项并提升至队首
    const [targetItem] = threadQueue.splice(index, 1)
    if (!targetItem) return
    threadQueue.unshift(targetItem)

    // 同步调整在 thread.items 中的排列位置，保证与执行顺序一致
    const itemIndex = thread.items.findIndex((it) => it.id === targetItem.item.id)
    if (itemIndex > -1) {
      const [it] = thread.items.splice(itemIndex, 1)
      const firstQueuedIndex = thread.items.findIndex((candidate) => candidate.kind === 'user' && candidate.queued)
      if (firstQueuedIndex > -1) {
        thread.items.splice(firstQueuedIndex, 0, it)
      } else {
        thread.items.push(it)
      }
    }

    if (this.isThreadRunning(targetId)) {
      this.push({
        kind: 'info',
        text: `已插队立即发送：「${targetItem.text.slice(0, 30)}${targetItem.text.length > 30 ? '...' : ''}」`,
      })
      // 中止当前轮次，drain 循环自动进入下一轮执行被提前的 targetItem
      const controller = this.aborts.get(targetId)
      if (controller) {
        controller.abort()
      }
    } else {
      void this.drain(thread)
    }
    this.notify()
  }

  /**
   * 移出队列中的指定排队消息
   */
  removeQueuedItem(index: number, threadId?: string): { text: string; images?: string[] } | null {
    const targetId = threadId ?? this.active.id
    const thread = this.threads.find((t) => t.id === targetId)
    const threadQueue = this.queues.get(targetId)
    if (!threadQueue || index < 0 || index >= threadQueue.length) return null

    const [removed] = threadQueue.splice(index, 1)
    if (!removed) return null

    if (thread) {
      thread.items = thread.items.filter((it) => it.id !== removed.item.id)
    }
    if (threadQueue.length === 0) {
      this.queues.delete(targetId)
    }
    this.push({ kind: 'info', text: '已移出排队消息' })
    this.notify()
    return { text: removed.text, images: removed.images }
  }

  /**
   * 清空指定会话的全部排队消息
   */
  clearQueue(threadId?: string): void {
    const targetId = threadId ?? this.active.id
    const thread = this.threads.find((t) => t.id === targetId)
    const threadQueue = this.queues.get(targetId)
    if (!threadQueue || threadQueue.length === 0) return

    const queuedItemIds = new Set(threadQueue.map((q) => q.item?.id).filter(Boolean))
    if (thread) {
      thread.items = thread.items.filter((it) => !queuedItemIds.has(it.id))
    }
    this.queues.delete(targetId)
    this.push({ kind: 'info', text: '已清空所有排队消息' })
    this.notify()
  }

  /**
   * 编辑已经发送的用户消息并重新发送：
   * 将该消息之后的所有历史项（包括之后的助手回复、工具调用、思考过程以及后续用户消息）全部丢弃，
   * 并在当前截断点重新发起一轮执行。
   */
  async editUserMessageAndResend(
    itemId: string,
    newText: string,
    images?: string[],
    threadId?: string,
  ): Promise<void> {
    const targetId = threadId ?? this.active.id
    const thread = this.threads.find((t) => t.id === targetId)
    if (!thread) return

    const targetItemIndex = thread.items.findIndex((it) => it.id === itemId)
    if (targetItemIndex < 0) return

    // 1. 若当前会话正在运行，中止当前执行
    if (this.isThreadRunning(targetId)) {
      this.aborts.get(targetId)?.abort()
      await new Promise((r) => setTimeout(r, 60))
    }

    // 2. 清空该会话所有待发送的排队任务
    this.clearQueue(targetId)

    // 3. 计算在当前被编辑的消息之前一共有多少条已发送的用户消息
    const userCountBefore = thread.items
      .slice(0, targetItemIndex)
      .filter((it) => it.kind === 'user').length

    // 4. 在 thread.messages 中截断：保留前 userCountBefore 条用户消息及其对应的轮次历史
    let userFound = 0
    let msgTruncateIndex = thread.messages.length
    for (let i = 0; i < thread.messages.length; i++) {
      if (thread.messages[i]?.role === 'user') {
        if (userFound === userCountBefore) {
          msgTruncateIndex = i
          break
        }
        userFound++
      }
    }
    thread.messages = thread.messages.slice(0, msgTruncateIndex)

    // 5. 在 thread.items 中截断：丢弃 targetItemIndex 及其之后的所有内容
    thread.items = thread.items.slice(0, targetItemIndex)

    // 6. 重写会话磁盘持久化记录（丢弃截断之后的消息流水）
    void defaultSessionManager.rewriteSessionMessages(
      thread.id,
      thread.messages,
      thread.workspace,
    ).catch(() => {})

    this.push({ kind: 'info', text: '已更新用户指令，重新生成回复' })
    this.notify()

    // 7. 发送修改后的新消息（作为该截断点的新一轮开始执行）
    this.send(newText, images)
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

    // 启动门禁：插在 enabled 检查之后、建会话之前（设计文档 core 阶段 C）。
    // 判定由插件提供（决策插件用它的引擎），核心只管失败方向与把结论透传出去。
    const gateOutcome = await this.gateSubagent(profile, options.task, workspace)
    const gateVerdict: SubagentEndContext['gate'] = gateOutcome
      ? {
          allowed: gateOutcome.allowed,
          judged: gateOutcome.judged,
          reason: gateOutcome.reason,
          calibrated: gateOutcome.calibrated,
        }
      : undefined

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

    // 工具解析（白名单/黑名单/通配符/只读/防递归）与 runner 共用一份实现，
    // 免得"只读模式漏了个工具"只在某一条入口上出现
    let subagentTools = resolveSubagentTools(profile, workspace)
    // 门禁给的 tools 已经按授权集合裁过（access.ts），这里直接用
    if (gateOutcome?.tools && gateOutcome.tools.length > 0) {
      subagentTools = gateOutcome.tools
      this.trace(`[子智能体门禁] 「${profile.name}」的工具表按门禁结论收窄为 ${subagentTools.length} 个`)
    }
    // notify_parent 是「作为子智能体运行」自带的能力，不是普通工具：它绕过了 profile
    // 白名单（否则只读 profile 就唤醒不了父智能体，整个委派机制就断了），也刻意不在
    // 通用工具表里（主智能体调它没有意义）。会话 id 在这里注入，避免运行时认错人。
    subagentTools.push(createNotifyParentTool(subagentThread.id))

    const steeringQueue: AgentMessage[] = []
    this.steeringQueues.set(subagentThread.id, steeringQueue)

    const resultPromise = (async (): Promise<SubagentRunResult> => {
      const startTime = Date.now()
      const maxSteps = profile.maxSteps
      let stepsExecuted = 0
      let toolCallsCount = 0
      let lastAssistantMessage: any = null
      let latestSummary = ''
      /** 结束时交给父智能体的结论（唤醒兜底用），见 finally。 */
      let wakeStatus: 'done' | 'error' = 'done'
      let wakeSummary = ''

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
        // 离线兜底路径从 return 出去、走不到 finally：收尾（含唤醒兜底）得在这里补上，
        // 否则主智能体等着一个永远不会到来的唤醒。
        await this.finishSubagent({
          thread: subagentThread,
          profile,
          parentThread,
          wake: options.onStepUpdate ? { summary: fallbackText, status: 'done' } : undefined,
          stepsExecuted: 1,
          durationMs: Date.now() - startTime,
          gate: gateVerdict,
        })
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
        // 子智能体循环同样派发钩子（设计文档 §5.5）：kind/subagentId 让插件能分辨
        // "这次是主会话还是某个子智能体的回合"
        const hooks = await this.composeHooks({
          workspace: subagentThread.workspace,
          kind: 'subagent',
          threadId: subagentThread.id,
          subagentId: profile.id,
        })
        const loop = runAgentLoop(subagentThread.messages, config, {
          systemPrompt: profile.systemPrompt,
          tools: subagentTools,
          maxSteps,
          effort: profile.modelOverride?.effort ?? 'high',
          toolExecution: 'sequential',
          signal: controller.signal,
          hooks,
          hookContext: { kind: 'subagent', threadId: subagentThread.id, subagentId: profile.id },
          onNotice: (message) => this.trace(message),
          getSteeringMessages: async () => {
            if (steeringQueue.length === 0) return []
            return steeringQueue.splice(0, steeringQueue.length)
          },
          beforeToolCall: async (context) => {
            if (profile.mode === 'readonly' && defaultToolRegistry.isWriteTool(context.toolCall.name)) {
              return { block: true, reason: `子智能体 ${profile.name} 运行在只读安全模式下，禁止执行写操作。` }
            }
            // 子智能体同样在工作区里写文件，改动一样要留回滚的退路
            await this.captureCheckpoint(subagentThread, context.toolCall)
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
        // 唤醒兜底在 finally 里发（那时它已移出 runningThreadIds，全部终止才算得准）
        wakeStatus = isOk ? 'done' : 'error'
        wakeSummary = `${resultText}${outputFile ? `\n\n完整报告：${outputFile}` : ''}`

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
        wakeStatus = 'error'
        wakeSummary = `执行异常中断：${errorMessage}`
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
        // 自动唤醒：子智能体跑完（含失败）时把结论推给正等待的父智能体。没有它的话，
        // 子智能体一旦忘了调 notify_parent，主智能体就会一直挂到超时。
        await this.finishSubagent({
          thread: subagentThread,
          profile,
          parentThread,
          wake:
            options.onStepUpdate && wakeSummary
              ? { summary: wakeSummary, status: wakeStatus }
              : undefined,
          stepsExecuted,
          durationMs: Date.now() - startTime,
          gate: gateVerdict,
        })
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

    // 若子智能体已处于停止/完成状态：**走正式的恢复路径**（不是"塞进队列再 drain"）。
    //
    // 为什么要改：原先这里是 `queue.push + drain`，而 `drain → turn` 用的是主会话工具表与
    // `kind: 'main'` 钩子——也就是说这条路上既不过门禁，也不应用 profile 白名单
    // （只读子智能体在这条路上能拿到写工具，且没有任何提示）。
    // 现在复用 `resumeSubagentThread`：它自带门禁（判定输入 `resumeGateTask`）、
    // profile 解析、子智能体工具表与 `kind: 'subagent'` 钩子，并且会通知父会话。
    await this.resumeSubagentThread({
      subagentThreadId: thread.id,
      instruction: options.message,
    })

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

    // 启动门禁：与 startSubagentThread 走**同一份实现**（设计文档 §6.3）。恢复同样会
    // 产生新的一轮执行与开销，所以按设计也要过门禁——判定输入见 resumeGateTask。
    // 位置在**写任何东西之前**：被拦下时会话原样不动（不会多出一条"恢复指示"消息，
    // 也不会进 runningThreadIds），调用方拿到的是那句错误（`resume_subagent` 会把它
    // 作为工具失败回给模型）。
    const gateOutcome = await this.gateSubagent(
      profile,
      this.resumeGateTask(subagentThread, options.instruction),
      subagentThread.workspace
    )
    const gateVerdict: SubagentEndContext['gate'] = gateOutcome
      ? {
          allowed: gateOutcome.allowed,
          judged: gateOutcome.judged,
          reason: gateOutcome.reason,
          calibrated: gateOutcome.calibrated,
        }
      : undefined

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

    // 与 startSubagentThread 共用同一份解析（见 subagents/access.ts）
    let subagentTools = resolveSubagentTools(profile, workspace)
    // 门禁给的 tools 已经按授权集合裁过（access.ts），这里直接用——与 start 路径一致
    if (gateOutcome?.tools && gateOutcome.tools.length > 0) {
      subagentTools = gateOutcome.tools
      this.trace(`[子智能体门禁] 「${profile.name}」的工具表按门禁结论收窄为 ${subagentTools.length} 个`)
    }
    // notify_parent 绕过 profile 白名单，理由见 startSubagentThread
    subagentTools.push(createNotifyParentTool(subagentThread.id))

    const steeringQueue: AgentMessage[] = []
    this.steeringQueues.set(subagentThread.id, steeringQueue)

    const resultPromise = (async (): Promise<SubagentRunResult> => {
      const startTime = Date.now()
      const maxSteps = profile.maxSteps
      let stepsExecuted = 0
      let toolCallsCount = 0
      let lastAssistantMessage: any = null
      let latestSummary = ''
      /** 结束时交给父智能体的结论（唤醒兜底用），见 finally。 */
      let wakeStatus: 'done' | 'error' = 'done'
      let wakeSummary = ''

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
        // 离线兜底从 return 出去走不到 finally：收尾（含唤醒兜底）在这里补上
        await this.finishSubagent({
          thread: subagentThread,
          profile,
          parentThread,
          wake: options.onStepUpdate ? { summary: fallbackText, status: 'done' } : undefined,
          stepsExecuted: 1,
          durationMs: Date.now() - startTime,
          gate: gateVerdict,
        })
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
        // 子智能体循环同样派发钩子（设计文档 §5.5）：kind/subagentId 让插件能分辨
        // "这次是主会话还是某个子智能体的回合"
        const hooks = await this.composeHooks({
          workspace: subagentThread.workspace,
          kind: 'subagent',
          threadId: subagentThread.id,
          subagentId: profile.id,
        })
        const loop = runAgentLoop(subagentThread.messages, config, {
          systemPrompt: profile.systemPrompt,
          tools: subagentTools,
          maxSteps,
          effort: profile.modelOverride?.effort ?? 'high',
          toolExecution: 'sequential',
          signal: controller.signal,
          hooks,
          hookContext: { kind: 'subagent', threadId: subagentThread.id, subagentId: profile.id },
          onNotice: (message) => this.trace(message),
          getSteeringMessages: async () => {
            if (steeringQueue.length === 0) return []
            return steeringQueue.splice(0, steeringQueue.length)
          },
          beforeToolCall: async (context) => {
            if (profile.mode === 'readonly' && defaultToolRegistry.isWriteTool(context.toolCall.name)) {
              return { block: true, reason: `子智能体 ${profile.name} 运行在只读安全模式下，禁止执行写操作。` }
            }
            // 子智能体同样在工作区里写文件，改动一样要留回滚的退路
            await this.captureCheckpoint(subagentThread, context.toolCall)
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
        wakeStatus = isOk ? 'done' : 'error'
        wakeSummary = `${resultText}${outputFile ? `\n\n完整报告：${outputFile}` : ''}`

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
        wakeStatus = 'error'
        wakeSummary = `执行异常中断：${errorMessage}`
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
        // 自动唤醒：恢复执行结束（含失败）时把结论推给正等待的父智能体
        await this.finishSubagent({
          thread: subagentThread,
          profile,
          parentThread,
          wake:
            options.onStepUpdate && wakeSummary
              ? { summary: wakeSummary, status: wakeStatus }
              : undefined,
          stepsExecuted,
          durationMs: Date.now() - startTime,
          gate: gateVerdict,
        })
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
    let selection = selectCompactSelection(thread.messages, thread.items)
    const beforeCompaction = {
      messages: thread.messages.length,
      items: thread.items.length,
    }

    // 插件可以在压缩前**追加必须保留的消息**，或整体替换选择方案（受
    // allowCompactionReplace 约束，见 hook-runtime）。没历史可压时不打扰插件。
    let compactionHooks: AgentHooks = {}
    if (selection.messagesToSummarize.length > 0) {
      compactionHooks = await this.composeHooks({
        workspace: thread.workspace,
        kind: 'main',
        threadId: thread.id,
        mode: thread.mode ?? this.mode ?? 'code',
      })
      if (compactionHooks.beforeCompaction) {
        const verdict = await compactionHooks.beforeCompaction({
          kind: 'main',
          workspace: thread.workspace,
          threadId: thread.id,
          trigger: options.trigger === 'auto' ? 'auto' : 'manual',
          selection,
          messageCount: thread.messages.length,
          itemCount: thread.items.length,
          trace: (message) => this.trace(message),
        })
        if (verdict) {
          selection = applyCompactionVerdict(selection, verdict)
          if (verdict.keepMessages?.length) {
            this.trace(
              `[插件] 压缩前追加保留 ${verdict.keepMessages.length} 条消息（否则它们会被总结掉）`
            )
          }
          if (verdict.selection) {
            this.trace(`[插件] 压缩选择方案被「${verdict.by ?? '未知插件'}」整体替换`)
          }
        }
      }
    }

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

    const compactionStartedAt = Date.now()

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
      await this.runAfterCompactionHooks(compactionHooks, {
        thread,
        trigger: options.trigger === 'auto' ? 'auto' : 'manual',
        before: beforeCompaction,
        after: { messages: thread.messages.length, items: thread.items.length },
        turnsSummarized: result.turnsSummarized,
        savedTokens: result.savedTokens,
        durationMs: Date.now() - compactionStartedAt,
        success: true,
      })
      this.notify()
      return { success: true }
    } catch (error) {
      // 移除临时 notice 并提示错误
      thread.items = thread.items.filter((it) => it.id !== noticeId)
      const errText = `上下文压缩失败：${(error as Error).message}`
      this.fail(thread, errText)
      await this.runAfterCompactionHooks(compactionHooks, {
        thread,
        trigger: options.trigger === 'auto' ? 'auto' : 'manual',
        before: beforeCompaction,
        after: { messages: thread.messages.length, items: thread.items.length },
        turnsSummarized: 0,
        savedTokens: 0,
        durationMs: Date.now() - compactionStartedAt,
        success: false,
      })
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
    // 落盘前的脱敏挂在异步链路里：`persist` 本身仍是"发起即返回"，调用方不受影响
    void this.persistWithHooks(threadId, message, workspace)
  }

  /**
   * 落盘（可被 `beforePersist` 改写正文）。
   *
   * 与 `beforeLlmRequest` 的分工：那个管"发给模型的"，这个管"写进磁盘的"——两者
   * 可以不一致（发给模型的要完整，落盘的去敏感片段）。
   */
  private async persistWithHooks(
    threadId: string,
    message: AgentMessage,
    workspace?: string
  ): Promise<void> {
    try {
      const hooks = await this.composeHooks({ workspace: workspace ?? this.project, kind: 'main', threadId })
      if (hooks.beforePersist) {
        const verdict = await hooks.beforePersist({
          kind: 'main',
          workspace,
          threadId,
          message,
          trace: (line) => this.trace(line),
        })
        if (verdict?.content !== undefined) {
          message = { ...message, content: verdict.content } as AgentMessage
        }
      }
    } catch (error) {
      this.trace(`[插件] beforePersist 抛错，按原文落盘：${(error as Error).message}`)
    }
    await defaultSessionManager.appendMessage(threadId, message, workspace).catch(() => {})
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
        // 关键：若该排队项尚未加入 thread.items（即在会话运行期间被排队进来的消息），
        // 在真正开始执行该轮时才将其作为用户消息挂入会话流，保证会话时序严谨，不提前堆叠
        if (!next.item || !thread.items.some((it) => it.id === next.item?.id)) {
          const userItem: Item = next.item ?? {
            kind: 'user',
            id: nextId('item'),
            at: Date.now(),
            text: next.text,
            images: next.images && next.images.length > 0 ? [...next.images] : undefined,
          }
          next.item = userItem
          thread.items.push(userItem)
          this.notify()
        }
        if (next.item?.kind === 'user' && next.item.queued) {
          delete next.item.queued
          this.notify()
        }
        try {
          await this.turn(next.thread, next.text, next.images)
        } catch (turnErr) {
          if ((turnErr as Error)?.name === 'AbortError') {
            // 被立即发送或中断时，若队列中仍有被提前的消息，则继续处理！
            continue
          }
          throw turnErr
        }
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
    // 第二道防线（第一道在 `send`）：**子智能体会话绝不能在这里跑**。
    //
    // 为什么是"拒绝"而不是"改成按子智能体身份跑"：拍板结论是子智能体标签页不接受直接输入，
    // 所以跑到这里就说明有人在往子智能体会话里塞消息（排队项、命令通道、或将来某条新路径）。
    // 此时用主会话工具表继续跑，等于让只读/白名单子智能体拿到写工具——静默越权；
    // 而"就地改造成子智能体身份"会把门禁、profile 解析、父会话唤醒这一整套在这里再实现一遍。
    // 子智能体的执行路径只有两条（`startSubagentThread` / `resumeSubagentThread`），
    // 它们都带着门禁与 profile；这里如实拒绝，并把该走的路指出来。
    if (thread.isSubagent) {
      this.push({
        kind: 'info',
        text:
          `已阻止在子智能体会话「${thread.title}」上以主会话身份执行。` +
          `子智能体的执行必须走 start_subagent / resume_subagent（带门禁与 profile 白名单）。`,
      })
      this.trace(`[子智能体] turn() 拒绝以主会话身份执行子智能体会话（threadId=${thread.id}）`)
      this.notify()
      return
    }

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

    let systemPrompt = await defaultPromptManager.getCompositeSystemPrompt(thread.workspace, currentMode)
    thread.lastSystemPrompt = systemPrompt
    thread.lastSystemPromptChars = systemPrompt.length
    thread.lastToolSpecsChars = JSON.stringify(tools).length

    // 插件钩子：能收窄本轮工具表、注入消息、替换/追加系统提示词、收尾时再补一段
    const hooks = await this.composeHooks({
      workspace: thread.workspace,
      kind: 'main',
      threadId: thread.id,
      mode: currentMode,
    })

    // 系统提示词组装完成、即将使用（单向点位）：插件看到的是**最终**文本
    if (hooks.beforeSystemPrompt) {
      try {
        const verdict = await hooks.beforeSystemPrompt({
          kind: 'main',
          workspace: thread.workspace,
          threadId: thread.id,
          systemPrompt,
          mode: currentMode,
          trace: (message) => this.trace(message),
        })
        if (verdict?.replace !== undefined) systemPrompt = verdict.replace
        if (verdict?.append) {
          systemPrompt = systemPrompt ? `${systemPrompt}\n\n${verdict.append}` : verdict.append
        }
        if (verdict) {
          thread.lastSystemPrompt = systemPrompt
          thread.lastSystemPromptChars = systemPrompt.length
        }
      } catch (error) {
        this.trace(`[插件] beforeSystemPrompt 抛错，已忽略：${(error as Error).message}`)
      }
    }

    const loop = runAgentLoop(thread.messages, config, {
      tools,
      systemPrompt,
      workspace: thread.workspace,
      hooks,
      hookContext: { kind: 'main', threadId: thread.id },
      onNotice: (message) => this.trace(message),
      effort: EFFORT_VALUE[this.effort],
      // 顺序执行：审批一次只该问一件事，命令之间也不该互相抢工作目录。
      // 例外见 agent-loop：整批调用都显式声明 parallel（如并发委派多个只读子智能体）
      // 时仍会重叠执行——那类调用既不弹审批也不抢目录。
      toolExecution: 'sequential',
      signal: controller.signal,
      beforeToolCall: (context: BeforeToolCallContext) =>
        this.gate(thread, context.toolCall, controller.signal),
      // 用户钩子（hooks.json）的 after_tool 事件：工具执行完代跑自动格式化之类的命令
      afterToolCall: async (context) => {
        try {
          const result = await defaultHooks.run(thread.workspace, 'after_tool', context.toolCall.name, {
            args: context.toolCall.arguments,
            ok: !context.isError,
            output: context.result.output,
          })
          for (const run of result.runs) {
            this.push({ kind: 'tool', text: `钩子 after_tool · ${context.toolCall.name} · 退出码 ${run.exitCode ?? 'err'}` })
          }
        } catch {
          // 钩子失败不影响工具结果
        }
        return undefined
      },
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
            // 用户钩子：一轮结束（agent_end 事件）
            void defaultHooks
              .run(thread.workspace, 'agent_end', null, { reason: event.reason, thread_id: thread.id })
              .catch(() => {})
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

  /**
   * 用户钩子（hooks.json）的 before_tool 事件：工具获准后、执行前代跑守门命令。
   * 退出码非零 = 拦截，stderr（或默认文案）作为拒绝理由回给模型。
   */
  private async runBeforeToolHook(thread: Thread, call: ToolCallBlock): Promise<string | null> {
    try {
      const result = await defaultHooks.run(thread.workspace, 'before_tool', call.name, {
        args: call.arguments,
        thread_id: thread.id,
        workspace: thread.workspace,
      })
      for (const run of result.runs) {
        this.push({ kind: 'tool', text: `钩子 before_tool · ${call.name} · 退出码 ${run.exitCode ?? 'err'}` })
      }
      if (result.blocked) {
        this.push({ kind: 'info', text: `钩子拦截了 ${call.name}：${result.reason ?? ''}` })
        return result.reason ?? `用户配置的钩子拦截了这次 ${call.name} 调用。`
      }
    } catch {
      // 钩子体系本身出错不阻塞工具执行
    }
    return null
  }

  private needsApproval(name: string): boolean {
    if (this.approval === 'ask') return true
    if (this.approval === 'readonly') return isWriteTool(name)
    return false
  }

  /**
   * 写工具执行前抓检查点：把目标文件当下的内容快照一份并挂到卡片上。
   * 主循环与子智能体循环都走这里。快照失败不拦截执行——安全网失效时
   * 写操作本身照常工作（沙箱拒绝之类的错误让工具自己报）。
   */
  private async captureCheckpoint(thread: Thread, call: ToolCallBlock): Promise<void> {
    if (!CHECKPOINT_TOOLS.has(call.name)) return
    const card = this.cards.get(call.id)
    if (card?.checkpointId) return
    try {
      const relatives = checkpointPathsOf(call.name, call.arguments)
      if (relatives.length === 0) return
      const targets: Array<{ path: string; absolute: string }> = []
      for (const relative of relatives) {
        try {
          targets.push({
            path: relative.replace(/\\/g, '/'),
            absolute: checkWorkspaceSandbox(thread.workspace, relative),
          })
        } catch {
          // 单个路径越界不该拖累同批其它文件的快照；越界的那个由工具自己报错
        }
      }
      if (targets.length === 0) return
      const record = await defaultCheckpointManager.capture(thread.id, call.id, targets)
      if (card) card.checkpointId = record.id
      // 检查点已建立（单向观察点）：录了哪些文件、id 是多少
      const hooks = await this.composeHooks({
        workspace: thread.workspace,
        kind: 'main',
        threadId: thread.id,
      })
      if (hooks.afterCheckpoint) {
        await hooks.afterCheckpoint({
          kind: 'main',
          workspace: thread.workspace,
          threadId: thread.id,
          checkpointId: record.id,
          paths: targets.map((target) => target.path),
          trace: (message) => this.trace(message),
        })
      }
    } catch {
      // 快照失败不阻塞工具执行
    }
  }

  /**
   * 压缩已发生，跑一遍 `afterCompaction`（纯观察：前后规模、省下的 token 与耗时）。
   *
   * 前后规模是刻意给出来的：插件替换选择方案过激时，压缩会"白做"——用户看不出
   * 原因，但"压缩前后差不多长"这件事本身能提示他去查是哪个插件干的（日志里有）。
   */
  private async runAfterCompactionHooks(
    hooks: AgentHooks,
    outcome: {
      thread: Thread
      trigger: 'manual' | 'auto'
      before: { messages: number; items: number }
      after: { messages: number; items: number }
      turnsSummarized: number
      savedTokens: number
      durationMs: number
      success: boolean
    }
  ): Promise<void> {
    if (!hooks.afterCompaction) return
    try {
      await hooks.afterCompaction({
        kind: 'main',
        workspace: outcome.thread.workspace,
        threadId: outcome.thread.id,
        trigger: outcome.trigger,
        before: outcome.before,
        after: outcome.after,
        turnsSummarized: outcome.turnsSummarized,
        savedTokens: outcome.savedTokens,
        durationMs: outcome.durationMs,
        success: outcome.success,
        trace: (message) => this.trace(message),
      })
    } catch (error) {
      this.trace(`[插件] afterCompaction 抛错，已忽略：${(error as Error).message}`)
    }
  }

  /**
   * 审批决策已定，跑一遍 `afterApproval`（纯观察：决策与耗时）。
   *
   * 只在闸门真的走过审批时调用——"自动批准"的工具没有审批这回事，把每个工具调用都
   * 报一遍只会把信号淹没。
   */
  private async runAfterApprovalHooks(
    hooks: AgentHooks,
    outcome: {
      thread: Thread
      call: ToolCallBlock
      approved: boolean
      decidedBy: 'user' | 'plugin' | 'aborted'
      pluginId?: string
      reason?: string
      durationMs: number
    }
  ): Promise<void> {
    if (!hooks.afterApproval) return
    try {
      await hooks.afterApproval({
        kind: 'main',
        workspace: outcome.thread.workspace,
        threadId: outcome.thread.id,
        toolCall: outcome.call,
        approved: outcome.approved,
        decidedBy: outcome.decidedBy,
        pluginId: outcome.pluginId,
        reason: outcome.reason,
        durationMs: outcome.durationMs,
        trace: (message) => this.trace(message),
      })
    } catch (error) {
      // 事后钩子不改写既成事实，抛错只记一行
      this.trace(`[插件] afterApproval 抛错，已忽略：${(error as Error).message}`)
    }
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

    // 插件的前置判定：只在闸门本来要问用户时才跑（不问就没有"免问"可言）。
    // 必须在建卡片之前跑：插件放行时卡片不该显示成"等你点"。
    const asksUser = this.needsApproval(call.name)
    const approvalStartedAt = Date.now()
    const approvalHooks = asksUser
      ? await this.composeHooks({ workspace: thread.workspace, kind: 'main', threadId: thread.id, mode: currentMode })
      : {}

    /**
     * 向用户提问并等他的回答——**受控能力，由核心实现**（设计文档 §6.6.4）。
     *
     * 插件只决定"问什么"，弹卡片、等点击、中止处理、写历史全在这里。
     * 关键安全属性：本方法**只能转发真实点击**，没有任何"直接批准"的旁路——
     * 插件想放行只能走 `decision: 'allow'`，那会写进调试日志且受审批档位约束。
     *
     * 不设超时：用户没答就是没答，中止由 signal 负责（与内核自身等审批一致）。
     */
    const askUser: BeforeApprovalContext['askUser'] = async (request) => {
      const target = request.toolCall ?? call
      const answer = await this.waitForUserApproval(thread, target, signal)
      if (request.reason) {
        this.trace(`[插件] ${call.name} 的审批提问：${request.reason}`)
      }
      // `choice` 只在插件给了 options 时才有意义；默认的是/否由 approved 表达
      const wanted = answer.approved ? 'approve' : 'deny'
      const choice = request.options?.find((option) => option.id === wanted)?.id
      return { approved: answer.approved, choice, answeredBy: answer.answeredBy }
    }

    let pluginApproval: { decision: 'allow' | 'deny'; reason?: string; pluginId?: string } | undefined
    if (asksUser && approvalHooks.beforeApproval) {
      const verdict = await approvalHooks.beforeApproval({
        kind: 'main',
        workspace: thread.workspace,
        threadId: thread.id,
        toolCall: call,
        approvalMode: this.approval === 'readonly' ? 'readonly' : 'ask',
        isWrite: defaultToolRegistry.isWriteTool(call.name),
        trace: (message) => this.trace(message),
        askUser,
      })
      if (verdict?.decision) {
        if (verdict.decision === 'allow' && this.approval === 'readonly') {
          // 只读档位的语义就是"写操作必须经我确认"，插件不该替用户取消它。
          // 不静默：说出来，然后照常问用户。
          this.trace(
            `[插件] ${verdict.decidedBy ?? '(未知插件)'} 想自动批准 ${call.name}，但当前是只读审批档位，已忽略`
          )
        } else {
          pluginApproval = {
            decision: verdict.decision,
            reason: verdict.reason,
            pluginId: verdict.decidedBy,
          }
        }
      }
    }

    // 插件可能已经通过 askUser 造过卡片（它跑在这之前）：那说明用户已经被问过且答了，
    // 复用同一张，不要再造第二张——否则界面上会出现两个同一次调用的卡片。
    const existing = this.cards.get(call.id)
    const card: ToolCard = existing ?? {
      kind: 'tool',
      id: nextId('item'),
      at: Date.now(),
      callId: call.id,
      name: call.name,
      args: call.arguments,
      rawArgs: call.rawArguments,
      status: 'running',
      threadId: thread.id,
    }
    if (!existing) {
      thread.items.push(card)
      this.cards.set(call.id, card)
    }
    // 状态在插件判定之后才定稿：放行时卡片不该显示成"等你点"
    card.status = asksUser && !pluginApproval ? 'awaiting' : 'running'
    this.notify()

    if (pluginApproval) {
      await this.runAfterApprovalHooks(approvalHooks, {
        thread,
        call,
        approved: pluginApproval.decision === 'allow',
        decidedBy: 'plugin',
        pluginId: pluginApproval.pluginId,
        reason: pluginApproval.reason,
        durationMs: Date.now() - approvalStartedAt,
      })
    }

    if (pluginApproval?.decision === 'deny') {
      const reason = pluginApproval.reason ?? '被插件策略拒绝'
      card.status = 'denied'
      card.output = reason
      this.cards.delete(call.id)
      this.push({
        kind: 'tool',
        text: `${call.name} 被插件「${pluginApproval.pluginId ?? '未知'}」拒绝：${reason}`,
      })
      this.persist(thread.id, {
        role: 'toolResult',
        toolCallId: call.id,
        toolName: call.name,
        // 理由要回给模型（设计文档 §10：不能变成"工具执行失败"那种无从纠正的错误）
        content: reason,
        isError: true,
        timestamp: Date.now(),
      })
      this.notify()
      return { block: true, reason }
    }

    if (card.status !== 'awaiting') {
      await this.captureCheckpoint(thread, call)
      const hookBlock = await this.runBeforeToolHook(thread, call)
      if (hookBlock) {
        card.status = 'denied'
        card.output = hookBlock
        this.notify()
        return { block: true, reason: hookBlock }
      }
      return undefined
    }

    const { approved } = await this.waitForUserApproval(thread, call, signal)
    await this.runAfterApprovalHooks(approvalHooks, {
      thread,
      call,
      approved,
      decidedBy: signal?.aborted ? 'aborted' : 'user',
      durationMs: Date.now() - approvalStartedAt,
    })

    if (approved) {
      await this.captureCheckpoint(thread, call)
      const hookBlock = await this.runBeforeToolHook(thread, call)
      if (hookBlock) {
        card.status = 'denied'
        card.output = hookBlock
        this.notify()
        return { block: true, reason: hookBlock }
      }
      return undefined
    }

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

  /**
   * 等用户就一次工具调用给出"批准 / 拒绝"。
   *
   * 抽出来是因为有两条路径要等：内核自己的审批闸门（`gate`），以及插件通过
   * `ctx.askUser` 发起的提问（设计文档 §6.6.4）。两处**必须共用同一份实现**——
   * 否则"中止时算不算拒绝""卡片 id 用哪个"这类细节会在两边漂移，而这类漂移
   * 表现为偶发的挂起或误判，极难查。
   *
   * **没有卡片就现造一张**，这是关键：插件的 `askUser` 跑在 `gate` 建卡片之前，
   * 若什么都不做，用户界面里根本没有可点的东西，等待会永久挂起。造出来的卡片
   * 由 `gate` 后续复用（它按 `call.id` 查 `this.cards`）。
   *
   * 停机语义：`signal` 中止时立即按"未批准"收尾，且**摘掉等待句柄**，
   * 否则一个已经没人会点的卡片会永远占着 `approvals`。
   */
  private async waitForUserApproval(
    thread: Thread,
    call: ToolCallBlock,
    signal?: AbortSignal
  ): Promise<{ approved: boolean; answeredBy: 'user' | 'aborted' }> {
    let card = this.cards.get(call.id)
    if (!card) {
      card = {
        kind: 'tool',
        id: nextId('item'),
        at: Date.now(),
        callId: call.id,
        name: call.name,
        args: call.arguments,
        rawArgs: call.rawArguments,
        status: 'awaiting',
        threadId: thread.id,
      }
      thread.items.push(card)
      this.cards.set(call.id, card)
      this.notify()
    }
    const waiterId = card.id

    return new Promise((resolve) => {
      const finish = (approved: boolean, answeredBy: 'user' | 'aborted') => {
        this.approvals.delete(waiterId)
        resolve({ approved, answeredBy })
      }
      this.approvals.set(waiterId, (approved) => finish(approved, 'user'))
      if (signal?.aborted) {
        finish(false, 'aborted')
      } else if (signal) {
        signal.addEventListener('abort', () => finish(false, 'aborted'), { once: true })
      }
    })
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
      // 检查点 id 跟着流水走：重启恢复之后「撤销此次改动」仍然可用
      checkpointId: card?.checkpointId,
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
