/**
 * 进程树清理的回归测试。
 *
 * 只杀 shell 会留下孤儿：cmd 被杀后，它派生的 powershell 还在继续跑。
 * 这条测试用「子进程持续写文件」来钉住整棵树必须一起死——文件不再增长，
 * 才说明孙进程真的被清掉了。
 *
 * Windows 的孙进程命令一律走 -EncodedCommand（base64）：Bun 在 Windows 拼
 * spawn 命令行时会重写引号与反斜杠，明文 PS 脚本里的路径和 $true 都会被搅碎。
 */

import { describe, expect, test } from 'bun:test'
import { spawn } from 'node:child_process'
import { mkdtemp, rm } from 'node:fs/promises'
import { statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { killProcessTree } from './proc'
import { createBashTool } from './builtins/bash'

const isWin = process.platform === 'win32'

/** 轮询等文件出现并开始增长；超时返回 -1。 */
function waitForGrowth(path: string, timeoutMs: number): Promise<number> {
  const deadline = Date.now() + timeoutMs
  return new Promise((resolve) => {
    const poll = (): void => {
      try {
        const size = statSync(path).size
        if (size > 0) return resolve(size)
      } catch {}
      if (Date.now() > deadline) return resolve(-1)
      setTimeout(poll, 100)
    }
    poll()
  })
}

function sampleSize(path: string): number {
  try {
    return statSync(path).size
  } catch {
    return -1
  }
}

/** 宽限后删除临时目录：Windows 上进程句柄释放是异步的，rm 失败就再等一轮。 */
async function rmTolerant(dir: string): Promise<void> {
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      await rm(dir, { recursive: true, force: true })
      return
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 500))
    }
  }
}

describe('killProcessTree', () => {
  test('shell 被杀时孙进程一起被清掉（文件停止增长）', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'a-da-proc-'))
    const alive = join(dir, 'alive.txt')
    let command: string
    if (isWin) {
      const script = `for(;;){ Add-Content -Path '${alive}' -Value tick; Start-Sleep -Milliseconds 100 }`
      const encoded = Buffer.from(script, 'utf16le').toString('base64')
      command = `powershell -NoProfile -EncodedCommand ${encoded}`
    } else {
      command = `while true; do echo tick >> '${alive}'; sleep 0.1; done`
    }

    const child = spawn(
      isWin ? process.env.COMSPEC || 'cmd.exe' : '/bin/sh',
      isWin ? ['/d', '/s', '/c', command] : ['-c', command],
      { windowsHide: true, detached: !isWin || undefined }
    )

    try {
      const grown = await waitForGrowth(alive, 8000)
      expect(grown).toBeGreaterThan(0)

      killProcessTree({ pid: child.pid })
      await new Promise((resolve) => setTimeout(resolve, 1500))

      const settled = sampleSize(alive)
      expect(settled).toBeGreaterThan(0)
      await new Promise((resolve) => setTimeout(resolve, 1200))
      expect(sampleSize(alive)).toBe(settled)
    } finally {
      killProcessTree({ pid: child.pid })
      child.removeAllListeners?.()
      // taskkill 是异步发出终止，稍等句柄释放再删目录
      await new Promise((resolve) => setTimeout(resolve, 400))
      await rmTolerant(dir)
    }
  }, 25000)

  test('run_command 超时后快速返回超时错误', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'a-da-bash-'))
    const tool = createBashTool(dir)
    const started = Date.now()
    const result = await tool.execute('call_test', {
      command: isWin ? 'ping -n 8 127.0.0.1 > nul' : 'sleep 8',
      timeout: 1,
    })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('超时')
    expect(Date.now() - started).toBeLessThan(5000)
    await rmTolerant(dir)
  }, 15000)
})
