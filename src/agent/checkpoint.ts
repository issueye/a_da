/**
 * 写操作检查点：改动前的文件快照与回滚。
 *
 * 每次内置写工具（write_file / edit_file）获准执行前，把目标文件当下的内容
 * 快照一份，按会话追加进 `~/.a-da/checkpoints/<threadId>.jsonl`。之后用户可以：
 * - 撤销单次工具调用（revertCheckpoint：恢复这张卡的 pre-state）
 * - 把某个文件恢复到 Agent 动它之前的样子（revertFile：最早的未作废快照）
 * - 一键恢复整个会话动过的所有文件（revertAll）
 *
 * 回滚是显式动作而不是自动拦截：这里不猜测模型意图，只保证「有得退」。
 * run_command 里发生的改动（git、构建脚本）不在此跟踪范围内。
 */

import { appendFile, readFile, rm, writeFile, mkdir } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { getAppHome } from './home'

/** 单文件快照上限：超大文件不存内容（回滚时该文件只能跳过）。 */
const MAX_SNAPSHOT_BYTES = 5 * 1024 * 1024

export interface CheckpointFile {
  /** 展示用的相对路径（posix 分隔符） */
  path: string
  /** 回滚用的绝对路径（真实落点） */
  absolute: string
  existed: boolean
  contentBase64?: string
  /** 文件太大没存内容：回滚时这个文件只能跳过 */
  snapshotIncomplete?: boolean
}

export interface CheckpointRecord {
  type: 'checkpoint'
  id: string
  threadId: string
  toolCallId: string
  at: number
  files: CheckpointFile[]
}

export interface RevertRecord {
  type: 'revert'
  id: string
  at: number
  /** 被这次回滚作废的检查点 id */
  checkpointIds: string[]
  scope: 'single' | 'file' | 'all'
  detail?: string
}

type Entry = CheckpointRecord | RevertRecord

export interface RevertOutcome {
  /** 内容被恢复的文件（改动前存在） */
  restored: string[]
  /** 被删除的文件（改动前不存在） */
  deleted: string[]
  /** 没法处理的文件（快照不完整） */
  skipped: string[]
  /** 这次回滚作废的检查点 id */
  invalidated: string[]
}

function samePath(a: string, b: string): boolean {
  const left = resolve(a)
  const right = resolve(b)
  return process.platform === 'win32'
    ? left.toLowerCase() === right.toLowerCase()
    : left === right
}

let counter = 0
const nextId = (prefix: string) => `${prefix}_${Date.now().toString(36)}_${++counter}`

export class CheckpointManager {
  private cache = new Map<string, Entry[] | null>()

  private fileFor(threadId: string): string {
    return join(getAppHome(), 'checkpoints', `${threadId}.jsonl`)
  }

  /** 读取一个会话的全部检查点条目（损坏行跳过；null 表示文件不存在）。 */
  async load(threadId: string, force = false): Promise<Entry[] | null> {
    if (!force && this.cache.has(threadId)) return this.cache.get(threadId)!
    const file = this.fileFor(threadId)
    if (!existsSync(file)) {
      this.cache.set(threadId, null)
      return null
    }
    const text = await readFile(file, 'utf-8')
    const entries: Entry[] = []
    for (const line of text.split('\n')) {
      const trimmed = line.trim()
      if (!trimmed) continue
      try {
        entries.push(JSON.parse(trimmed) as Entry)
      } catch {
        // 单行损坏不影响其余流水
      }
    }
    this.cache.set(threadId, entries)
    return entries
  }

  private async append(threadId: string, entry: Entry): Promise<void> {
    const file = this.fileFor(threadId)
    await mkdir(dirname(file), { recursive: true })
    await appendFile(file, `${JSON.stringify(entry)}\n`, 'utf-8')
    const cached = this.cache.get(threadId)
    if (cached) cached.push(entry)
  }

  /**
   * 执行写工具之前快照目标文件。读取失败按「新文件」处理——write_file 对
   * 不存在的路径是合法操作，回滚时把它删掉就是。
   */
  async capture(
    threadId: string,
    toolCallId: string,
    files: Array<{ path: string; absolute: string }>
  ): Promise<CheckpointRecord> {
    const snapshotFiles: CheckpointFile[] = []
    for (const file of files) {
      const entry: CheckpointFile = { path: file.path, absolute: file.absolute, existed: false }
      try {
        const content = await readFile(file.absolute)
        entry.existed = true
        if (content.length <= MAX_SNAPSHOT_BYTES) {
          entry.contentBase64 = content.toString('base64')
        } else {
          entry.snapshotIncomplete = true
        }
      } catch {
        entry.existed = false
      }
      snapshotFiles.push(entry)
    }

    const record: CheckpointRecord = {
      type: 'checkpoint',
      id: nextId('ckpt'),
      threadId,
      toolCallId,
      at: Date.now(),
      files: snapshotFiles,
    }
    await this.append(threadId, record)
    return record
  }

  private async applyRestore(files: CheckpointFile[]): Promise<Omit<RevertOutcome, 'invalidated'>> {
    const outcome = { restored: [] as string[], deleted: [] as string[], skipped: [] as string[] }
    // 逆序恢复：后写的先撤，尽量贴近当时的落盘顺序
    for (const file of [...files].reverse()) {
      if (!file.existed) {
        await rm(file.absolute, { force: true }).catch(() => {})
        outcome.deleted.push(file.path)
      } else if (file.contentBase64 !== undefined) {
        await mkdir(dirname(file.absolute), { recursive: true }).catch(() => {})
        await writeFile(file.absolute, Buffer.from(file.contentBase64, 'base64')).catch(() => {})
        outcome.restored.push(file.path)
      } else {
        outcome.skipped.push(file.path)
      }
    }
    return outcome
  }

  private isInvalidated(entries: Entry[] | null, checkpointId: string): boolean {
    if (!entries) return false
    return entries.some((entry) => entry.type === 'revert' && entry.checkpointIds.includes(checkpointId))
  }

  /** 撤销单次工具调用：恢复这张检查点记录的所有文件。已作废或不存在返回 null。 */
  async revertCheckpoint(threadId: string, checkpointId: string): Promise<RevertOutcome | null> {
    const entries = await this.load(threadId)
    if (!entries) return null
    const record = entries.find(
      (entry): entry is CheckpointRecord => entry.type === 'checkpoint' && entry.id === checkpointId
    )
    if (!record || this.isInvalidated(entries, checkpointId)) return null

    const outcome = await this.applyRestore(record.files)
    const revert: RevertRecord = {
      type: 'revert',
      id: nextId('rvrt'),
      at: Date.now(),
      checkpointIds: [checkpointId],
      scope: 'single',
    }
    await this.append(threadId, revert)
    return { ...outcome, invalidated: [checkpointId] }
  }

  /**
   * 把一个文件恢复到 Agent 第一次动它之前的样子：用最早的未作废快照，
   * 并把该文件名下所有仍有效的检查点一并作废。
   */
  async revertFile(threadId: string, absolutePath: string): Promise<RevertOutcome | null> {
    const entries = await this.load(threadId)
    if (!entries) return null

    const active = entries.filter(
      (entry): entry is CheckpointRecord =>
        entry.type === 'checkpoint' && !this.isInvalidated(entries, entry.id)
    )
    const mine = active.filter((entry) => entry.files.some((file) => samePath(file.absolute, absolutePath)))
    if (mine.length === 0) return null

    const earliest = mine[0]!
    const target = earliest.files.find((file) => samePath(file.absolute, absolutePath))!
    const outcome = await this.applyRestore([target])
    const invalidated = mine.map((entry) => entry.id)
    await this.append(threadId, {
      type: 'revert',
      id: nextId('rvrt'),
      at: Date.now(),
      checkpointIds: invalidated,
      scope: 'file',
      detail: target.path,
    })
    return { ...outcome, invalidated }
  }

  /** 一键恢复：把会话里所有仍有效的检查点涉及的文件全部还原。 */
  async revertAll(threadId: string): Promise<RevertOutcome | null> {
    const entries = await this.load(threadId)
    if (!entries) return null

    const active = entries.filter(
      (entry): entry is CheckpointRecord =>
        entry.type === 'checkpoint' && !this.isInvalidated(entries, entry.id)
    )
    if (active.length === 0) return null

    // 每个文件取最早的快照（即 Agent 动它之前的状态）
    const earliest = new Map<string, CheckpointFile>()
    const invalidated: string[] = []
    for (const record of active) {
      invalidated.push(record.id)
      for (const file of record.files) {
        const key = resolve(file.absolute).toLowerCase()
        if (!earliest.has(key)) earliest.set(key, file)
      }
    }

    const outcome = await this.applyRestore([...earliest.values()])
    await this.append(threadId, {
      type: 'revert',
      id: nextId('rvrt'),
      at: Date.now(),
      checkpointIds: invalidated,
      scope: 'all',
    })
    return { ...outcome, invalidated }
  }

  /** 会话被删除时清掉它的检查点流水。 */
  async discard(threadId: string): Promise<void> {
    this.cache.delete(threadId)
    await rm(this.fileFor(threadId), { force: true }).catch(() => {})
  }
}

export const defaultCheckpointManager = new CheckpointManager()
