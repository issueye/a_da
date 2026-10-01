/**
 * 主机自举（M3-4 / 协议 §1.8）：UI 角色把**自己**再起一次当主机，然后照常连它。
 *
 * ## 为什么要 spawn 自己
 *
 * 交付物只有一个二进制（§0.2 第 4 条）。所以"主机"不是另装的东西，就是同一个 exe 带 `--host`
 * 再跑一次。开发态（`bun app.tsx`）下 `process.execPath` 是 bun，于是要把入口脚本也带上；
 * 打包后 `process.execPath` 就是 exe 本身，只带参数即可——这段差异只在这一个函数里。
 *
 * ## 端口与令牌
 *
 * - 端口用 `--port 0` 让系统分配：避免固定端口冲突（协议 §1.8）。
 *   真实端口靠子进程 stdout 的**就绪行**回传，就绪行不承载协议消息。
 * - 令牌由这里生成、经命令行传给子进程：只在本机回环上用，不落盘。
 * - `--parent-pid` 让主机自己盯着我们：UI 被强杀时它能自杀，不留孤儿。
 *
 * ## 失败要如实
 *
 * 子进程在就绪前退出（参数错、端口被占、bun 起不来）→ 带上 stderr 的尾巴一起报错，
 * 而不是让 UI 干等到超时。
 */

import { spawn, type ChildProcess } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { hostEntryArgs, parseReadyLine } from '../../agent/host/main'
import { log } from './logging'

export interface HostProcess {
  pid: number
  port: number
  token: string
  /** 主机的 WebSocket 地址（带令牌）。 */
  url: string
  /** 结束主机：先礼貌（SIGTERM），超时再强杀。 */
  stop(): void
  /** 主机是否还活着。 */
  readonly alive: boolean
}

export interface SpawnHostOptions {
  /** 就绪超时（ms），默认 10s。 */
  timeoutMs?: number
  /** 起进程的命令（默认：自己在跑的运行时/可执行文件）。测试里可以换成 bun。 */
  execPath?: string
  /** 打包形态（true）还是 `bun <入口脚本>` 形态（false）。默认按 execPath 名字猜。 */
  compiled?: boolean
  /** 入口脚本路径（非打包形态必须给）。默认从本文件位置推出仓库根的 `app.tsx`。 */
  entryScript?: string
}

/**
 * 应用入口（非打包形态要带上它）。
 *
 * **不能用 `Bun.main`**：`bun test` 下它是**测试文件**，于是"spawn 自己当主机"会把测试文件
 * 当入口跑起来（实测就是这样炸的：`Cannot use afterEach() outside of the test runner`）。
 * 从本文件位置推更稳：`src/ui/client/` 往上三层就是仓库根。
 */
function defaultEntryScript(): string {
  return join(import.meta.dir, '..', '..', '..', 'app.tsx')
}

/**
 * 解析默认的主机执行源。
 *
 * 优先顺序：
 * 1. 环境变量 A_DA_CORE_PATH；
 * 2. 编译好的原生 Rust 核心（优先 release，其次 debug）；
 * 3. 兜底回退：当前运行时（Bun 或打包 exe）+ app.tsx。
 */
export function resolveDefaultHostRunner(): {
  execPath: string
  compiled: boolean
  entryScript?: string
} {
  // 1. 显式指定原生 Rust 核心（环境变量 A_DA_CORE_PATH 或 A_DA_USE_RUST_CORE=1）
  if (process.env.A_DA_CORE_PATH && existsSync(process.env.A_DA_CORE_PATH)) {
    return { execPath: process.env.A_DA_CORE_PATH, compiled: true }
  }

  if (process.env.A_DA_USE_RUST_CORE === '1') {
    const appDir = dirname(process.execPath)
    const cwd = process.cwd()
    const repoRoot = join(import.meta.dir, '..', '..', '..')
    const ext = process.platform === 'win32' ? '.exe' : ''
    const exeName = `agent_core${ext}`

    const candidates = [
      join(appDir, exeName),
      join(cwd, 'dist', exeName),
      join(cwd, exeName),
      join(repoRoot, 'agent_core', 'target', 'release', exeName),
      join(repoRoot, 'dist', exeName),
      join(repoRoot, 'agent_core', 'target', 'debug', exeName),
    ]

    for (const candidate of candidates) {
      if (existsSync(candidate)) {
        return { execPath: candidate, compiled: true }
      }
    }
  }

  // 2. 默认：单文件双角色标准契约（协议 §1.8：UI 角色自举起自身带 --host，无缝继承全部配置与完整插件流式）
  const compiled = !/(^|[\\/])bun(\.exe)?$/i.test(process.execPath)
  return {
    execPath: process.execPath,
    compiled,
    entryScript: compiled ? undefined : defaultEntryScript(),
  }
}

/** 本机回环的一次性令牌。 */
export function makeHostToken(): string {
  return randomBytes(32).toString('hex')
}

/**
 * 起一个主机进程并等它就绪。
 *
 * 刻意不在这里连 WebSocket：自举只负责"把主机拉起来、拿到端口与令牌"，
 * 连接由 `ui/client/ws.ts` 负责——两件事分开，哪一步坏了就报哪一步。
 */
export async function spawnHostProcess(options: SpawnHostOptions = {}): Promise<HostProcess> {
  const timeoutMs = options.timeoutMs ?? 10_000
  const token = makeHostToken()
  const defaultRunner = resolveDefaultHostRunner()
  const execPath = options.execPath ?? defaultRunner.execPath
  const compiled = options.compiled ?? (options.execPath ? !/(^|[\\/])bun(\.exe)?$/i.test(execPath) : defaultRunner.compiled)
  const entryScript = options.entryScript ?? (compiled ? undefined : defaultRunner.entryScript)

  const args = [
    ...(compiled ? [] : entryScript ? [entryScript] : []),
    ...hostEntryArgs({ port: 0, token, parentPid: process.pid }),
  ]

  const child = spawn(execPath, args, {
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
    // 主机不该继承 UI 的那套环境变量开关（例如 A_DA_TRANSPORT，避免它自己也去 spawn）
    env: { ...process.env, A_DA_TRANSPORT: 'inprocess' },
  })

  /**
   * 主机的输出一律落进应用日志。
   *
   * 为什么必须做：主机的 stdout/stderr 以前只用来读就绪行，**之后就丢掉了**——
   * 于是"插件加载失败：Cannot find module '../dist/babel.cjs'"这类只在主机侧出现的错误，
   * 用户在界面上什么都看不到，只能看到某个命令超时（现象与原因隔了一层）。
   * 打包形态是 GUI 子系统、没有控制台，日志文件是唯一能留下现场的地方。
   */
  const stderrTail: string[] = []
  const logHostLine = (line: string): void => {
    const text = line.trim()
    if (text) log(`[host] ${text}`)
  }
  child.stderr?.setEncoding('utf8')
  child.stderr?.on('data', (chunk: string) => {
    stderrTail.push(chunk)
    if (stderrTail.length > 20) stderrTail.shift()
    for (const line of chunk.split('\n')) logHostLine(line)
  })

  const ready = await new Promise<{ port: number; pid: number }>((resolve, reject) => {
    let settled = false
    const timer = setTimeout(() => {
      if (settled) return
      settled = true
      reject(new Error(`主机在 ${timeoutMs}ms 内没报就绪。stderr: ${stderrTail.join('').trim()}`))
    }, timeoutMs)

    const finish = (value: { port: number; pid: number } | Error): void => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      if (value instanceof Error) reject(value)
      else resolve(value)
    }

    child.stdout?.setEncoding('utf8')
    let buffer = ''
    child.stdout?.on('data', (chunk: string) => {
      buffer += chunk
      let newline = buffer.indexOf('\n')
      while (newline >= 0) {
        const line = buffer.slice(0, newline)
        buffer = buffer.slice(newline + 1)
        const parsed = parseReadyLine(line)
        if (parsed) {
          finish(parsed)
        } else {
          // 就绪行之外的主机输出（含稳态后的 console 输出）全部留痕
          logHostLine(line)
        }
        newline = buffer.indexOf('\n')
      }
    })

    child.on('error', (err) => finish(err))
    child.on('exit', (code, signal) => {
      // 就绪**之后**的退出不是"启动失败"，而是"主机没了"——照实记一笔，
      // 界面那边有连接状态横幅，日志这边留现场（否则只能看到命令超时）。
      if (settled) {
        log(`[host] 主机退出（code=${String(code)}, signal=${String(signal)}）`)
        return
      }
      finish(
        new Error(
          `主机在就绪前退出（code=${String(code)}, signal=${String(signal)}）。stderr: ${stderrTail
            .join('')
            .trim()}`
        )
      )
    })
  })

  const url = `ws://127.0.0.1:${ready.port}/rpc`
  return {
    pid: ready.pid || child.pid || 0,
    port: ready.port,
    token,
    url,
    stop: () => stopChild(child),
    get alive() {
      return child.exitCode === null && !child.killed
    },
  }
}

/** 先 SIGTERM，给它一点时间自己收尾；超时再强杀（避免 UI 退出被卡住）。 */
function stopChild(child: ChildProcess): void {
  if (child.exitCode !== null || child.killed) return
  try {
    if (process.platform === 'win32') child.kill()
    else child.kill('SIGTERM')
  } catch {
    // 已经没了
  }
  setTimeout(() => {
    try {
      if (child.exitCode === null) child.kill('SIGKILL')
    } catch {
      // 已经没了
    }
  }, 1500).unref?.()
}
