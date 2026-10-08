/**
 * Prove the compiled binary is a real app — **两个角色都验**。
 *
 *   bun run build && bun scripts/binary-check.ts
 *
 * ## 为什么不用 `@gpuix/react/automation` 的 `launch()` 查 testId（重要）
 *
 * 打包后的 exe 是 **PE 子系统 2（无控制台）**，而 `src/platform/init.ts` 会把
 * `console.log` 劫持到 `A_DA_HOME/app_debug.log`，仅在"检测到控制台"时才同时写真实 stdout。
 * 自动化通道恰恰是走 `console.log('data: …')` 的，所以在打包产物里**响应到不了管道**，
 * `launch()` 会一直等下去（实测：进程明明画出了窗口、日志里能看到自动化请求的响应，
 * 客户端却永远收不到）。
 *
 * 这是**既有**行为，与"单文件双角色"无关。所以这里的机制是：
 * **以应用自己写的启动日志为证据 + 每一步都有硬超时 + 无论成败都收尾**。
 * 三条缺一不可，否则失败就变成"卡住"，而卡住是最难查的失败。
 *
 * 要窗口级的交互验证（点击/截图/查 testId），用开发态（`bun app.tsx`，有控制台，自动化通）
 * 或 `bun test` 里的真窗口用例——那才是它们的用武之地。
 */

import { existsSync, mkdirSync, readFileSync, rmSync } from 'node:fs'
import { randomBytes } from 'node:crypto'
import path from 'node:path'
import { spawn, type ChildProcess } from 'node:child_process'
import { parseReadyLine } from '../src/agent/host/main'

const root = path.join(import.meta.dir, '..')
const coreBinary = path.join(root, 'dist', 'a-da-core.exe')
const binary =
  process.platform === 'win32' && existsSync(coreBinary)
    ? coreBinary
    : path.join(root, 'dist', process.platform === 'win32' ? 'a-da.exe' : 'a-da')

const tmpDir = path.join(root, 'tmp')
mkdirSync(tmpDir, { recursive: true })

if (!existsSync(binary)) {
  console.error(`[binary-check] 找不到 ${path.relative(root, binary)}，先跑 bun run build`)
  process.exit(2)
}

/** 有界等待：条件不满足就抛，绝不无限等。 */
async function waitUntil(check: () => boolean, timeoutMs: number, what: string): Promise<void> {
  const started = Date.now()
  while (Date.now() - started < timeoutMs) {
    if (check()) return
    await sleep(100)
  }
  throw new Error(`等待「${what}」超过 ${timeoutMs}ms`)
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

/** 收尾：先请它自己走，超时再强杀；无论哪条路都**等到进程真的没了**才算完。 */
async function shutdown(child: ChildProcess, what: string): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return
  child.kill()
  const started = Date.now()
  while (child.exitCode === null && child.signalCode === null && Date.now() - started < 8000) {
    await sleep(100)
  }
  if (child.exitCode === null && child.signalCode === null) {
    child.kill('SIGKILL')
    await sleep(500)
    if (child.exitCode === null && child.signalCode === null) {
      throw new Error(`${what} 收尾失败：进程还在（pid=${child.pid}）`)
    }
  }
}

/**
 * 第一关：UI 角色（无参数启动）——真的画出首帧，且画完之后没有崩。
 *
 * 证据取自应用自己写的启动日志（`A_DA_HOME/app_debug.log`）：
 * `render() 初始化执行成功` + `mount complete` 两行说明窗口建好、React 树挂上了。
 */
async function checkUiRole(): Promise<void> {
  const home = path.join(tmpDir, 'binary-home')
  rmSync(home, { recursive: true, force: true })
  mkdirSync(home, { recursive: true })
  const logFile = path.join(home, 'app_debug.log')

  const child = spawn(binary, [], {
    cwd: root,
    env: { ...process.env, GPUIX_BACKGROUND: '1', A_DA_HOME: home },
    stdio: 'ignore',
  })

  try {
    const readLog = (): string => (existsSync(logFile) ? readFileSync(logFile, 'utf8') : '')
    await waitUntil(
      () => readLog().includes('render() 初始化执行成功') && readLog().includes('mount complete'),
      60_000,
      'UI 角色画出首帧'
    )
    // 画完再等一会儿，确认它不是"画完立刻崩"（那也算没起来）
    await sleep(1500)
    if (child.exitCode !== null) {
      throw new Error(`UI 角色画完首帧后退出（code=${child.exitCode}）\n${readLog().split('\n').slice(-8).join('\n')}`)
    }

    // **拆分真的在跑**：打包形态必须自己起了主机并走 WebSocket（协议 §1.8）。
    // 这一行是应用自己写的日志，比"看起来有窗口"更硬：它证明传输选择、自 spawn、
    // 握手与首帧快照都真的发生过。
    const logText = readLog()
    const transportLine = logText.split('\n').find((line) => line.includes('传输：')) ?? '(日志里没有传输那一行)'
    if (!transportLine.includes('传输：ws')) {
      throw new Error(
        `打包形态没有走 WebSocket 传输（期望日志里有「传输：ws（主机 pid=… 端口=…）」）。实际：${transportLine}\n` +
          `--- app_debug.log 尾部 ---\n${logText.trim().split('\n').slice(-10).join('\n')}`
      )
    }
    console.log(`[binary-check] UI 角色 OK：画出首帧、存活，且 ${transportLine.split('] ').pop()}`)
  } catch (err) {
    const tail = existsSync(logFile) ? readFileSync(logFile, 'utf8').trim().split('\n').slice(-10).join('\n') : '(没有日志)'
    throw new Error(`${(err as Error).message}\n--- app_debug.log 尾部 ---\n${tail}`)
  } finally {
    await shutdown(child, 'UI 角色')
  }
}

/**
 * 第二关：主机角色——同一个 exe 带 `--host` 再跑一次，报出端口、接受 WebSocket、
 * 给出快照。这条是"单文件 + 内部拆分"的可执行证明：不依赖 bun，也不依赖任何额外文件。
 */
async function checkHostRole(): Promise<void> {
  const home = path.join(tmpDir, 'binary-host-home')
  rmSync(home, { recursive: true, force: true })
  mkdirSync(home, { recursive: true })

  const token = randomBytes(32).toString('hex')
  const child = spawn(binary, ['--host', '--port', '0', '--token', token], {
    cwd: root,
    env: { ...process.env, A_DA_HOME: home },
    stdio: ['pipe', 'pipe', 'pipe'],
  })

  try {
    const stderr: string[] = []
    child.stderr?.setEncoding('utf8')
    child.stderr?.on('data', (chunk: string) => {
      stderr.push(chunk)
      if (stderr.length > 20) stderr.shift()
    })

    let buffer = ''
    let port = 0
    child.stdout?.setEncoding('utf8')
    child.stdout?.on('data', (chunk: string) => {
      buffer += chunk
      for (const line of buffer.split('\n')) {
        const ready = parseReadyLine(line)
        if (ready) port = ready.port
      }
    })

    await waitUntil(() => port > 0, 60_000, '主机角色报出就绪行')
    if (child.exitCode !== null) {
      throw new Error(`主机角色提前退出 code=${child.exitCode}。stderr: ${stderr.join('')}`)
    }

    // 连它：握手 + 一条命令 + 一份快照，三样都通才算过
    const outcome = await new Promise<string>((resolve, reject) => {
      const socket = new WebSocket(`ws://127.0.0.1:${port}/rpc?token=${encodeURIComponent(token)}`)
      const timer = setTimeout(() => reject(new Error('连上后 30s 内没拿到快照')), 30_000)
      let sawHello = false
      socket.addEventListener('open', () => {
        socket.send(
          JSON.stringify({
            jsonrpc: '2.0',
            id: 1,
            method: 'session.hello',
            params: { token, protocolVersion: '1.0' },
          })
        )
      })
      socket.addEventListener('message', (event) => {
        const frame = JSON.parse(String((event as MessageEvent).data)) as {
          id?: number
          method?: string
          result?: { protocolVersion?: string }
          params?: { payload?: { threads?: unknown[] } }
        }
        if (frame.id === 1 && frame.result?.protocolVersion) sawHello = true
        if (frame.method === 'evt.state.snapshot') {
          clearTimeout(timer)
          const threads = frame.params?.payload?.threads?.length ?? 0
          socket.close()
          resolve(`握手${sawHello ? '✓' : '（快照先到）'}、快照 ${threads} 个会话`)
        }
      })
      socket.addEventListener('error', () => {
        clearTimeout(timer)
        reject(new Error(`连不上 127.0.0.1:${port} 的 WebSocket`))
      })
    })

    console.log(`[binary-check] 主机角色 OK：--host → 端口 ${port}，${outcome}`)
  } finally {
    await shutdown(child, '主机角色')
  }
}

await checkUiRole()
await checkHostRole()
console.log('[binary-check] 单文件双角色均通过')
