/**
 * Line diffs for the file-editing tools.
 *
 * The renderer's `<diff>` element wants a unified patch, so an edit has to
 * produce one. A plain LCS over lines is enough here: files an agent edits are
 * small, and the DP is guarded so a huge file falls back to one replace hunk
 * instead of allocating a table that big.
 */

export interface DiffOp {
  type: 'equal' | 'del' | 'ins'
  text: string
}

const CONTEXT = 3
const MAX_CELLS = 4_000_000

function lcsOps(before: string[], after: string[]): DiffOp[] {
  const n = before.length
  const m = after.length
  if (n * m > MAX_CELLS) {
    return [
      ...before.map<DiffOp>((text) => ({ type: 'del', text })),
      ...after.map<DiffOp>((text) => ({ type: 'ins', text })),
    ]
  }
  // lengths[i][j] = LCS length of before[i..] and after[j..]
  const lengths: Int32Array[] = []
  for (let i = 0; i <= n; i++) lengths.push(new Int32Array(m + 1))
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lengths[i]![j] =
        before[i] === after[j]
          ? lengths[i + 1]![j + 1]! + 1
          : Math.max(lengths[i + 1]![j]!, lengths[i]![j + 1]!)
    }
  }
  const ops: DiffOp[] = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (before[i] === after[j]) {
      ops.push({ type: 'equal', text: before[i]! })
      i++
      j++
    } else if (lengths[i + 1]![j]! >= lengths[i]![j + 1]!) {
      ops.push({ type: 'del', text: before[i]! })
      i++
    } else {
      ops.push({ type: 'ins', text: after[j]! })
      j++
    }
  }
  while (i < n) ops.push({ type: 'del', text: before[i++]! })
  while (j < m) ops.push({ type: 'ins', text: after[j++]! })
  return ops
}

/** A unified patch that `<diff patch={..}>` renders, or `''` when nothing changed. */
export function unifiedPatch(filePath: string, before: string, after: string): string {
  if (before === after) return ''
  const a = before.length ? before.replace(/\n$/, '').split('\n') : []
  const b = after.length ? after.replace(/\n$/, '').split('\n') : []
  const ops = lcsOps(a, b)

  // Group the ops into hunks that carry CONTEXT lines around every change.
  const changed = ops
    .map((op, index) => (op.type === 'equal' ? -1 : index))
    .filter((index) => index >= 0)
  const ranges: { start: number; end: number }[] = []
  for (const index of changed) {
    const start = Math.max(0, index - CONTEXT)
    const end = Math.min(ops.length, index + CONTEXT + 1)
    const last = ranges[ranges.length - 1]
    if (last && start <= last.end) last.end = Math.max(last.end, end)
    else ranges.push({ start, end })
  }

  const lines: string[] = [`--- a/${filePath}`, `+++ b/${filePath}`]
  // Running line numbers, walked once across all hunks.
  let oldLine = 1
  let newLine = 1
  let cursor = 0
  for (const range of ranges) {
    for (; cursor < range.start; cursor++) {
      const op = ops[cursor]!
      if (op.type !== 'ins') oldLine++
      if (op.type !== 'del') newLine++
    }
    const slice = ops.slice(range.start, range.end)
    const oldCount = slice.filter((op) => op.type !== 'ins').length
    const newCount = slice.filter((op) => op.type !== 'del').length
    lines.push(`@@ -${oldLine},${oldCount} +${newLine},${newCount} @@`)
    for (const op of slice) {
      lines.push(`${op.type === 'equal' ? ' ' : op.type === 'del' ? '-' : '+'}${op.text}`)
      if (op.type !== 'ins') oldLine++
      if (op.type !== 'del') newLine++
    }
    cursor = range.end
  }
  return `${lines.join('\n')}\n`
}

/** `2 files changed, 14 insertions(+), 3 deletions(-)` in the tool card header. */
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
