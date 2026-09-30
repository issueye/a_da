/**
 * 主机角色的入口（协议 §1.8 的 `--host`）：**不开窗口**，只跑 agent 并把协议挂在回环端口上。
 *
 * 它是同一个二进制里的另一个角色——`app.tsx` 按 argv 分流到这里，所以交付物仍然只有一个 exe。
 *
 * ## 命令行
 *
 * ```
 * a-da.exe --host --port 0 --token <一次性令牌> [--parent-pid <UI 的 pid>]
 * ```
 *
 * - `--port 0`：让操作系统挑空闲端口，避免固定端口冲突；
 * - `--token`：由 UI 生成并只在命令行上传一次（不落盘）；
 * - `--parent-pid`：**父进程看门狗**。UI 被强杀（任务管理器）时收不到任何信号，
 *   所以主机自己每两秒确认一次父进程还在；不在就自杀，避免孤儿进程。
 *   `stdin` 关闭是第二道（正常退出时 UI 会关管道）。
 *
 * ## 就绪行
 *
 * 绑好端口后往 stdout 打**一行**机器可读的就绪行，UI 读它拿到真实端口。
 * 就绪行不承载任何协议消息（协议 §1.8 明确），它只是"我起来了 + 端口是多少"。
 */

import { PROTOCOL_VERSION } from '../../shared/protocol'
import { startHostServer, type HostServer } from './server'

export interface HostMainOptions {
  port: number
  token: string
  /** UI 进程的 pid；给了就启用父进程看门狗。 */
  parentPid?: number
  /** 看门狗检查间隔（ms），默认 2000。测试里可以调小。 */
  watchdogIntervalMs?: number
}

/**
 * 造主机角色的 argv。
 *
 * 与 {@link parseHostArgs} 放在同一个文件里：**写与读必须成对**，否则某天加一个参数，
 * 只有一侧改了，症状是"主机起来了但某个开关没生效"这类难查的静默失效。
 * 两边的往返由 `main.test.ts` 钉住。
 */
export function hostEntryArgs(options: HostMainOptions): string[] {
  const args = ['--host', '--port', String(options.port), '--token', options.token]
  if (options.parentPid !== undefined) args.push('--parent-pid', String(options.parentPid))
  return args
}

/** 解析主机角色的 argv（纯函数，单独可测）。 */export function parseHostArgs(argv: readonly string[]): HostMainOptions {
  let port = 0
  let token: string | undefined
  let parentPid: number | undefined

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]
    if (arg === '--host') continue
    if (arg === '--port') {
      const raw = argv[++i]
      const parsed = Number(raw)
      if (!Number.isInteger(parsed) || parsed < 0 || parsed > 65535) {
        throw new Error(`--port 需要一个 0..65535 的整数，收到 ${String(raw)}`)
      }
      port = parsed
      continue
    }
    if (arg === '--token') {
      token = argv[++i]
      continue
    }
    if (arg === '--parent-pid') {
      const raw = argv[++i]
      const parsed = Number(raw)
      if (!Number.isInteger(parsed) || parsed <= 0) {
        throw new Error(`--parent-pid 需要一个正整数，收到 ${String(raw)}`)
      }
      parentPid = parsed
      continue
    }
    throw new Error(`主机角色不认识这个参数：${arg}`)
  }

  if (!token) throw new Error('--token 是必须的：本机回环也不能无令牌开放')
  return { port, token, parentPid }
}

/** 就绪行：UI 读它拿端口。前缀是为了在混杂输出里也能可靠扫到。 */
export function readyLine(port: number): string {
  return `A_DA_HOST_READY ${JSON.stringify({
    ready: true,
    port,
    pid: process.pid,
    protocolVersion: PROTOCOL_VERSION,
  })}`
}

/** 从一行输出里解析就绪行；不是就绪行就返回 null。 */
export function parseReadyLine(line: string): { port: number; pid: number } | null {
  const marker = 'A_DA_HOST_READY '
  const at = line.indexOf(marker)
  if (at < 0) return null
  try {
    const parsed = JSON.parse(line.slice(at + marker.length)) as { ready?: boolean; port?: number; pid?: number }
    if (parsed.ready !== true || typeof parsed.port !== 'number') return null
    return { port: parsed.port, pid: typeof parsed.pid === 'number' ? parsed.pid : 0 }
  } catch {
    return null
  }
}

/**
 * 主机进程入口：跑起来、打就绪行、挂看门狗，然后**一直不返回**（进程活到被要求退出）。
 *
 * store 由调用方注入：主机角色的 `app.tsx` 分支会 `import { store } from '../store'`，
 * 而测试可以直接传一个自己造的 store。
 */
export async function hostEntry(
  argv: readonly string[],
  deps: { store: Parameters<typeof startHostServer>[0]['store'] }
): Promise<{ server: HostServer; options: HostMainOptions }> {
  const options = parseHostArgs(argv)
  const server = startHostServer({ store: deps.store, token: options.token, port: options.port })

  // 就绪行必须是**一行**且尽早打出去：UI 在等它
  process.stdout.write(`${readyLine(server.port)}\n`)

  const timers: Array<ReturnType<typeof setInterval>> = []

  if (options.parentPid !== undefined) {
    const parentPid = options.parentPid
    const interval = setInterval(() => {
      try {
        // 信号 0 = 只做存在性检查，不发信号
        process.kill(parentPid, 0)
      } catch {
        console.error('[host] 父进程已消失，主机自行退出')
        stop(0)
      }
    }, options.watchdogIntervalMs ?? 2000)
    timers.push(interval)
  }

  // 第二道：UI 关掉管道（正常退出路径）也当作"该走了"
  process.stdin.on('end', () => stop(0))
  process.stdin.on('close', () => stop(0))
  process.stdin.resume()

  const onSignal = (): void => stop(0)
  process.on('SIGTERM', onSignal)
  process.on('SIGINT', onSignal)

  function stop(code: number): void {
    for (const timer of timers) clearInterval(timer)
    try {
      server.stop()
    } catch {
      // 关的时候出问题也不能卡住退出
    }
    process.exit(code)
  }

  return { server, options }
}
