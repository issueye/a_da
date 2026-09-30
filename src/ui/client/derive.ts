/**
 * 从**复制到的数据**本地推导 UI 要的派生值（M1 收尾的一部分）。
 *
 * M0 时这些读助手是"转手问 store"（`isThreadRunning` / `labelFor` / `getThreadFileChanges` …）。
 * 那在进程内能用，但 M3 起客户端读不到主机的内存对象——所以它们必须变成**纯函数**，
 * 输入只有快照里的数据。这个文件就是那次搬家的落点。
 *
 * 判据：**只吃快照/条目数据，不 import 任何 agent 实现模块**（协议设计 §7.1 的方向约束）。
 * `patchStats` 本来就是纯函数（`agent/patch`），UI 侧一直在用；这里沿用。
 */

import { patchStats } from '../../agent/patch'
import { workspaceLabel, isPublicWorkspace } from '../../agent/home'
import type { FileChange, Item } from '../../shared/protocol'

/**
 * 主题展示名：公共区显示为「公共区」，其余是短路径。
 *
 * 直接复用主机侧同一份实现（`agent/home` 的纯函数），避免"两处各写一套短路径规则"
 * 而在远端/本机显示不一致。
 */
export function deriveLabelFor(workspace: string, publicWorkspace: string): string {
  return workspaceLabel(workspace, publicWorkspace)
}

/** 这个路径是不是公共区（大小写与分隔符都归一化）。 */
export function deriveIsPublic(workspace: string, publicWorkspace: string): boolean {
  return isPublicWorkspace(workspace, publicWorkspace)
}

/**
 * 汇总一个会话的文件改动（`write_file` / `edit_file` / `edit_files` 卡片），按文件聚合。
 *
 * 与主机侧 `store.getThreadFileChanges` 同一套规则（那边将来可以删掉，UI 只用这份）：
 * 每个文件保留最近一次的 patch 与累计改动量，`reverted` 只有在"该文件所有卡片都撤销了"时才为真。
 * `run_command` 里的改动不在此列。
 */
export function deriveThreadFileChanges(items: Item[]): FileChange[] {
  const byPath = new Map<string, FileChange>()

  const accumulate = (path: string, patch: string | undefined, item: Extract<Item, { kind: 'tool' }>): void => {
    if (!path) return
    const stats = patch ? patchStats(patch) : { added: 0, removed: 0 }
    const existing = byPath.get(path)
    if (existing) {
      existing.editsCount += 1
      existing.additions += stats.added
      existing.deletions += stats.removed
      existing.reverted = existing.reverted && Boolean(item.reverted)
      if (!existing.cardIds.includes(item.id)) existing.cardIds.push(item.id)
      if (patch) existing.latestPatch = patch
      return
    }
    byPath.set(path, {
      path,
      latestPatch: patch ?? '',
      additions: stats.added,
      deletions: stats.removed,
      editsCount: 1,
      reverted: Boolean(item.reverted),
      cardIds: [item.id],
    })
  }

  for (const item of items) {
    if (item.kind !== 'tool') continue
    if (item.name !== 'write_file' && item.name !== 'edit_file' && item.name !== 'edit_files') continue
    if (item.status !== 'done' && item.status !== 'error') continue

    if (item.name === 'edit_files') {
      const files = Array.isArray((item.details as { files?: unknown } | undefined)?.files)
        ? ((item.details as { files: Array<{ path?: unknown; patch?: unknown }> }).files)
        : []
      if (files.length > 0) {
        for (const entry of files) {
          accumulate(String(entry?.path ?? '').replace(/\\/g, '/'), entry?.patch as string | undefined, item)
        }
        continue
      }
      // 老流水没有 details.files：退回按参数里的路径记账，至少不丢文件
      for (const relative of pathsFromArgs(item.name, item.args)) {
        accumulate(relative.replace(/\\/g, '/'), undefined, item)
      }
      continue
    }

    const path = String((item.args as { path?: unknown } | undefined)?.path ?? '').replace(/\\/g, '/')
    accumulate(path, item.patch, item)
  }

  return [...byPath.values()]
}

/** 待保留的改动文件数（`reverted` 的不算）。 */
export function deriveThreadChangeCount(items: Item[]): number {
  return deriveThreadFileChanges(items).filter((change) => !change.reverted).length
}

/**
 * 从工具参数里取"这次调用动了哪些文件"。
 *
 * 只覆盖写工具的三条形状（主机侧的 `checkpointPathsOf` 还认识别的工具，但那些不进改动审阅）；
 * 刻意不 import 主机的检查点模块——那是实现，客户端不该依赖。
 */
function pathsFromArgs(toolName: string, args: Record<string, unknown> | undefined): string[] {
  if (!args) return []
  if (toolName === 'write_file' || toolName === 'edit_file') {
    const path = args.path
    return typeof path === 'string' && path ? [path] : []
  }
  const files = args.files
  if (Array.isArray(files)) {
    return files
      .map((entry) => (typeof entry === 'string' ? entry : String((entry as { path?: unknown })?.path ?? '')))
      .filter((path) => path.length > 0)
  }
  return []
}
