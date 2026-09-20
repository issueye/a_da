/**
 * 会话持久化管理器 (SessionManager)
 * 参考 @earendil-works/pi-coding-agent/src/core/session-manager.ts
 * 基于追加式 JSONL 格式落盘，支持崩溃安全与重启后历史恢复
 *
 * 落盘位置与 config.ts 的配置文件同一个目录（`~/.a-da`，A_DA_HOME 可覆盖），
 * 一个应用只该有一个数据目录。
 */

import { appendFile, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import type { AgentMessage } from '../core/types'
import { getAppHome } from '../home'
import { CURRENT_SESSION_VERSION, type SessionEntry, type SessionHeader, type SessionSummary } from './types'

export function getSessionsDir(): string {
  return join(getAppHome(), 'sessions')
}

export class SessionManager {
  private explicitDir?: string

  /**
   * `dir` 只在测试里用；默认目录是**每次操作时**解析的。
   *
   * 不能在这里就定下来：`defaultSessionManager` 是模块级单例，构造发生在
   * import 的那一刻——那时 `A_DA_HOME` 还没被读到，测试想换目录就没机会了。
   */
  constructor(dir?: string) {
    this.explicitDir = dir
  }

  private get sessionsDir(): string {
    return this.explicitDir ?? getSessionsDir()
  }

  private async ensureDir(): Promise<void> {
    const dir = this.sessionsDir
    if (!existsSync(dir)) {
      await mkdir(dir, { recursive: true })
    }
  }

  private getSessionPath(sessionId: string): string {
    // 移除非法字符
    const safeId = sessionId.replace(/[^a-zA-Z0-9_-]/g, '_')
    return join(this.sessionsDir, `${safeId}.jsonl`)
  }

  /**
   * 创建新的持久化会话文件并写入首行 Header
   */
  async createSession(id: string, workspace: string, title: string = '新会话'): Promise<SessionHeader> {
    await this.ensureDir()
    const filePath = this.getSessionPath(id)
    const now = Date.now()

    const header: SessionHeader = {
      type: 'session',
      version: CURRENT_SESSION_VERSION,
      id,
      title,
      workspace,
      createdAt: now,
      updatedAt: now,
    }

    await writeFile(filePath, `${JSON.stringify(header)}\n`, 'utf-8')
    return header
  }

  /**
   * 向会话以 Append-only 方式追加一条消息
   */
  async appendMessage(sessionId: string, message: AgentMessage): Promise<void> {
    await this.ensureDir()
    const filePath = this.getSessionPath(sessionId)
    const entry: SessionEntry = {
      type: 'message',
      id: `msg_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      timestamp: Date.now(),
      message,
    }

    await appendFile(filePath, `${JSON.stringify(entry)}\n`, 'utf-8')
  }

  /**
   * 更新会话标题
   */
  async updateSessionTitle(sessionId: string, title: string): Promise<void> {
    const filePath = this.getSessionPath(sessionId)
    if (!existsSync(filePath)) return

    try {
      const content = await readFile(filePath, 'utf-8')
      const lines = content.split('\n')
      if (lines.length > 0 && lines[0]?.trim()) {
        const header = JSON.parse(lines[0]!) as SessionHeader
        if (header.type === 'session') {
          header.title = title
          header.updatedAt = Date.now()
          lines[0] = JSON.stringify(header)
          await writeFile(filePath, lines.join('\n'), 'utf-8')
        }
      }
    } catch {
      // 忽略非致命读取错误
    }
  }

  /**
   * 删掉一个会话的流水文件。删一个本来就不存在的会话不算错。
   */
  async deleteSession(sessionId: string): Promise<void> {
    const filePath = this.getSessionPath(sessionId)
    if (!existsSync(filePath)) return
    await rm(filePath, { force: true })
  }

  /**
   * 读取并重建指定会话的所有消息历史
   */
  async loadSession(sessionId: string): Promise<{ header: SessionHeader; messages: AgentMessage[] } | null> {
    const filePath = this.getSessionPath(sessionId)
    if (!existsSync(filePath)) return null

    try {
      const content = await readFile(filePath, 'utf-8')
      const lines = content.split('\n')
      let header: SessionHeader | null = null
      const messages: AgentMessage[] = []

      for (const line of lines) {
        if (!line.trim()) continue
        try {
          const entry = JSON.parse(line) as SessionEntry
          if (entry.type === 'session') {
            header = entry
          } else if (entry.type === 'message') {
            messages.push(entry.message)
          }
        } catch {
          // 容错跳过损坏单行
        }
      }

      if (!header) return null
      return { header, messages }
    } catch {
      return null
    }
  }

  /**
   * 列举指定工作区的所有历史会话摘要（按修改时间倒序排列）
   */
  async listSessionsForWorkspace(workspace: string): Promise<SessionSummary[]> {
    await this.ensureDir()
    const summaries: SessionSummary[] = []

    let files: string[] = []
    try {
      files = await readdir(this.sessionsDir)
    } catch {
      return []
    }

    for (const file of files) {
      if (!file.endsWith('.jsonl')) continue
      const filePath = join(this.sessionsDir, file)
      try {
        const content = await readFile(filePath, 'utf-8')
        const firstLine = content.split('\n')[0]
        if (!firstLine) continue
        const header = JSON.parse(firstLine) as SessionHeader
        if (header.type === 'session' && header.workspace === workspace) {
          const fileStat = await stat(filePath)
          summaries.push({
            id: header.id,
            title: header.title || '新会话',
            workspace: header.workspace,
            createdAt: header.createdAt,
            updatedAt: fileStat.mtimeMs || header.updatedAt,
            filePath,
          })
        }
      } catch {
        continue
      }
    }

    return summaries.sort((a, b) => b.updatedAt - a.updatedAt)
  }
}

export const defaultSessionManager = new SessionManager()
