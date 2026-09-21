/**
 * 跨平台剪贴板复制工具
 * 优先使用浏览器标准 navigator.clipboard，桌面环境回退至系统原生命令
 */

export async function copyToClipboard(text: string): Promise<boolean> {
  if (!text) return false

  // 1. Web / 浏览器标准 API
  if (typeof navigator !== 'undefined' && navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      // 权限受限或非安全上下文时向下回退
    }
  }

  // 2. 原生桌面回退 (Bun.spawn)
  if (typeof Bun !== 'undefined' && typeof Bun.spawn === 'function') {
    try {
      if (process.platform === 'win32') {
        const proc = Bun.spawn(['clip'], { stdin: 'pipe' })
        proc.stdin.write(text)
        proc.stdin.end()
        await proc.exited
        return true
      } else if (process.platform === 'darwin') {
        const proc = Bun.spawn(['pbcopy'], { stdin: 'pipe' })
        proc.stdin.write(text)
        proc.stdin.end()
        await proc.exited
        return true
      } else if (process.platform === 'linux') {
        const proc = Bun.spawn(['xclip', '-selection', 'clipboard'], { stdin: 'pipe' })
        proc.stdin.write(text)
        proc.stdin.end()
        await proc.exited
        return true
      }
    } catch {
      // 容错
    }
  }

  return false
}
