import type { Item, FileChange } from '../types'

/** 计算 patch 文本中的增删行数 */
export function patchStats(patch: string): { added: number; removed: number } {
  let added = 0
  let removed = 0
  for (const line of patch.split('\n')) {
    if (line.startsWith('+++') || line.startsWith('---')) continue
    if (line.startsWith('+')) added++
    else if (line.startsWith('-')) removed++
  }
  return { added, removed }
}

/** 规范化路径字符串 */
function normalizePath(p?: string): string {
  if (!p) return ''
  return p.trim().replace(/\\/g, '/')
}

/** 从 Item 的 args 中获取文件路径 */
function pathFromItem(item: Item): string {
  const args = item.args
  if (!args) return ''
  if (typeof args === 'string') return normalizePath(args)
  return normalizePath(args.path || args.file || args.filePath || '')
}

/**
 * 汇总一个会话的所有文件改动（write_file / edit_file），按文件聚合
 */
export function deriveThreadFileChanges(items: Item[]): FileChange[] {
  const byPath = new Map<string, FileChange>()

  for (const item of items) {
    const isTool = item.kind === 'toolCall' || item.kind === 'tool' || item.role === 'tool' || Boolean(item.tool || item.callId)
    if (!isTool) continue

    const toolName = item.tool || item.name || ''
    if (toolName !== 'write_file' && toolName !== 'edit_file') continue

    const status = item.state || item.status || 'done'
    if (status !== 'done' && status !== 'error' && status !== 'failed') continue

    const filePath = pathFromItem(item)
    if (!filePath) continue

    const patch = item.patch || (typeof item.output === 'string' && item.output.includes('@@') ? item.output : undefined)
    const stats = patch ? patchStats(patch) : { added: 1, removed: 0 }
    const cardId = item.callId || item.id

    const existing = byPath.get(filePath)
    if (existing) {
      existing.editsCount += 1
      existing.additions += stats.added
      existing.deletions += stats.removed
      existing.reverted = existing.reverted && Boolean(item.reverted)
      if (cardId && !existing.cardIds.includes(cardId)) {
        existing.cardIds.push(cardId)
      }
      if (patch) {
        existing.latestPatch = patch
      }
    } else {
      byPath.set(filePath, {
        path: filePath,
        latestPatch: patch,
        additions: stats.added,
        deletions: stats.removed,
        editsCount: 1,
        reverted: Boolean(item.reverted),
        cardIds: cardId ? [cardId] : [],
      })
    }
  }

  return Array.from(byPath.values())
}

/** 待保留的改动文件数（未撤销） */
export function deriveActiveChangeCount(items: Item[]): number {
  return deriveThreadFileChanges(items).filter((c) => !c.reverted).length
}
