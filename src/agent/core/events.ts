/**
 * 插件可干预的**决策点**契约（设计文档 §6.1、§6.2、§6.2.1、§6.4）。
 *
 * 与 `core/types.ts` 的 `AgentEvent` 刻意分开：
 * - `AgentEvent` 是**上报给界面的已发生事实**，只读、丢弃返回值；
 * - 这里是**可干预的决策点**，有返回值、能改变控制流，信任要求更高。
 *
 * 两条贯穿全层的原则：
 *
 * 1. **成对**：凡是有状态延续的点位，事前 / 事后必须成对存在。少了 after，插件就
 *    无法知道自己"是不是真的生效了"（§6.0）——这是本层最核心的诚实性保障。
 * 2. **不可扩张**：钩子返回值里的 `tools` **只能收窄**，不能加进新工具。工具集是审批
 *    闸门的依据（`isWriteTool` 是静态名字白名单），能凭返回值塞新名字就等于绕过审批。
 *    这一条不可配置（§6.4.3）。
 */

import type { CompactSelection } from '../compact/types'
import type {
  AgentEndReason,
  AgentMessage,
  AgentTool,
  AssistantMessage,
  BeforeToolCallContext,
  BeforeToolCallResult,
  AfterToolCallContext,
  AfterToolCallResult,
  ToolCallBlock,
  ToolResultMessage,
} from './types'

/** 主智能体循环还是子智能体循环在跑。子循环同样派发钩子（§5.5）。 */
export type AgentHookKind = 'main' | 'subagent'

/** 所有钩子上下文共有的身份信息。 */
export interface AgentHookContextBase {
  kind: AgentHookKind
  /** 会话 id（主循环是主会话，子循环是子会话） */
  threadId?: string
  /** 子智能体的角色 id（仅子循环） */
  subagentId?: string
  workspace?: string
  /**
   * 往调试面板写一行（可选）。
   *
   * 钩子里的决定往往"看不见"——比如按配置收窄了工具表、或发现配置里的名字都不存在
   * 于是什么都没做。插件要能解释自己做了什么，否则用户只能看到"行为诡异"却查不到原因。
   */
  trace?: (message: string) => void
}

// ─────────────────────────────── 整轮：agent_start / agent_end

export interface BeforeAgentStartContext extends AgentHookContextBase {
  messages: AgentMessage[]
  tools: AgentTool[]
  systemPrompt: string
}

export interface BeforeAgentStartResult {
  /** 在系统提示词**之后**追加一段（默认形态，永远安全） */
  appendSystemPrompt?: string
  /**
   * 整体替换系统提示词，受 `allowSystemPromptReplace` 控制（默认开）。
   *
   * 替换时核心仍会在末尾附上 {@link NON_NEGOTIABLE_TOOL_TAIL}——那段只陈述运行时机制
   * （有哪些工具、写操作会过审批、不许声称执行过没做过的事），不是人设，不可协商：
   * 插件换掉系统提示词后，模型仍必须知道这些约束存在（§6.2.1）。
   */
  systemPrompt?: string
  /** 追加初始消息（排在现有消息之后、第一轮之前；不写回会话历史） */
  extraMessages?: AgentMessage[]
  /** 收窄初始工具集（只能收窄，§6.4.3） */
  tools?: AgentTool[]
}

export interface AfterAgentEndContext extends AgentHookContextBase {
  reason: AgentEndReason
  messages: AgentMessage[]
  /** 本轮实际执行了多少步（turn 数） */
  stepsExecuted: number
  /** 从 agent_start 到收尾的耗时（不含 afterAgentEnd 自身，避免污染它要测量的数据） */
  durationMs: number
}

export interface AfterAgentEndResult {
  /**
   * 追加一段文本到本次会话（受 `allowTextRewrite` 控制，默认开）。
   *
   * 实现方式是在 `agent_end` 之前再产生一条助手消息：文本此刻早已流式送达界面，
   * 原地改写已渲染的内容不可能，所以"追加"是本项目唯一诚实的交付方式。
   */
  appendText?: string
}

// ─────────────────────────────── 轮次：turn_start / turn_end

export interface BeforeTurnContext extends AgentHookContextBase {
  step: number
  messages: AgentMessage[]
  /** 本轮**当前**的工具表（还没被任何钩子改过） */
  tools: AgentTool[]
}

export interface BeforeTurnResult {
  /** 本轮工具表覆盖。`undefined` = 沿用；数组 = 设为该数组（只能收窄） */
  tools?: AgentTool[] | 'casual'
  /** 追加消息，本轮请求就生效（不写回会话历史） */
  extraMessages?: AgentMessage[]
  /** 结束整轮：**跑完本轮之后**收尾，不是当前批就断（= 既有 terminateBatch 语义） */
  terminate?: boolean
  terminateReason?: string
  /** 由钩子运行层填写（哪个插件要求的终止），插件不必设置 */
  terminateBy?: string
}

export interface AfterTurnContext extends AgentHookContextBase {
  step: number
  /** 本轮定稿的助手消息 */
  message: AssistantMessage
  toolResults: ToolResultMessage[]
  /**
   * 本轮**实际下发**的工具名——这是 `beforeTurn` 决策的**回执**，不是它的输入。
   *
   * 插件据此判断"我以为的"和"真正生效的"是否一致（可能被收窄规则改写）。缺了这个
   * 字段，"插件以为自己生效了"就无法被发现（§6.2）。
   */
  effectiveToolNames: string[]
  llmDurationMs: number
  toolsDurationMs: number
  /** 事前有插件要求终止时为该插件 id */
  terminatedByHook?: string
}

export interface AfterTurnResult {
  /** 给模型的旁注，下一轮生效（如"你上一轮漏了 X"） */
  appendNote?: string
  /** 结束整轮：跑完本轮之后收尾 */
  terminate?: boolean
  terminateReason?: string
  /** 由钩子运行层填写 */
  terminateBy?: string
}

// ─────────────────────────────── 子智能体（启动判定 + 结束复核）

/**
 * 子智能体启动前的**门禁**（设计文档 §6.3）。
 *
 * 只有当 profile 配了 `gate.criteria` 时才会跑：没有判定标准就没有要判的事。
 * 判定本身由插件提供（决策插件用它的引擎实现），核心只负责**失败方向**与把结论
 * 透传给 `invoke_subagent`——核心不该内置"怎么判断"。
 */
export interface SubagentGateContext extends AgentHookContextBase {
  profileId: string
  profileName: string
  task: string
  /** 用户写的验收标准 */
  criteria: string
  /** 判定阈值（默认由判定方决定） */
  threshold?: number
  /**
   * 用户是否显式要求"拿不到判定时也要拦"。
   *
   * `undefined` = 没表态 → 核心按**放行**处理并提示"门禁未生效"（§6.4.4.5：
   * "用户没表态"不该被核心解读成"要求安全"）。显式 `false` 时，即使判定方给了
   * 一个"通过"，只要它拿不出校准信息，核心也**拦**——这是防止判定方自说自话。
   */
  failOpen?: boolean
}

export interface SubagentGateResult {
  allowed: boolean
  /**
   * 置信度（0-1）。**拿不到真实判断时必须是 `undefined`**，不许编一个看起来合理的数
   * ——与决策插件的契约一致（AGENTS.md §9）。
   */
  confidence?: number
  /** 该置信度是否经过校准；本地自评是多次采样的投票占比，恒为 false */
  calibrated?: boolean
  reason?: string
  /** 收窄该子智能体的工具集（只能收窄，§6.4.3） */
  tools?: AgentTool[]
}

export interface SubagentEndContext extends AgentHookContextBase {
  profileId: string
  /** 子会话 id */
  subagentThreadId: string
  status: 'done' | 'error'
  summary: string
  stepsExecuted: number
  durationMs: number
  /** 本次委派若走了门禁，把结论一并带上，供事后复核"放行的是否真是需要的" */
  gate?: {
    allowed: boolean
    /** 是否真的做了判定（没有判定能力时为 false，即"门禁未生效"） */
    judged: boolean
    reason?: string
    calibrated?: boolean
  }
}

export interface AfterSubagentEndResult {
  /** 给父会话的旁注（父智能体下一轮能看到） */
  appendParentNote?: string
}

// ─────────────────────────────── 上下文压缩（能决定保留什么）

/**
 * 压缩前：插件可以**追加必须保留的消息**（任何配置下都生效——它只会让压缩少做点，
 * 不会多做），也可以整体替换选择方案（受 `allowCompactionReplace` 约束，默认开）。
 *
 * 风险提示：替换过激会让压缩白做，所以 `afterCompaction` 会把前后消息数摆出来，
 * 让用户看得出"某个插件让压缩几乎没生效"。
 */
export interface BeforeCompactionContext extends AgentHookContextBase {
  trigger: 'manual' | 'auto'
  /** 当前的选择方案（插件拿到的是真实对象，可以直接只做微调） */
  selection: CompactSelection
  messageCount: number
  itemCount: number
}

export interface BeforeCompactionResult {
  /**
   * 必须保留的消息。按**对象引用**匹配当前待总结的消息（插件拿到的就是同一批对象）：
   * 命中的会被移出"待总结"、按原顺序并入"保留"。
   */
  keepMessages?: AgentMessage[]
  /** 整体替换选择方案；受 `allowCompactionReplace` 约束，关掉时被忽略并说明 */
  selection?: CompactSelection
  /** 由钩子运行层填写 */
  by?: string
}

export interface AfterCompactionContext extends AgentHookContextBase {
  trigger: 'manual' | 'auto'
  /** 压缩前后的规模：让"压缩几乎没生效"这种情况看得见 */
  before: { messages: number; items: number }
  after: { messages: number; items: number }
  turnsSummarized: number
  savedTokens: number
  durationMs: number
  success: boolean
}

/** 事后钩子是纯观察：压缩已经发生。 */
export type AfterCompactionResult = void

// ─────────────────────────────── 审批闸门（能实现"白名单工具免问"）

/**
 * 审批闸门的前置判定。
 *
 * **只在闸门本来要问用户时才会被调用**（`needsApproval` 为真）：用户选了"自动批准"
 * 的工具本来就不问，也就没有"免问"可言。插件因此能用它实现"白名单工具免问"这类
 * 自动批准策略，而不必去改用户的审批档位。
 */
export interface BeforeApprovalContext extends AgentHookContextBase {
  toolCall: ToolCallBlock
  /** 为什么会问：ask = 用户要求每个工具都确认；readonly = 用户要求写操作都确认 */
  approvalMode: 'ask' | 'readonly'
  /** 这个工具是否会产生写副作用（`isWriteTool` 的静态判定） */
  isWrite: boolean
}

export interface BeforeApprovalResult {
  /**
   * `'allow'` = 免问直接放行；`'deny'` = 直接拒绝（理由会作为工具结果回给模型）；
   * `undefined` = 照常问用户。
   *
   * 两个方向的效力刻意不对称（见 store 的实现）：
   * - `deny` **总是**被采纳——它是收窄，任何插件都不该能推翻另一个插件的否决；
   * - `allow` 在 `readonly` 档位下**被忽略**：那一档的语义就是"写操作必须经我确认"，
   *   插件不该替用户取消它（忽略时会写进调试日志，不静默）。
   */
  decision?: 'allow' | 'deny'
  reason?: string
  /** 由钩子运行层填写（哪个插件做的决定） */
  decidedBy?: string
}

export interface AfterApprovalContext extends AgentHookContextBase {
  toolCall: ToolCallBlock
  /** 决策是谁做的：用户、插件，或调用被中止（等同于拒绝） */
  decidedBy: 'user' | 'plugin' | 'aborted'
  approved: boolean
  reason?: string
  /** 从进入闸门到决策完成（含用户思考时间；插件决策很快） */
  durationMs: number
  /** `decidedBy: 'plugin'` 时是哪个插件 */
  pluginId?: string
}

/** 事后钩子是纯观察：审批已经发生，插件没有可改变的东西。 */
export type AfterApprovalResult = void

// ─────────────────────────────── 会话生命周期

/**
 * 会话将要建立。
 *
 * 插件可以建议标题、也可以往 `Thread.pluginData[自己的 id]` 里塞数据——**核心永不
 * 读取它**（§6.7.3）：一旦核心去解释它，插件数据就变成了隐式契约，插件作者再也没法
 * 自由改自己的结构。它随会话持久化、随会话删除。
 */
export interface BeforeThreadCreateContext extends AgentHookContextBase {
  /** 会话将要在哪个工作区建立 */
  workspace: string
  /** 只有主会话会走这条链路；子会话的标题由角色与任务推导 */
  isSubagent: boolean
}

export interface BeforeThreadCreateResult {
  /** 建议的标题。**空串或纯空白会被忽略**——不产生无名会话 */
  title?: string
  /** 该插件要写进 `Thread.pluginData[id]` 的数据（核心不解释） */
  data?: unknown
  /** 由钩子运行层填写（哪个插件写的） */
  by?: string
}

export interface AfterThreadCreateContext extends AgentHookContextBase {
  /** 已就绪的会话：能读到 id 与 workspace */
  threadId: string
  title: string
  workspace: string
  isSubagent: boolean
}

export type AfterThreadCreateResult = void

export interface BeforeThreadDeleteContext extends AgentHookContextBase {
  threadId: string
  title: string
  workspace: string
  isSubagent: boolean
  /** 因为父会话被删而级联删除（此时每个子会话各调一次，见 §10 的验收） */
  cascaded: boolean
}

export interface BeforeThreadDeleteResult {
  /** 阻止删除。受 `allowThreadDeleteBlock` 约束（默认开），关掉时只能归档 */
  block?: boolean
  blockReason?: string
  /** 删除前先归档一份（默认关：它不阻止删除，只是留个副本） */
  archiveBeforeDelete?: boolean
  /** 由钩子运行层填写 */
  blockedBy?: string
}

export interface AfterThreadDeleteContext extends BeforeThreadDeleteContext {
  /** 是否被插件拦下（拦下时删除并没有发生） */
  blocked: boolean
  archived: boolean
}

export type AfterThreadDeleteResult = void

/**
 * 会话切换（纯通知）。
 *
 * **刻意不成对**：切换是瞬时事件，没有"后续状态"可观察，强行配对只会加重插件负担
 * （§6.0 的边界：有状态延续的点位才需要成对）。
 */
export interface ThreadSwitchContext extends AgentHookContextBase {
  /** 切到的会话 id */
  threadId: string
  workspace: string
  isSubagent: boolean
}

// ─────────────────────────────── 工具调用（既有，M2 开放给插件）

export interface AgentHooks {
  beforeAgentStart?: (ctx: BeforeAgentStartContext) => Promise<BeforeAgentStartResult | undefined>
  afterAgentEnd?: (ctx: AfterAgentEndContext) => Promise<AfterAgentEndResult | undefined>
  beforeTurn?: (ctx: BeforeTurnContext) => Promise<BeforeTurnResult | undefined>
  afterTurn?: (ctx: AfterTurnContext) => Promise<AfterTurnResult | undefined>
  beforeToolCall?: (ctx: BeforeToolCallContext) => Promise<BeforeToolCallResult | undefined>
  afterToolCall?: (ctx: AfterToolCallContext) => Promise<AfterToolCallResult | undefined>
  beforeSubagentStart?: (ctx: SubagentGateContext) => Promise<SubagentGateResult | undefined>
  afterSubagentEnd?: (ctx: SubagentEndContext) => Promise<AfterSubagentEndResult | undefined>
  beforeApproval?: (ctx: BeforeApprovalContext) => Promise<BeforeApprovalResult | undefined>
  afterApproval?: (ctx: AfterApprovalContext) => Promise<AfterApprovalResult>
  beforeCompaction?: (ctx: BeforeCompactionContext) => Promise<BeforeCompactionResult | undefined>
  afterCompaction?: (ctx: AfterCompactionContext) => Promise<AfterCompactionResult>
  beforeThreadCreate?: (ctx: BeforeThreadCreateContext) => Promise<BeforeThreadCreateResult | undefined>
  afterThreadCreate?: (ctx: AfterThreadCreateContext) => Promise<AfterThreadCreateResult>
  beforeThreadDelete?: (ctx: BeforeThreadDeleteContext) => Promise<BeforeThreadDeleteResult | undefined>
  afterThreadDelete?: (ctx: AfterThreadDeleteContext) => Promise<AfterThreadDeleteResult>
  onThreadSwitch?: (ctx: ThreadSwitchContext) => Promise<void>
}

/**
 * 刻意**不成对**的点位：纯判定（`check_gate` 类）与纯通知（`onThreadSwitch`）。
 *
 * 它们没有"后续状态"可观察，强行配对只会加重插件负担——成对原则的准确表述是
 * "有状态延续的点位都应成对"，而不是"一切都必须成对"（§6.0）。
 */
export const UNPAIRED_HOOKS: ReadonlyArray<keyof AgentHooks> = ['onThreadSwitch']

/**
 * 系统提示词被插件替换时，核心仍在末尾附上的不可协商段落。
 *
 * 刻意只写**机制**，不写人设、语气或工作方法——那些正是插件想替换的部分。这四句
 * 陈述的都是运行时事实：工具表就是全部可用工具（未知名字会失败）、写操作会被审批
 * 拦住、检查点会在写之前建立、不许声称执行过没实际调用的操作。
 */
export const NON_NEGOTIABLE_TOOL_TAIL = [
  '【运行时约定（不可协商）】',
  '1. 只能调用工具表里列出的工具；调用表外的名字会直接失败。',
  '2. 写操作与命令执行可能被审批拦截，被拒绝时要如实告知用户，不要换个说法重试。',
  '3. 写入类操作会在执行前建立检查点，用户可回滚；不要删除或绕过检查点。',
  '4. 不要声称执行过没有实际调用的工具，也不要把猜测说成执行结果。',
].join('\n')

/** 成对钩子的配对表：`before*` 与它必须有对应 `after*`（§6.0、§10 的成对性断言）。 */
export const HOOK_PAIRS: ReadonlyArray<{ before: keyof AgentHooks; after: keyof AgentHooks }> = [
  { before: 'beforeAgentStart', after: 'afterAgentEnd' },
  { before: 'beforeTurn', after: 'afterTurn' },
  { before: 'beforeToolCall', after: 'afterToolCall' },
  { before: 'beforeSubagentStart', after: 'afterSubagentEnd' },
  { before: 'beforeApproval', after: 'afterApproval' },
  { before: 'beforeCompaction', after: 'afterCompaction' },
  { before: 'beforeThreadCreate', after: 'afterThreadCreate' },
  { before: 'beforeThreadDelete', after: 'afterThreadDelete' },
]
