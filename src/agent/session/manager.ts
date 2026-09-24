/**
 * 会话持久化管理器 (SessionManager)
 * 参考 @earendil-works/pi-agent-core 与 @earendil-works/pi-coding-agent 架构设计。
 * 基于追加式 JSONL 格式落盘，支持崩溃安全与重启后历史恢复
 *
 * 落盘位置与 config.ts 的配置文件同一个目录（`~/.a-da`，A_DA_HOME 可覆盖），
 * 一个应用只该有一个数据目录：
 *
 *   ~/.a-da/sessions/<工作区>/<会话 id>.jsonl
 *
 * 工作区是一层目录而不是字段前缀，因为「这个项目有哪些会话」是最常问的问题：
 * 列一个目录就答完了，不必扫全部文件再按字段过滤。目录名是工作区路径的散列，
 * 带一个 `.json` 边车记下真实路径——路径里有 `\ / :` 这类不能做目录名的字符，
 * 散列也让目录名不会长到踩到 Windows 的路径上限。
 */

import { appendFile, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { join, resolve } from 'node:path'
import type { AgentMessage } from '../core/types'
import { getAppHome } from '../home'
import { CURRENT_SESSION_VERSION, type SessionCompactEntry, type SessionEntry, type SessionHeader, type SessionSummary } from './types'
import { buildCompactSummaryMessage } from '../compact/prompt'

export function getSessionsDir(): string {
  return join(getAppHome(), 'sessions')
}

/**
 * `E:\code\a_da` → `1f3c…a9`：稳定、短，且没有文件系统不接受的字符。
 *
 * 分隔符和大小写都要归一化：同一个项目会以 `E:/code/a_da`（用户粘的）和
 * `E:\code\a_da`（对话框选的）两种形式出现，不归一化就会散列成两个目录，一个
 * 项目的会话被劈成两半。
 */
function workspaceSlug(workspace: string): string {
  const canonical = workspace.replace(/[\\/]+/g, '\\').toLowerCase()
  return createHash('sha256').update(canonical).digest('hex').slice(0, 20)
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

  /** 一个工作区的目录。不判断存在性，调用方按需 `ensureDir`。 */
  private workspaceDir(workspace: string): string {
    return join(this.sessionsDir, workspaceSlug(workspace))
  }

  /** 工作区目录下的边车文件路径。 */
  private workspacePointer(workspace: string): string {
    return join(this.workspaceDir(workspace), 'workspace.json')
  }

  /**
   * 工作区的边车文件，记下这个目录对应的真实路径。
   *
   * 启动时要按工作区恢复会话，而目录名是散列——真实路径只能另存一份。存调用方给的
   * 原样（`E:/code` 就存 `E:/code`），因为侧边栏显示的就是它；归一化是散列的事，
   * 写指针不必再改一遍。
   *
   * 写坏了不影响会话文件本身，所以失败就静默：那个目录照样能被列举，只是路径未知。
   */
  private async rememberWorkspace(workspace: string): Promise<void> {
    try {
      const dir = this.workspaceDir(workspace)
      await mkdir(dir, { recursive: true })
      await writeFile(
        this.workspacePointer(workspace),
        `${JSON.stringify({ workspace }, null, 2)}\n`,
        'utf-8',
      )
    } catch {
      // 一个写不进去的指针只影响「按工作区恢复」的体验，不该让保存本身失败。
    }
  }

  private getSessionPath(workspace: string, sessionId: string): string {
    // 移除非法字符
    const safeId = sessionId.replace(/[^a-zA-Z0-9_-]/g, '_')
    return join(this.workspaceDir(workspace), `${safeId}.jsonl`)
  }

  /**
   * 创建新的持久化会话文件并写入首行 Header
   */
  async createSession(
    id: string,
    workspace: string,
    title: string = '新会话',
    meta?: { parentId?: string; subagentId?: string },
  ): Promise<SessionHeader> {
    await this.rememberWorkspace(workspace)
    const filePath = this.getSessionPath(workspace, id)
    const now = Date.now()

    const header: SessionHeader = {
      type: 'session',
      version: CURRENT_SESSION_VERSION,
      id,
      title,
      workspace,
      createdAt: now,
      updatedAt: now,
      parentId: meta?.parentId,
      subagentId: meta?.subagentId,
    }

    await writeFile(filePath, `${JSON.stringify(header)}\n`, 'utf-8')
    return header
  }

  /**
   * 向会话以 Append-only 方式追加一条消息
   */
  async appendMessage(sessionId: string, message: AgentMessage, workspace?: string): Promise<void> {
    const dir = workspace ? this.workspaceDir(workspace) : await this.findSessionDir(sessionId)
    if (!dir) return
    if (!existsSync(dir)) await mkdir(dir, { recursive: true })
    const filePath = join(dir, `${sessionId.replace(/[^a-zA-Z0-9_-]/g, '_')}.jsonl`)
    const entry: SessionEntry = {
      type: 'message',
      id: `msg_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      timestamp: Date.now(),
      message,
    }

    await appendFile(filePath, `${JSON.stringify(entry)}\n`, 'utf-8')
  }

  /**
   * 向会话以 Append-only 方式追加一条上下文压缩摘要记录
   */
  async appendCompactEntry(
    sessionId: string,
    entry: Omit<SessionCompactEntry, 'type'>,
    workspace?: string,
  ): Promise<void> {
    const dir = workspace ? this.workspaceDir(workspace) : await this.findSessionDir(sessionId)
    if (!dir) return
    if (!existsSync(dir)) await mkdir(dir, { recursive: true })
    const filePath = join(dir, `${sessionId.replace(/[^a-zA-Z0-9_-]/g, '_')}.jsonl`)
    const compactEntry: SessionCompactEntry = {
      type: 'compact',
      ...entry,
    }

    await appendFile(filePath, `${JSON.stringify(compactEntry)}\n`, 'utf-8')
  }

  /**
   * 一个会话现在在哪个工作区目录下。
   *
   * 追加消息时调用方不一定带着工作区（会话流水是追加写的流水账，写不进去也不该
   * 打断这一轮），所以得能自己找。扫的目录数量等于工作区数量，启动后基本不变。
   */
  private async findSessionDir(sessionId: string): Promise<string | null> {
    let entries: { name: string; isDirectory: () => boolean }[]
    try {
      entries = await readdir(this.sessionsDir, { withFileTypes: true })
    } catch {
      return null
    }
    const safe = sessionId.replace(/[^a-zA-Z0-9_-]/g, '_')
    for (const entry of entries) {
      // 只进目录：工作区一层是散列目录，同级的 `.jsonl` 之类一律跳过。
      if (!entry.isDirectory()) continue
      const candidate = join(this.sessionsDir, entry.name, `${safe}.jsonl`)
      if (existsSync(candidate)) return join(this.sessionsDir, entry.name)
    }
    return null
  }

  /**
   * 更新会话标题
   */
  async updateSessionTitle(sessionId: string, title: string, workspace?: string): Promise<void> {
    const filePath = workspace
      ? this.getSessionPath(workspace, sessionId)
      : await this.findSessionPath(sessionId)
    if (!filePath || !existsSync(filePath)) return

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
   * 更新会话的元数据（如纠正父会话挂载关系）
   */
  async updateSessionMeta(
    sessionId: string,
    meta: { parentId?: string; subagentId?: string },
    workspace?: string
  ): Promise<void> {
    const filePath = workspace
      ? this.getSessionPath(workspace, sessionId)
      : await this.findSessionPath(sessionId)
    if (!filePath || !existsSync(filePath)) return

    try {
      const content = await readFile(filePath, 'utf-8')
      const lines = content.split('\n')
      if (lines.length > 0 && lines[0]?.trim()) {
        const header = JSON.parse(lines[0]!) as SessionHeader
        if (header.type === 'session') {
          if (meta.parentId !== undefined) header.parentId = meta.parentId
          if (meta.subagentId !== undefined) header.subagentId = meta.subagentId
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
   * 重写会话的消息列表（截断并重写磁盘 JSONL 文件，保留 SessionHeader）
   */
  async rewriteSessionMessages(
    sessionId: string,
    messages: AgentMessage[],
    workspace?: string,
  ): Promise<void> {
    const filePath = workspace
      ? this.getSessionPath(workspace, sessionId)
      : await this.findSessionPath(sessionId)
    if (!filePath || !existsSync(filePath)) return

    try {
      const content = await readFile(filePath, 'utf-8')
      const lines = content.split('\n')
      let headerLine = ''
      if (lines.length > 0 && lines[0]?.trim()) {
        try {
          const parsed = JSON.parse(lines[0]!)
          if (parsed.type === 'session') {
            parsed.updatedAt = Date.now()
            headerLine = `${JSON.stringify(parsed)}\n`
          }
        } catch {}
      }
      if (!headerLine) {
        headerLine = `${JSON.stringify({
          type: 'session',
          version: CURRENT_SESSION_VERSION,
          id: sessionId,
          title: '会话',
          workspace: workspace || process.cwd(),
          createdAt: Date.now(),
          updatedAt: Date.now(),
        })}\n`
      }

      let newContent = headerLine
      for (const msg of messages) {
        const entry: SessionEntry = {
          type: 'message',
          id: `msg_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
          timestamp: msg.timestamp || Date.now(),
          message: msg,
        }
        newContent += `${JSON.stringify(entry)}\n`
      }

      await writeFile(filePath, newContent, 'utf-8')
    } catch (err) {
      console.warn('[SessionManager] rewriteSessionMessages failed:', err)
    }
  }

  /** 一个会话文件在哪，`findSessionDir` 的文件版。 */
  private async findSessionPath(sessionId: string): Promise<string | null> {
    const dir = await this.findSessionDir(sessionId)
    if (!dir) return null
    return join(dir, `${sessionId.replace(/[^a-zA-Z0-9_-]/g, '_')}.jsonl`)
  }

  /**
   * 删掉一个会话的流水文件。删一个本来就不存在的会话不算错。
   */
  async deleteSession(sessionId: string, workspace?: string): Promise<void> {
    const filePath = workspace
      ? this.getSessionPath(workspace, sessionId)
      : await this.findSessionPath(sessionId)
    if (!filePath || !existsSync(filePath)) return
    await rm(filePath, { force: true })

    // 级联删除名下的所有子智能体会话
    try {
      const dir = workspace ? this.workspaceDir(workspace) : await this.findSessionDir(sessionId)
      if (dir) {
        const ws = await this.readWorkspacePointer(dir)
        if (ws) {
          const summaries = await this.listSessionsForWorkspace(ws)
          for (const s of summaries) {
            if (s.parentId === sessionId) {
              await rm(s.filePath, { force: true })
            }
          }
        }
      }
    } catch {
      // 容错处理
    }
  }

  /**
   * 删掉一个工作区的全部会话与目录（包含 workspace.json 和所有 .jsonl）。
   * 删一个本来就不存在的工作区不算错。
   */
  async deleteWorkspace(workspace: string): Promise<void> {
    const dir = this.workspaceDir(workspace)
    if (!existsSync(dir)) return
    await rm(dir, { recursive: true, force: true })
  }

  /**
   * 读取并重建指定会话的所有消息历史
   */
  async loadSession(
    sessionId: string,
    workspace?: string,
  ): Promise<{ header: SessionHeader; messages: AgentMessage[] } | null> {
    const filePath = workspace
      ? this.getSessionPath(workspace, sessionId)
      : await this.findSessionPath(sessionId)
    if (!filePath || !existsSync(filePath)) return null

    try {
      const content = await readFile(filePath, 'utf-8')
      const lines = content.split('\n')
      let header: SessionHeader | null = null
      let messages: AgentMessage[] = []

      for (const line of lines) {
        if (!line.trim()) continue
        try {
          const entry = JSON.parse(line) as SessionEntry
          if (entry.type === 'session') {
            header = entry
          } else if (entry.type === 'message') {
            messages.push(entry.message)
          } else if (entry.type === 'compact') {
            const continuationMsg: AgentMessage = {
              role: 'user',
              content: buildCompactSummaryMessage(entry.summary, { recentMessagesPreserved: true }),
              timestamp: entry.timestamp,
            }
            messages = [continuationMsg]
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
   *
   * 目录名是散列，所以这里算一次散列去取那个目录，而不是扫全部再比字段。
   */
  async listSessionsForWorkspace(workspace: string): Promise<SessionSummary[]> {
    const dir = this.workspaceDir(workspace)
    let files: string[] = []
    try {
      files = await readdir(dir)
    } catch {
      return []
    }

    const summaries: SessionSummary[] = []
    for (const file of files) {
      if (!file.endsWith('.jsonl')) continue
      const filePath = join(dir, file)
      try {
        const content = await readFile(filePath, 'utf-8')
        const firstLine = content.split('\n')[0]
        if (!firstLine) continue
        const header = JSON.parse(firstLine) as SessionHeader
        if (header.type !== 'session') continue
        const fileStat = await stat(filePath)
        summaries.push({
          id: header.id,
          title: header.title || '新会话',
          workspace: header.workspace,
          createdAt: header.createdAt,
          updatedAt: fileStat.mtimeMs || header.updatedAt,
          filePath,
          parentId: header.parentId,
          subagentId: header.subagentId,
        })
      } catch {
        continue
      }
    }

    return summaries.sort((a, b) => b.updatedAt - a.updatedAt)
  }

  /**
   * 列举**所有**工作区的会话摘要，按修改时间倒序。
   *
   * 启动恢复要它：窗口重新打开时要把每个项目的会话都摆回来，而不是只看当前项目。
   * 每个工作区目录带一个 `workspace.json` 记着真实路径——目录名是散列，没有它就
   * 不知道该把会话挂到哪个项目上，那种目录只能跳过。
   */
  async listAllSessions(): Promise<SessionSummary[]> {
    let entries: string[]
    try {
      entries = await readdir(this.sessionsDir)
    } catch {
      return []
    }

    const summaries: SessionSummary[] = []
    for (const entry of entries) {
      const dir = join(this.sessionsDir, entry)
      const workspace = await this.readWorkspacePointer(dir)
      if (!workspace) continue
      summaries.push(...(await this.listSessionsForWorkspace(workspace)))
    }

    return summaries.sort((a, b) => b.updatedAt - a.updatedAt)
  }

  /** 读一个工作区目录的 `workspace.json`；没有或坏了就当这个目录不存在。 */
  private async readWorkspacePointer(dir: string): Promise<string | null> {
    try {
      const parsed = JSON.parse(await readFile(join(dir, 'workspace.json'), 'utf8')) as {
        workspace?: unknown
      }
      return typeof parsed.workspace === 'string' && parsed.workspace ? parsed.workspace : null
    } catch {
      return null
    }
  }

  // ---------------------------------------------------------------- sync reads
  //
  // 启动恢复走同步路径，和 `readSavedAppearance` 同一个理由：窗口的第一帧就得
  // 是对的，等异步任务回来再换会闪一下。这些都是小文件，读得起。

  /** `listAllSessions` 的同步版。 */
  listAllSessionsSync(): SessionSummary[] {
    let entries: string[]
    try {
      entries = readdirSync(this.sessionsDir)
    } catch {
      return []
    }

    const summaries: SessionSummary[] = []
    for (const entry of entries) {
      const dir = join(this.sessionsDir, entry)
      const workspace = this.readWorkspacePointerSync(dir)
      if (!workspace) continue
      summaries.push(...this.listSessionsForWorkspaceSync(workspace))
    }

    return summaries.sort((a, b) => b.updatedAt - a.updatedAt)
  }

  /** `listSessionsForWorkspace` 的同步版。 */
  listSessionsForWorkspaceSync(workspace: string): SessionSummary[] {
    const dir = this.workspaceDir(workspace)
    let files: string[]
    try {
      files = readdirSync(dir)
    } catch {
      return []
    }

    const summaries: SessionSummary[] = []
    for (const file of files) {
      if (!file.endsWith('.jsonl')) continue
      const filePath = join(dir, file)
      try {
        const firstLine = readFileSync(filePath, 'utf8').split('\n')[0]
        if (!firstLine) continue
        const header = JSON.parse(firstLine) as SessionHeader
        if (header.type !== 'session') continue
        summaries.push({
          id: header.id,
          title: header.title || '新会话',
          workspace: header.workspace,
          createdAt: header.createdAt,
          // 同步路径拿 mtime 要额外一次 stat，而 `updatedAt` 已经够排序用了。
          updatedAt: header.updatedAt,
          filePath,
          parentId: header.parentId,
          subagentId: header.subagentId,
        })
      } catch {
        continue
      }
    }

    return summaries.sort((a, b) => b.updatedAt - a.updatedAt)
  }

  /**
   * 读一个会话摘要对应的完整原始条目流水（同步）。
   */
  loadSummaryEntriesSync(summary: SessionSummary): SessionEntry[] {
    const filePath = summary.filePath ?? this.findSessionPathSync(summary.id, summary.workspace)
    if (!filePath) return []

    try {
      const entries: SessionEntry[] = []
      for (const line of readFileSync(filePath, 'utf8').split('\n')) {
        if (!line.trim()) continue
        try {
          const entry = JSON.parse(line) as SessionEntry
          entries.push(entry)
        } catch {
          // 容错跳过损坏单行
        }
      }
      return entries
    } catch {
      return []
    }
  }

  /**
   * 读一个会话摘要对应的消息流水（同步）。
   *
   * 若会话中存在 compact 压缩点，则自动将该点之前的消息压缩替换为 Continuation 消息，
   * 紧接后续追加或保留的消息。
   */
  loadSummaryMessagesSync(summary: SessionSummary): AgentMessage[] {
    const entries = this.loadSummaryEntriesSync(summary)
    let messages: AgentMessage[] = []

    for (const entry of entries) {
      if (entry.type === 'message') {
        messages.push(entry.message)
      } else if (entry.type === 'compact') {
        // 当遇到压缩点时，历史早期消息已在当时被压缩归档，由结构化 continuation 消息接续
        const continuationMsg: AgentMessage = {
          role: 'user',
          content: buildCompactSummaryMessage(entry.summary, { recentMessagesPreserved: true }),
          timestamp: entry.timestamp,
        }
        messages = [continuationMsg]
      }
    }

    return messages
  }

  /** `findSessionPath` 的同步版。 */
  private findSessionPathSync(sessionId: string, workspace?: string): string | null {
    const filePath = workspace
      ? this.getSessionPath(workspace, sessionId)
      : null
    if (filePath && existsSync(filePath)) return filePath
    const dir = this.findSessionDirSync(sessionId)
    return dir ? join(dir, `${sessionId.replace(/[^a-zA-Z0-9_-]/g, '_')}.jsonl`) : null
  }

  /** `findSessionDir` 的同步版。 */
  private findSessionDirSync(sessionId: string): string | null {
    let entries: { name: string; isDirectory: () => boolean }[]
    try {
      entries = readdirSync(this.sessionsDir, { withFileTypes: true })
    } catch {
      return null
    }
    const safe = sessionId.replace(/[^a-zA-Z0-9_-]/g, '_')
    for (const entry of entries) {
      if (!entry.isDirectory()) continue
      const candidate = join(this.sessionsDir, entry.name, `${safe}.jsonl`)
      if (existsSync(candidate)) return join(this.sessionsDir, entry.name)
    }
    return null
  }

  private readWorkspacePointerSync(dir: string): string | null {
    try {
      const parsed = JSON.parse(readFileSync(join(dir, 'workspace.json'), 'utf8')) as {
        workspace?: unknown
      }
      return typeof parsed.workspace === 'string' && parsed.workspace ? parsed.workspace : null
    } catch {
      return null
    }
  }
}

export const defaultSessionManager = new SessionManager()
