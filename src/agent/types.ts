/**
 * 界面层的数据形状：一个会话持有什么、每行卡片渲染什么。
 *
 * 发给模型的消息模型在 `core/types.ts`（AgentMessage）。会话原样保存那一份，
 * 所以 thread.messages 永远等于真正发出去过的历史，不再有第二套转换。
 */

import type { AgentMessage } from './core/types'

export type ToolStatus = 'awaiting' | 'running' | 'done' | 'error' | 'denied'

export type Item =
  | { kind: 'user'; id: string; at: number; text: string; queued?: boolean }
  | {
      /** 模型的思考链（reasoning_content）。默认折叠，只留一行时长。 */
      kind: 'thinking'
      id: string
      /** 第一次收到思考增量的时刻，用来算「持续了几秒」。 */
      at: number
      text: string
      /** 这段思考结束的时刻：思考完就不再增长。 */
      endedAt?: number
    }
  | { kind: 'assistant'; id: string; at: number; text: string; streaming?: boolean }
  | {
      kind: 'tool'
      id: string
      at: number
      callId: string
      name: string
      args: Record<string, unknown>
      rawArgs: string
      status: ToolStatus
      output?: string
      patch?: string
    }
  | { kind: 'notice'; id: string; at: number; text: string; level: 'info' | 'error' }

export interface Thread {
  id: string
  title: string
  createdAt: number
  /** The project this conversation works in. Every tool call is scoped to it. */
  workspace: string
  items: Item[]
  /** Everything the model has been told in this thread, in the core message model. */
  messages: AgentMessage[]
}

export interface DebugEntry {
  id: number
  at: number
  kind: 'request' | 'delta' | 'tools' | 'tool' | 'error' | 'info'
  text: string
}

export interface ToolOutcome {
  output: string
  ok: boolean
  patch?: string
}
