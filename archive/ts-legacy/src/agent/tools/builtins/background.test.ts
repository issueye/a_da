/**
 * 后台任务工具的集成测试：真的启动命令、轮询输出、终止进程树。
 * 与 run_command 的超时测试一样，Windows 的孙进程命令走 base64 编码绕开引号问题。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdtempSync, statSync } from "node:fs"
import { mkdtemp as mkdtempAsync, rm as rmAsync } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import {
  createCheckTaskTool,
  createKillTaskTool,
  createRunBackgroundTool,
  defaultBackgroundTasks,
  type BackgroundTask,
} from './background'

const isWin = process.platform === 'win32'

let ws = ''

beforeAll(async () => {
  ws = await mkdtempAsync(join(tmpdir(), 'a-da-bg-'))
})

afterAll(async () => {
  defaultBackgroundTasks.dispose()
  // Windows 上进程 cwd 句柄的释放是异步的，rm 失败就再等一轮
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      await rmAsync(ws, { recursive: true, force: true })
      return
    } catch {
      await sleep(500)
    }
  }
})

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

async function waitFor(predicate: () => boolean, timeoutMs = 8000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (predicate()) return
    await sleep(100)
  }
}

describe('后台任务工具', () => {
  test('run_background 立即返回任务 id，check_task 能查到它', async () => {
    const run = createRunBackgroundTool(ws)
    const check = createCheckTaskTool()

    const started = Date.now()
    const launch = await run.execute('call_1', { command: 'echo hello-from-bg' })
    // 启动即返回，不等命令执行完
    expect(Date.now() - started).toBeLessThan(3000)
    expect(launch.ok).toBe(true)
    const taskId = String((launch.details as { taskId?: string }).taskId)
    expect(taskId).toMatch(/^bg_/)

    const listed = await check.execute('call_2', {})
    expect(listed.ok).toBe(true)
    expect(listed.output).toContain(taskId)

    await waitFor(() => defaultBackgroundTasks.get(taskId)?.output.includes('hello-from-bg') === true)
    const detail = await check.execute('call_3', { task_id: taskId })
    expect(detail.ok).toBe(true)
    expect(detail.output).toContain(taskId)
    expect(detail.output).toContain('hello-from-bg')
  }, 15000)

  test('后台命令的输出会累积，完成后状态变为已退出', async () => {
    const run = createRunBackgroundTool(ws)
    const check = createCheckTaskTool()
    const kill = createKillTaskTool()

    const task: BackgroundTask = defaultBackgroundTasks.start(
      ws,
      isWin ? 'echo line-one && echo line-two' : 'echo line-one; echo line-two'
    )

    await waitFor(() => task.output.includes('line-two'))
    const result = await check.execute('call_1', { task_id: task.id, lines: 10 })
    expect(result.ok).toBe(true)
    expect(result.output).toContain('line-one')
    expect(result.output).toContain('line-two')

    await waitFor(() => task.status !== 'running', 10000)
    expect(task.status).toBe('completed')

    // 终止已结束的任务：提示无需终止
    const lateKill = await kill.execute('call_2', { task_id: task.id })
    expect(lateKill.ok).toBe(true)
    expect(lateKill.output).toContain('已经结束')
  }, 15000)

  test('kill_task 终止运行中的任务（孙进程一起死）', async () => {
    const dir = mkdtempSync(join(tmpdir(), "a-da-bgkill-"))
    try {
      const alive = join(dir, 'alive.txt')
      let command: string
      if (isWin) {
        const script = `for(;;){ Add-Content -Path '${alive}' -Value tick; Start-Sleep -Milliseconds 100 }`
        command = `powershell -NoProfile -EncodedCommand ${Buffer.from(script, 'utf16le').toString('base64')}`
      } else {
        command = `while true; do echo tick >> '${alive}'; sleep 0.1; done`
      }

      const task = defaultBackgroundTasks.start(ws, command)
      const deadline = Date.now() + 8000
      while (Date.now() < deadline) {
        try {
          if (statSync(alive).size > 0) break
        } catch {}
        await sleep(100)
      }
      expect(statSync(alive).size).toBeGreaterThan(0)

      const kill = createKillTaskTool()
      const result = await kill.execute('call_1', { task_id: task.id })
      expect(result.ok).toBe(true)
      expect(result.output).toContain('已终止')

      await sleep(1500)
      const settled = statSync(alive).size
      await sleep(1200)
      expect(statSync(alive).size).toBe(settled)
    } finally {
      await rmAsync(dir, { recursive: true, force: true }).catch(() => {})
    }
  }, 20000)

  test('check_task 不带 task_id 列出全部任务；未知 id 报错', async () => {
    const check = createCheckTaskTool()
    const task = defaultBackgroundTasks.start(ws, isWin ? 'echo x' : 'echo x')
    const listed = await check.execute('call_1', {})
    expect(listed.ok).toBe(true)
    expect(listed.output).toContain(task.id)

    const missing = await check.execute('call_2', { task_id: 'bg_not_exist' })
    expect(missing.ok).toBe(false)
    expect(missing.output).toContain('不存在')

    const kill = createKillTaskTool()
    const missingKill = await kill.execute('call_3', { task_id: 'bg_not_exist' })
    expect(missingKill.ok).toBe(false)
  }, 15000)

  test('cwd 越过工作区被拒绝', async () => {
    const run = createRunBackgroundTool(ws)
    const result = await run.execute('call_1', { command: 'echo hi', cwd: '..' })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('工作区')
  })
})
