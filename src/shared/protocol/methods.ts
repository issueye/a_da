/**
 * 协议方法表：**方法名 / params / result 的唯一来源**。
 *
 * 为什么要一张表而不是散落的常量：`ParamsOf<M>` / `ResultOf<M>` 都由它推导，于是
 * 「客户端调用」与「主机实现」在类型上强制对齐——方法名写错、参数少字段、返回值对不上，
 * 都是编译错误而不是运行期惊喜。
 *
 * 设计依据：`docs/jsonrpc-protocol.md` §3（命令）、§4（通知）、§5（反向请求）。
 * 命名约定见协议 §0.3：`域.动作` = 客户端→服务端命令；`evt.*` = 服务端→客户端通知；
 * `req.*` = 服务端→客户端请求。
 */

import type {
  AgentMode,
  ApprovalMode,
  BuiltinToolInfo,
  ClientSnapshot,
  Effort,
  PluginCapabilities,
  PluginDiagnostic,
  PluginItem,
  PromptItem,
  ProviderConfig,
  ProviderPreset,
  ResolvedPluginCapabilitiesDto,
  SkillSummary,
  SubagentProfile,
} from './dto'

/** 命令（客户端 → 服务端，有响应）。M0 先覆盖 UI 实际用到的那些（协议 §9.1 的 A 组 + 焦点上报）。 */
export interface ProtocolCommands {
  // ── 会话（连接与快照，协议 §1.2/§1.3）──
  /**
   * 握手。令牌同时走 URL（升级时校验一次）与这里——协议 §1.6 要求两道都要有：
   * 升级时拦住"没带令牌的连接"，握手时拦住"令牌在连接后被换掉"的情况。
   */
  'session.hello': {
    params: { token: string; protocolVersion: string; clientName?: string }
    result: { sessionId: string; protocolVersion: string; host: { pid: number } }
  }
  /** 要一份当前快照（连上、重连、以及客户端怀疑自己落后时都走它）。 */
  'session.snapshot': { params: Record<string, never>; result: ClientSnapshot }

  // ── 会话内容 ──
  'thread.create': { params: { workspace: string; mode?: AgentMode }; result: { threadId: string } }
  'thread.delete': { params: { threadId: string }; result: { message: string | null } }
  'thread.send': { params: { threadId: string; text: string; images?: string[] }; result: void }
  'thread.abort': { params: { threadId: string }; result: void }
  'thread.compact': {
    params: { threadId: string; customInstructions?: string; trigger?: 'auto' | 'manual' }
    result: { success: boolean; reason?: string }
  }
  'thread.setMode': { params: { mode: AgentMode; threadId?: string }; result: void }
  'thread.setWorkspace': { params: { threadId: string; workspace: string }; result: void }
  'thread.editAndResend': {
    params: { threadId: string; itemId: string; text: string; images?: string[] }
    result: void
  }

  // ── 排队指令 ──
  'queue.clear': { params: { threadId?: string }; result: void }
  'queue.promote': { params: { threadId?: string; index: number }; result: void }
  'queue.remove': { params: { threadId?: string; index: number }; result: { text: string; images?: string[] } | null }

  // ── 子智能体（运行态） ──
  'subagent.resume': { params: { subagentThreadId: string; instruction?: string }; result: { threadId: string } }

  // ── 审批 / 提问 ──
  'approval.decide': { params: { toolItemId: string; approved: boolean }; result: void }
  'question.answer': { params: { callId: string; choice?: string; text?: string }; result: void }

  // ── 工作区 ──
  'workspace.add': { params: { path: string }; result: { error: string | null } }
  'workspace.remove': { params: { workspace: string }; result: { message: string | null } }
  'workspace.openPublic': { params: { threadId?: string }; result: void }
  'workspace.rescan': { params: Record<string, never>; result: void }
  'workspace.entries': { params: { cursor?: string; limit?: number }; result: string[] }

  // ── 焦点上报（协议 §3.12：不是同步焦点，是让主机知道该为哪个工作区准备上下文） ──
  'ui.activeThread': { params: { threadId: string }; result: void }
  /**
   * 工作区形态的焦点上报（"切到某个项目"）。
   *
   * 协议 §3.12 只写了 `ui.activeThread`；这一条是它的项目级形态——今天 `store.selectProject`
   * 除了切焦点还会触发 `refresh()`（重扫 + 重载插件）与 `onThreadSwitch` 钩子，那两件事在主机侧，
   * 所以必须有通道。M2 定稿时并入协议文档。
   */
  'ui.activeProject': { params: { workspace: string }; result: void }

  // ── 配置 ──
  'config.setProvider': { params: { config: ProviderConfig }; result: { error: string | null } }
  'config.checkProvider': { params: { config: ProviderConfig }; result: { message: string } }
  'config.setApproval': { params: { mode: ApprovalMode }; result: void }
  'config.setEffort': { params: { effort: Effort }; result: void }

  // ── 改动审阅 ──
  'change.count': { params: { threadId: string }; result: { count: number } }
  'change.list': { params: { threadId: string }; result: unknown[] }
  'change.revertCard': { params: { threadId: string; cardId: string }; result: { ok: boolean } }
  'change.revertFile': { params: { threadId: string; path: string }; result: { ok: boolean } }
  'change.revertAll': { params: { threadId: string }; result: { ok: boolean } }

  // ── 调试 ──
  'debug.trace': { params: { text: string }; result: void }
  'debug.log.clear': { params: Record<string, never>; result: void }
  /** 主机环境信息（管理页要展示"模板会建到哪"、配置文件在哪）。 */
  'debug.hostInfo': {
    params: Record<string, never>
    result: { homeDir: string; extensionsDir: string; configPath: string }
  }

  // ── 统计（协议 §3.10）──
  /**
   * 当前工作区/模式下"系统提示词与工具表有多大"（字符数）。
   *
   * 存在的理由：会话还没跑过第一轮时，界面没有实测值（`Thread.lastSystemPromptChars` 是主机
   * 组装提示词时记下的），而上下文明细想给出**预计**占用。M2 之前界面在**渲染路径里**同步
   * 组装系统提示词来算这个数（等于在渲染里读磁盘）；现在改成向主机要一次。
   */
  'stats.promptChars': {
    params: { workspace: string; mode: AgentMode }
    result: { systemChars: number; toolSpecsChars: number }
  }

  // ── 插件管理（协议 §3.8；M2 收口：UI 不再直接摸加载器与配置文件）──
  /**
   * 插件管理页的**一次取全**：卡片、能力开关、每个插件的配置草稿、密钥是否已设置、诊断。
   *
   * 为什么合成一个方法而不是拆成五个：这一页打开时全都要，拆开就是五趟往返
   * （进程内无所谓，WebSocket 上就是五次 RTT）。协议 §3.8 的细分方法留给后续按需用。
   */
  'plugin.list': {
    params: { workspace: string }
    result: {
      plugins: PluginItem[]
      capabilities: ResolvedPluginCapabilitiesDto
      /** 每个插件的已存配置（非密钥） */
      configs: Record<string, Record<string, unknown>>
      /** 密钥是否已设置：键是 `${pluginId}:${key}`，**值只有布尔**——密钥永不回明文 */
      secrets: Record<string, boolean>
      diagnostics: PluginDiagnostic[]
    }
  }
  'plugin.capabilities.set': { params: { patch: Partial<PluginCapabilities> }; result: void }
  'plugin.config.set': { params: { pluginId: string; values: Record<string, unknown> }; result: void }
  'plugin.secret.set': { params: { pluginId: string; key: string; value: string }; result: void }
  'plugin.setEnabled': { params: { pluginId: string; enabled: boolean; workspace: string }; result: void }
  'plugin.delete': { params: { filePath: string; workspace: string }; result: { ok: boolean } }
  'plugin.createTemplate': {
    params: { workspace: string; scope: 'workspace' | 'global'; name: string; code?: string }
    result: { filePath: string }
  }
  'plugin.builtinCatalog': { params: Record<string, never>; result: BuiltinToolInfo[] }

  // ── 技能 / 提示词 / 子智能体档案（协议 §3.9）──
  'skill.list': { params: { workspace: string }; result: SkillSummary[] }
  'skill.setEnabled': { params: { id: string; enabled: boolean; workspace?: string }; result: void }
  'skill.create': {
    params: { name: string; description: string; scope: 'workspace' | 'global'; workspace: string; body?: string }
    result: { filePath: string }
  }
  'skill.delete': { params: { id: string; workspace: string }; result: { ok: boolean } }

  'prompt.list': { params: { workspace: string }; result: PromptItem[] }
  'prompt.setEnabled': { params: { id: string; enabled: boolean; workspace: string }; result: { ok: boolean } }
  'prompt.create': {
    params: {
      workspace: string
      /** 与主机侧 `CreatePromptOptions` 同形（契约层不 import 实现，所以在这里写开来） */
      options: {
        name: string
        description?: string
        argumentHint?: string
        content: string
        scope: 'workspace' | 'global'
        isSystem?: boolean
        enabled?: boolean
      }
    }
    result: PromptItem
  }
  'prompt.update': { params: { item: PromptItem }; result: { ok: boolean } }
  'prompt.delete': { params: { filePath: string }; result: { ok: boolean } }

  'subagentProfile.list': { params: { workspace?: string }; result: SubagentProfile[] }
  'subagentProfile.setEnabled': {
    params: { id: string; enabled: boolean; workspace?: string }
    result: void
  }
  'subagentProfile.delete': { params: { id: string; workspace?: string }; result: { ok: boolean } }

  // ── 配置读取（协议 §3.7）──
  /**
   * 读已保存的供应商配置。
   *
   * **`apiKey` 只在这里回**：它是用户自己要在设置里编辑的供应商密钥，今天明文存在
   * `config.json`；而**快照里的 `config` 恒不含 apiKey**（界面常态展示不需要它）。
   * 插件密钥走的是另一套（`plugin.secret.*`，永不回明文）。
   */
  'config.get': { params: Record<string, never>; result: { saved: Partial<ProviderConfig>; path: string } }
  'config.presets': { params: Record<string, never>; result: ProviderPreset[] }
}

/** 通知主题（服务端 → 客户端，无 id，带 `seq`）。M0 只登记名字，M1 才真正发。 */
export const EVENT_TOPICS = [
  'evt.thread.upserted',
  'evt.thread.removed',
  'evt.thread.items',
  'evt.item.upserted',
  'evt.item.patch',
  'evt.message.delta',
  'evt.card.updated',
  'evt.queue.updated',
  'evt.thread.running',
  'evt.workspace.scanned',
  'evt.log.appended',
  'evt.stats.updated',
  'evt.plugin.changed',
  'evt.change.count',
  'evt.approval.pending',
  'evt.approval.settled',
  'evt.question.pending',
  'evt.question.settled',
  'evt.progress',
  'evt.notify',
  'evt.host.log',
] as const

export type EventTopic = (typeof EVENT_TOPICS)[number]

/** 反向请求（服务端 → 客户端，有 id，客户端必须应答）。 */
export const REVERSE_REQUESTS = [
  'req.approval.decide',
  'req.question.ask',
  'req.ui.notify',
] as const

export type ReverseRequest = (typeof REVERSE_REQUESTS)[number]

export type ProtocolMethod = keyof ProtocolCommands
export type ParamsOf<M extends ProtocolMethod> = ProtocolCommands[M]['params']
export type ResultOf<M extends ProtocolMethod> = ProtocolCommands[M]['result']

/** 协议版本：major 不匹配拒绝握手，minor 只做能力位协商（协议 §8）。 */
export const PROTOCOL_VERSION = '1.0'
