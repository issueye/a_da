export function formatDuration(ms?: number): string {
  if (ms === undefined || ms <= 0) return ''
  if (ms < 1000) return `${(ms / 1000).toFixed(1)}s`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  const minutes = Math.floor(ms / 60_000)
  const seconds = Math.round((ms % 60_000) / 1000)
  return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`
}

/** 格式化 Token 简写 */
export function formatTokenShort(n: number): string {
  if (n < 1000) return String(n)
  if (n < 10_000) return `${(n / 1000).toFixed(1)}k`
  if (n < 1_000_000) return `${Math.round(n / 1000)}k`
  const m = n / 1_000_000
  return m >= 10 ? `${Math.round(m)}M` : `${m.toFixed(1).replace(/\.0$/, '')}M`
}

/** 每个会话的独立滚动高度记忆，防止切换标签页时丢失阅读位置 */
