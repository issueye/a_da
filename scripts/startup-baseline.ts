/**
 * 冷启动基线（B2）：测"进程起来 → 首帧挂上"用了多久，两种传输各测几次。
 *
 *   bun scripts/startup-baseline.ts            # 各测 3 次
 *   bun scripts/startup-baseline.ts 5          # 各测 5 次
 *
 * ## 为什么用应用自己的日志当"首帧"的信号
 *
 * 与 `binary-check.ts` 同一个理由：打包产物是 GUI 子系统（无控制台），`console.log` 被
 * `src/platform/init.ts` 劫持进 `A_DA_HOME/app_debug.log`，自动化通道到不了管道
 * （详见 `docs/agent-conventions.md` §17）。日志里的 `mount complete` 是原生层挂载完成的
 * 那一刻，正是"首帧已经画出来"的可靠证据。
 *
 * ## 测的是哪两件事
 *
 * - `inprocess`：`bun app.tsx`（开发/测试形态，不起主机进程）——这是 M0–M2 的基线；
 * - `ws`：`dist/a-da.exe`（打包形态，自己 spawn 主机 + WebSocket + 首帧快照）——这是 M3 的新形态。
 *
 * 每一步都有硬超时，失败就报错并收尾，不留进程、不无限等。
 */

import { existsSync, mkdirSync, readFileSync, rmSync } from 'node:fs'
import path from 'node:path'
import { spawn, type ChildProcess } from 'node:child_process'

const root = path.join(import.meta.dir, '..')
const runs = Number(process.argv[2] ?? 3)
const tmpDir = path.join(root, 'tmp', 'startup-baseline')
mkdirSync(tmpDir, { recursive: true })

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

async function until(check: () => boolean, timeoutMs: number, what: string): Promise<void> {
  const started = Date.now()
  while (Date.now() - started < timeoutMs) {
    if (check()) return
    await sleep(20)
  }
  throw new Error(`等待「${what}」超过 ${timeoutMs}ms`)
}

/** 收尾：请它走，超时强杀；直到进程真的没了才算完。 */
async function shutdown(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return
  child.kill()
  const started = Date.now()
  while (child.exitCode === null && child.signalCode === null && Date.now() - started < 8000) {
    await sleep(50)
  }
  if (child.exitCode === null && child.signalCode === null) {
    child.kill('SIGKILL')
    await sleep(300)
  }
}

/** 跑一次，返回"spawn → mount complete"的毫秒数。 */
async function once(
  label: string,
  command: string,
  args: string[],
  extraEnv: Record<string, string> = {}
): Promise<number> {
  const home = path.join(tmpDir, `home-${label}-${Date.now()}`)
  rmSync(home, { recursive: true, force: true })
  mkdirSync(home, { recursive: true })
  const logFile = path.join(home, 'app_debug.log')

  const started = Date.now()
  const child = spawn(command, args, {
    cwd: root,
    env: { ...process.env, GPUIX_BACKGROUND: '1', A_DA_HOME: home, ...extraEnv },
    stdio: 'ignore',
  })

  try {
    const readLog = (): string => (existsSync(logFile) ? readFileSync(logFile, 'utf8') : '')
    await until(
      () => readLog().includes('render() 初始化执行成功') && readLog().includes('mount complete'),
      60_000,
      '首帧'
    )
    const elapsed = Date.now() - started
    // 记录一下走的是哪条传输（ws 形态应当在这一行里看到主机 pid/端口）
    const transportLine = readLog().split('\n').find((line) => line.includes('传输：')) ?? '(无传输行)'
    console.log(`  ${label} #${' '.repeat(1)}${elapsed}ms   ${transportLine.split('] ').pop()}`)
    return elapsed
  } finally {
    await shutdown(child)
  }
}

function summarize(label: string, samples: number[]): void {
  const sorted = [...samples].sort((a, b) => a - b)
  const median = sorted[Math.floor(sorted.length / 2)]!
  console.log(`${label}: 中位 ${median}ms  样本 [${samples.join(', ')}]`)
}

console.log(`冷启动基线（各 ${runs} 次，隔离 A_DA_HOME）`)

const inprocess: number[] = []
for (let i = 0; i < runs; i++) {
  inprocess.push(await once('inprocess', process.execPath, [path.join(root, 'app.tsx')]))
}
summarize('inprocess（bun app.tsx）', inprocess)

const binary = path.join(root, 'dist', process.platform === 'win32' ? 'a-da.exe' : 'a-da')
if (!existsSync(binary)) {
  console.log(`跳过 ws 形态：找不到 ${path.relative(root, binary)}（先跑 bun run build）`)
  process.exit(0)
}

const ws: number[] = []
for (let i = 0; i < runs; i++) {
  ws.push(await once('ws', binary, []))
}
summarize('ws（dist/a-da.exe，自 spawn 主机）', ws)

/**
 * 对照行：**打包产物 + 进程内**（`A_DA_TRANSPORT=inprocess` 可强制）。
 *
 * 这一行才是"拆分到底花了多少"的公平口径——上面那行是 dev（bun 直跑），
 * 与打包产物的启动开销本来就不是一回事。`ws - 打包inprocess` 就是自 spawn + 握手 + 首帧快照的代价。
 */
const packagedInprocess: number[] = []
for (let i = 0; i < runs; i++) {
  packagedInprocess.push(
    await once('打包-inprocess', binary, [], { A_DA_TRANSPORT: 'inprocess' })
  )
}
summarize('inprocess（同一个 exe，强制进程内）', packagedInprocess)

const median = (samples: number[]): number =>
  [...samples].sort((a, b) => a - b)[Math.floor(samples.length / 2)]!
const inprocessMedian = median(inprocess)
const wsMedian = median(ws)
const packagedMedian = median(packagedInprocess)
console.log(`拆分代价（打包 ws - 打包 inprocess）: ${wsMedian - packagedMedian}ms`)
console.log(`dev 与打包的启动差异（打包 inprocess - dev inprocess）: ${packagedMedian - inprocessMedian}ms`)
console.log(`总冷启动（打包 ws）: ${wsMedian}ms`)
