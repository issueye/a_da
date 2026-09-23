/**
 * 会话持久化类型定义
 * 参考 @earendil-works/pi-coding-agent/src/core/session-manager.ts
 */

import type { AgentMessage } from '../core/types'

export const CURRENT_SESSION_VERSION = 1

export interface SessionHeader {
  type: 'session'
  version: number
  id: string
  title?: string
  workspace: string
  createdAt: number
  updatedAt: number
  parentId?: string
  subagentId?: string
}

export interface SessionMessageEntry {
  type: 'message'
  id: string
  timestamp: number
  message: AgentMessage
}

export interface SessionNoticeEntry {
  type: 'notice'
  id: string
  timestamp: number
  text: string
  level: 'info' | 'error'
}

export interface SessionCompactEntry {
  type: 'compact'
  id: string
  timestamp: number
  summary: string
  preTokens: number
  postTokens: number
  savedTokens: number
  turnsSummarized: number
  customInstructions?: string
}

export type SessionEntry = SessionHeader | SessionMessageEntry | SessionNoticeEntry | SessionCompactEntry

export interface SessionSummary {
  id: string
  title: string
  workspace: string
  createdAt: number
  updatedAt: number
  filePath: string
  parentId?: string
  subagentId?: string
}
