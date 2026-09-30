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

import type { AgentMode, ApprovalMode, Effort } from './dto'
import type { ProviderConfig } from './dto'

/** 命令（客户端 → 服务端，有响应）。M0 先覆盖 UI 实际用到的那些（协议 §9.1 的 A 组 + 焦点上报）。 */
export interface ProtocolCommands {
  // ── 会话 ──
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
