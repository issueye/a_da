/**
 * 带**硬超时**的测试运行器。
 *
 * ## 为什么需要它
 *
 * `bun test` 自己有 5s 的**单条用例**超时，但整个套件没有上限。而这套测试里有一类
 * 故障会让总时长失控：一个真窗口用例在中途断言失败 → 窗口没关（见 `PluginsDialog.test.tsx`
 * 里那条"同一时刻只让一个真窗口活着"的约束）→ 后面每个真窗口用例的坐标点击全部落空
 * → 每个用例都跑满自己的轮询超时才失败 → 十几个用例 × 10~30s，表现为"整套卡死"。
 *
 * 真实发生过：正常 ~84s 的全套，因为一个窗口泄漏跑了 6 分钟还没停。
 *
 * ## 用法
 *
 * ```bash
 * bun run test:guarded                    # 默认 2 分钟上限（正常一套约 90~120s）
 * A_DA_TEST_TIMEOUT_MS=300000 bun run test:guarded
 * bun run test:guarded src/agent          # 透传筛选参数
 * ```
 *
 * 2 分钟的来历：正常全套约 90~120s，护栏只留很窄的余量，是刻意的——这个套件一旦出问题
 * 不会"变慢一点"，而是**级联**：某个真窗口用例失败后没关窗（窗口泄漏），后面每个真窗口
 * 用例的坐标点击全部落空，各跑满自己的轮询超时；再往后连纯文件系统的用例也会因为事件
 * 循环被饿死而逐个撞上 5s 超时。此时套件会拖到几分钟还不停。所以宁可**误杀**也不要
 * 陪着它耗——被杀会看到明确的失败行，比盯着一个永远不结束的进度条有用得多。
 * 真觉得机器慢就临时放宽：`A_DA_TEST_TIMEOUT_MS=600000 bun run test:guarded`。
 *
 * 超时后**强制终止子进程**并以非零码退出——让"卡住"变成一次明确的失败，而不是一个
 * 永远占着终端的等待。
 */

import { spawn } from 'node:child_process'

const DEFAULT_TIMEOUT_MS = 2 * 60_000

function readTimeout(): number {
  const raw = process.env.A_DA_TEST_TIMEOUT_MS
  if (!raw) return DEFAULT_TIMEOUT_MS
  const parsed = Number(raw)
  // 解析不出来就用默认值，但要说一句——否则"我明明设了超时"却没生效会很难查
  if (!Number.isFinite(parsed) || parsed <= 0) {
    console.warn(`[test] A_DA_TEST_TIMEOUT_MS="${raw}" 不是正数，改用默认 ${DEFAULT_TIMEOUT_MS}ms`)
    return DEFAULT_TIMEOUT_MS
  }
  return parsed
}

const timeoutMs = readTimeout()
const startedAt = Date.now()

// stdio: 'inherit' —— 直接继承终端输出，进度实时可见；用 pipe 截断会让整套测试盲跑
const child = spawn('bun', ['test', ...process.argv.slice(2)], { stdio: 'inherit' })

const killer = setTimeout(() => {
  const seconds = Math.round((Date.now() - startedAt) / 1000)
  console.error(`\n[test] 已运行 ${seconds}s，超过上限 ${Math.round(timeoutMs / 1000)}s，强制终止。`)
  console.error('[test] 若这是首次出现，多半是某个真窗口用例失败后没关窗（窗口泄漏会让后续用例逐个跑满超时）。')
  child.kill()
  process.exitCode = 1
}, timeoutMs)

child.on('error', (err) => {
  clearTimeout(killer)
  console.error(`[test] 无法启动 bun test：${err.message}`)
  process.exitCode = 1
})

child.on('exit', (code, signal) => {
  clearTimeout(killer)
  const seconds = ((Date.now() - startedAt) / 1000).toFixed(1)
  // 被超时杀掉时 signal 非空；那一条原因上方已经写过，这里只补时长与退出码
  console.log(`[test] 用时 ${seconds}s（退出码 ${code ?? signal}）`)
  process.exitCode = code ?? (signal ? 1 : 0)
})