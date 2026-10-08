/**
 * 文本截断与保护工具
 * 参考 @earendil-works/pi-coding-agent/src/core/tools/truncate.ts
 * 防止大工具输出撑爆模型的上下文窗口 (Context Window)
 */

export const DEFAULT_MAX_LINES = 1000
export const DEFAULT_MAX_BYTES = 50 * 1024 // 50KB

export interface TruncationResult {
  content: string
  truncated: boolean
  totalLines: number
  totalBytes: number
  outputLines: number
  outputBytes: number
  truncationReason?: 'lines' | 'bytes'
}

export function formatByteSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

/**
 * 保留前部，截断尾部并附带截断警告说明
 */
export function truncateTail(
  text: string,
  maxLines: number = DEFAULT_MAX_LINES,
  maxBytes: number = DEFAULT_MAX_BYTES
): TruncationResult {
  const totalBytes = Buffer.byteLength(text, 'utf-8')
  const lines = text.split('\n')
  const totalLines = lines.length

  if (totalLines <= maxLines && totalBytes <= maxBytes) {
    return {
      content: text,
      truncated: false,
      totalLines,
      totalBytes,
      outputLines: totalLines,
      outputBytes: totalBytes,
    }
  }

  let keptLines: string[] = []
  let accumulatedBytes = 0
  let reason: 'lines' | 'bytes' = 'lines'

  for (let i = 0; i < lines.length; i++) {
    if (keptLines.length >= maxLines) {
      reason = 'lines'
      break
    }
    const line = lines[i]!
    const lineBytes = Buffer.byteLength(line, 'utf-8') + (keptLines.length > 0 ? 1 : 0)
    if (accumulatedBytes + lineBytes > maxBytes) {
      reason = 'bytes'
      break
    }
    keptLines.push(line)
    accumulatedBytes += lineBytes
  }

  const omittedLines = totalLines - keptLines.length
  const notice = `\n... [输出已截断：总共 ${totalLines} 行 (${formatByteSize(totalBytes)})，仅显示前 ${keptLines.length} 行。省略了 ${omittedLines} 行。] ...`
  const result = keptLines.join('\n') + notice

  return {
    content: result,
    truncated: true,
    totalLines,
    totalBytes,
    outputLines: keptLines.length,
    outputBytes: Buffer.byteLength(result, 'utf-8'),
    truncationReason: reason,
  }
}
