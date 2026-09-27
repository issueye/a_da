/**
 * 进程树清理：杀掉一个 shell 及它派生的所有子进程。
 *
 * `child.kill()` 只杀 shell 本身——cmd 会把后台子进程丢给系统变成孤儿，
 * 超时的构建脚本、被 Ctrl+C 的 dev server 都会继续活着占着端口。这里按平台
 * 补齐：Windows 用 taskkill /T /F，POSIX 用进程组信号（要求 spawn 时
 * detached: true，让 shell 成为组长）。
 */

import { spawn } from 'node:child_process'

export interface TreeKillTarget {
  pid: number | undefined
  platform?: NodeJS.Platform
}

/** 宽限 SIGTERM 之后强制 SIGKILL 的等待（仅 POSIX 需要）。 */
const SIGKILL_GRACE_MS = 2000

/**
 * 杀掉整棵进程树。返回值只表示清理动作已发出；子进程是否真的退出由调用方
 * 通过 close 事件判断。
 */
export function killProcessTree(target: TreeKillTarget): void {
  const platform = target.platform ?? process.platform
  const pid = target.pid
  if (!pid) return

  if (platform === 'win32') {
    // /T 连同子进程一起杀，/F 强制终止（不经过 WM_CLOSE，控制台程序不会弹确认）
    try {
      spawn('taskkill', ['/pid', String(pid), '/T', '/F'], {
        windowsHide: true,
        stdio: 'ignore',
      }).on('error', () => {})
    } catch {}
    return
  }

  // POSIX：shell 以 detached 方式成为进程组长（pgid == pid），
  // 负数 PID 的信号投递给整组。TERM 先给个优雅退出的机会。
  try {
    process.kill(-pid, 'SIGTERM')
  } catch {
    try {
      process.kill(pid, 'SIGKILL')
    } catch {}
    return
  }
  setTimeout(() => {
    try {
      // 0 探测进程是否还活着；ESRCH 说明已经退出了
      process.kill(-pid, 0)
      process.kill(-pid, 'SIGKILL')
    } catch {}
  }, SIGKILL_GRACE_MS).unref?.()
}
