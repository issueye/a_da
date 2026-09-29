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
import type { ChatCompletionMessageParam } from '../ai/stream'
import type { AgentMode } from '../types'
import type { TodoStep } from '../tools/builtins/todo'
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
  /**
   * 向用户提问并等他的回答——**受控能力，由核心实现**（设计文档 §6.6.4）。
   *
   * 为什么要给这个能力：审批策略本身就是"要不要问、问什么"的判断。插件要实现自定义
   * 策略（白名单免问、危险命令二次确认…）就必须能发起询问。但**弹卡片、等点击、超时、
   * 中止、写历史这些执行细节必须留在核心**——让插件自己实现等待，等于把"用户点了什么"
   * 的解释权交给第三方，也无从保证中止与超时不会漏。
   *
   * 契约上的三条保证：
   * - 插件拿到的是**用户的答案**，不是"插件希望用户答什么"；
   * - 插件**无法伪造**回答：本方法只能转发真实点击，核心不提供任何"直接批准"的接口；
   * - 用户取消/中止时返回 `approved: false`，与"明确拒绝"在结果上一致，
   *   但 `answeredBy` 会区分（`'user'` / `'aborted'`）。
   *
   * 未注入时（例如子智能体循环、或没有可等待的用户界面）该字段缺席，
   * 插件应当返回 `undefined` 让核心照常问，而不是自己猜一个答案。
   */
  askUser?: (request: AskUserRequest) => Promise<AskUserAnswer>
}

/** `askUser` 的提问内容。刻意只有这几个字段：插件能决定"问什么"，不能决定"怎么弹"。 */
export interface AskUserRequest {
  /** 要用户确认的工具调用（默认是当前这次；允许插件换成别的以支持批量确认） */
  toolCall?: ToolCallBlock
  /** 给用户看的理由（为什么这次要问） */
  reason?: string
  /**
   * 可选的固定选项。不给则用默认的"批准 / 拒绝"。
   *
   * 注意：**不能用它实现"批准一次 / 永久批准"这类记忆**——那是插件自己的状态，
   * 核心只负责把用户选中的项原样送回来。
   */
  options?: Array<{ id: string; label: string }>
}

export interface AskUserAnswer {
  /** 用户是否批准。中止、超时、关掉弹窗都算 false */
  approved: boolean
  /** 用户选中的选项 id（传了 `options` 时） */
  choice?: string
  /** 用户填写的理由（界面支持时） */
  reason?: string
  /** 由核心填写：谁答的 */
  answeredBy: 'user' | 'aborted'
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

// ─────────────────────────────── 任务清单（todo 工具）

/**
 * 任务清单即将被写入（模型刚给出新的清单）。
 *
 * 为什么这个点位值得成对：清单是**有状态延续**的东西——它留在工具卡片里、被界面读出来
 * 显示成"当前进度"，也被模型在后续轮次里当作计划引用。插件在这里可以补上模型漏掉的
 * 验收项、拆掉过碎的步骤，也可以直接拦下（例如"没有验收标准之前不许改计划"）。
 *
 * 清单本身不是授权、不参与审批，所以替换不需要额外开关；但拦下会改变模型接下来的行为，
 * 理由会回给模型（作为工具结果，而不是"执行失败"那种无从纠正的错误）。
 */
export interface BeforeTodoUpdateContext extends AgentHookContextBase {
  /** 这次要写入的清单（模型给的） */
  todos: TodoStep[]
  /** 当前生效的清单（上一次调用留下的），首次为空数组 */
  previous: TodoStep[]
}

export interface BeforeTodoUpdateResult {
  /** 替换要写入的清单 */
  todos?: TodoStep[]
  /** 拦下这次更新（理由回给模型） */
  block?: boolean
  blockReason?: string
  /** 由钩子运行层填写 */
  by?: string
}

/**
 * 清单已写入（**回执**：实际生效的是哪一份，以及相对上次变了什么）。
 *
 * 与 `beforeTodoUpdate` 成对的理由和轮次钩子一样：插件得知道自己的修改是否真的生效，
 * 也能在这里发现"模型在偷偷把已完成的项改回 pending"这类事。
 */
export interface AfterTodoUpdateContext extends AgentHookContextBase {
  /** 实际生效的清单 */
  todos: TodoStep[]
  /** 本次调用之前的清单 */
  previous: TodoStep[]
  /** 有变化的项数（新增 / 删除 / 改了标题或状态） */
  changed: number
  /** 已经完成、但这次被改回未完成（或反之）的项标题 */
  reopened: string[]
}

export interface AfterTodoUpdateResult {
  /** 追加到工具结果里的旁注（模型看得到，例如"第 3 项没有验收标准"） */
  appendNote?: string
}

// ─────────────────────────────── 第二优先点位（暴露但需谨慎，§6.6.2）

/**
 * 模型请求前：**最后一刻**的裁剪与脱敏。
 *
 * 比 `beforeTurn` 更晚：那里改的是"这一轮用哪些工具、注入什么"，这里拿到的是**即将
 * 发出去的完整消息数组**，适合做"别把这段发出去"这类判断。
 */
export interface BeforeLlmRequestContext extends AgentHookContextBase {
  model: string
  /** 即将下发的完整消息（含系统提示词） */
  messages: ChatCompletionMessageParam[]
  toolNames: string[]
  step: number
}

export interface BeforeLlmRequestResult {
  /** 替换要下发的消息。**系统提示词会被核心放回最前面**，插件不必自己保证顺序 */
  messages?: ChatCompletionMessageParam[]
}

export interface AfterLlmResponseContext extends AgentHookContextBase {
  model: string
  message: AssistantMessage
  step: number
  /** 本次调用耗时（模型侧），不含钩子自身开销 */
  durationMs: number
}

export type AfterLlmResponseResult = void

/**
 * 系统提示词组装完成、即将被这一轮使用（**单向**：组装即终态，没有可配对的 after）。
 *
 * 实现在 store 里、紧跟 `getCompositeSystemPrompt()` 之后：插件看到的是**最终**文本
 * （含项目说明与模式规范），这对"脱敏"和"补一句"都是最有用的时机。
 */
export interface BeforeSystemPromptContext extends AgentHookContextBase {
  systemPrompt: string
  mode: AgentMode
}

export interface BeforeSystemPromptResult {
  /** 追加在末尾 */
  append?: string
  /** 整体替换，受 `allowSystemPromptReplace` 约束（默认开） */
  replace?: string
}

/**
 * 技能加载前：能看到要加载哪个技能，可以拦下、也可以替换正文（脱敏/裁剪）。
 *
 * 技能正文是**给模型看的建议文本**，不是权限授予，所以替换正文不需要额外开关；
 * 但拦下（block）会改变模型能看到什么，理由会回给模型。
 */
export interface BeforeSkillLoadContext extends AgentHookContextBase {
  skillName: string
}

export interface BeforeSkillLoadResult {
  block?: boolean
  blockReason?: string
  /** 替换要注入的正文 */
  content?: string
}

export interface AfterSkillLoadContext extends AgentHookContextBase {
  skillName: string
  /** 是否真的加载到了 */
  loaded: boolean
  /** 注入正文的长度（0 表示没加载到） */
  chars: number
}

export type AfterSkillLoadResult = void

/**
 * 落盘前（**单向**）：最后一道脱敏关。
 *
 * 与 `beforeLlmRequest` 的区别：那个管"发给模型的"，这个管"写进磁盘的"——两者可以
 * 不一致（例如发给模型的要完整，落盘的要去掉敏感片段）。
 */
export interface BeforePersistContext extends AgentHookContextBase {
  /** 即将写入会话文件的消息 */
  message: AgentMessage
}

export interface BeforePersistResult {
  /** 替换落盘的正文（jsonl 里存替换后的内容） */
  content?: string
}

/**
 * 检查点已建立（**单向**：它本身就是 `after*`，没有配对的 before——检查点的"事前"
 * 是工具执行，那已经由 `beforeToolCall` 覆盖）。
 */
export interface AfterCheckpointContext extends AgentHookContextBase {
  /** 检查点 id；跳过快照时为 undefined */
  checkpointId?: string
  /** 本次纳入快照的工作区相对路径 */
  paths: string[]
}

export type AfterCheckpointResult = void

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
  beforeLlmRequest?: (ctx: BeforeLlmRequestContext) => Promise<BeforeLlmRequestResult | undefined>
  afterLlmResponse?: (ctx: AfterLlmResponseContext) => Promise<AfterLlmResponseResult>
  beforeSystemPrompt?: (ctx: BeforeSystemPromptContext) => Promise<BeforeSystemPromptResult | undefined>
  beforeSkillLoad?: (ctx: BeforeSkillLoadContext) => Promise<BeforeSkillLoadResult | undefined>
  afterSkillLoad?: (ctx: AfterSkillLoadContext) => Promise<AfterSkillLoadResult>
  beforePersist?: (ctx: BeforePersistContext) => Promise<BeforePersistResult | undefined>
  afterCheckpoint?: (ctx: AfterCheckpointContext) => Promise<AfterCheckpointResult>
  beforeTodoUpdate?: (ctx: BeforeTodoUpdateContext) => Promise<BeforeTodoUpdateResult | undefined>
  afterTodoUpdate?: (ctx: AfterTodoUpdateContext) => Promise<AfterTodoUpdateResult | undefined>
}

/**
 * 刻意**不成对**的点位：纯判定（`check_gate` 类）与纯通知（`onThreadSwitch`）。
 *
 * 它们没有"后续状态"可观察，强行配对只会加重插件负担——成对原则的准确表述是
 * "有状态延续的点位都应成对"，而不是"一切都必须成对"（§6.0）。
 */
export const UNPAIRED_HOOKS: ReadonlyArray<keyof AgentHooks> = [
  'onThreadSwitch',
  'beforeSystemPrompt',
  'beforePersist',
  'afterCheckpoint',
]

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
  { before: 'beforeLlmRequest', after: 'afterLlmResponse' },
  { before: 'beforeSkillLoad', after: 'afterSkillLoad' },
  { before: 'beforeTodoUpdate', after: 'afterTodoUpdate' },
]
