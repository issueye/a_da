// 脚本系统/插件平台的**协议级实机验收**（对 release 二进制跑真 JSON-RPC）。
//
// 验的是"编译出来的那一版里，这套机制真的活着"，而不是"库里的用例过了"：
//   1. `plugin.validate` 报出**这个插件实际拿到的**能力（P4·SP-71 的声明面）
//   2. `skill.list` 里插件技能带得住 `pluginId`（P2·SP-45 的归因）
//   3. `plugin.setEnabled` 真的**改变贡献集合**并回报差异收据（P2·SP-46）
//   4. 认不出的能力 token **不授予**且被报出来（SP-71 的失败安全）
//
// 不跑模型：这一版里所有插件/技能/装配路径都不需要模型，所以这四条是
// **纯协议面**能验的、且不依赖网络与供应商配置。

import { mkdtempSync, writeFileSync, mkdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawn } from 'node:child_process'

const REPO = 'E:\\codes\\rust_projects\\a_da'
const EXE = join(REPO, 'dist', 'ada-coding.exe')

const failures = []
const check = (ok, what, extra = '') => {
  console.log(`  ${ok ? '[PASS]' : '[FAIL]'} ${what}${ok || !extra ? '' : ` — ${extra}`}`)
  if (!ok) failures.push(what)
}

// ── 临时工作区：一个声明了 net + 未知 token 的插件（含钩子/技能/提示词/工具）──
const wsDir = mkdtempSync(join(tmpdir(), 'ada_rel_ws_'))
const pluginsDir = join(wsDir, '.ada', 'plugins')
mkdirSync(pluginsDir, { recursive: true })
writeFileSync(
  join(pluginsDir, 'demo.ts'),
  `// @ada-needs net,nett
export default function (api) {
  api.registerTool({
    name: 'git_status',
    description: '插件版 git 状态',
    execute: async () => ({ ok: true, output: 'plugin-ran' }),
  });
  api.registerSkill({ name: 'deploy-runbook', description: '发布流程', body: '先跑测试再打标签' });
  api.registerPrompt({ id: 'house-style', content: '本项目一律用 tabs。' });
  api.registerHook({
    point: 'beforeToolCall',
    actions: ['deny'],
    handler: async () => ({ action: 'deny', reason: '测试用：拦下' }),
  });
}
`,
)

// ── 起真产品，拿端口与 token ────────────────────────────────────────────────
//
// 🔴 **应用目录必须隔离**（`A_DA_HOME` 指向临时目录）：这个脚本会调
// `plugin.setEnabled`，而它按设计把启停状态写进**应用目录**的 `config.json`。
// 不隔离就是拿用户真实配置当草稿纸——本项目为此出过一次真事故（测试改掉了
// 用户激活的供应商），所以这里显式隔离，跑完连临时目录一起删。
const appHome = mkdtempSync(join(tmpdir(), 'ada_e2e_home_'))
const child = spawn(EXE, ['--port', '0', '--workspace', wsDir], {
  stdio: ['ignore', 'pipe', 'pipe'],
  env: { ...process.env, A_DA_HOME: appHome },
})
let stdoutBuf = ''
const ready = await new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error(`等 READY 超时；stdout=${stdoutBuf.slice(0, 400)}`)), 20000)
  child.stdout.on('data', (d) => {
    stdoutBuf += d.toString()
    const m = stdoutBuf.match(/A_DA_HOST_READY (\{.*\})/)
    if (m) {
      clearTimeout(timer)
      try {
        resolve(JSON.parse(m[1]))
      } catch (e) {
        reject(new Error(`READY 行解析失败：${m[1]}（${e}）`))
      }
    }
  })
  child.on('exit', (code) => reject(new Error(`产品提前退出（code=${code}）`)))
})
console.log(`产品就绪：port=${ready.port} pid=${ready.pid}`)

// ── 真 JSON-RPC over WebSocket ─────────────────────────────────────────────
const socket = new WebSocket(`ws://127.0.0.1:${ready.port}/?token=${encodeURIComponent(ready.token)}`)
let nextId = 1
const pending = new Map()

socket.addEventListener('message', (ev) => {
  let msg
  try {
    msg = JSON.parse(ev.data)
  } catch {
    return
  }
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg)
    pending.delete(msg.id)
  }
})

const call = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++
    pending.set(id, (msg) => (msg.error ? reject(new Error(`${method}: ${JSON.stringify(msg.error)}`)) : resolve(msg.result)))
    socket.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }))
    setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id)
        reject(new Error(`${method} 超时`))
      }
    }, 20000)
  })

await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve)
  socket.addEventListener('error', () => reject(new Error('WS 连接失败')))
})
await call('session.initialize', { protocolVersion: '1.0' })

try {
  // ① SP-71：能力声明（含认不出的 token）
  const report = await call('plugin.validate', { filePath: join(pluginsDir, 'demo.ts') })
  const needs = String(report?.needs ?? '')
  check(needs.includes('net: 开'), '声明 net → 自检报告如实说网络开着', needs)
  check(needs.includes('nett'), '认不出的 token 要报出来（不能静默）', needs)

  // ② SP-45：插件技能带得住归因
  const skills = await call('skill.list', { workspace: wsDir })
  const pluginSkill = (skills || []).find((s) => s.name === 'deploy-runbook')
  check(!!pluginSkill, '插件技能出现在 skill.list 里', JSON.stringify(skills).slice(0, 200))
  check(pluginSkill?.pluginId === 'workspace:demo' || !!pluginSkill?.pluginId, '插件技能带得住 pluginId', JSON.stringify(pluginSkill))

  // ③ SP-46：启停**真的改变贡献集合**，并回报差异
  const off = await call('plugin.setEnabled', { pluginId: 'workspace:demo', enabled: false })
  check(off?.applied === true, 'plugin.setEnabled 回报"已生效"', JSON.stringify(off).slice(0, 300))
  const removed = off?.report?.delta?.removed ?? []
  check(
    removed.some((l) => l.includes('deploy-runbook')) && removed.some((l) => l.includes('git_status')),
    '差异里报出被移除的工具与技能',
    JSON.stringify(removed),
  )
  const skillsAfter = await call('skill.list', { workspace: wsDir })
  check(
    !(skillsAfter || []).some((s) => s.name === 'deploy-runbook'),
    '停用之后技能列表里就没有它了（不是只改了个布尔值）',
  )
  const on = await call('plugin.setEnabled', { pluginId: 'workspace:demo', enabled: true })
  check(on?.applied === true, '再启用同样回报已生效')
} finally {
  try {
    socket.close()
  } catch {}
  child.kill()
  setTimeout(() => {
    try {
      rmSync(wsDir, { recursive: true, force: true })
      rmSync(appHome, { recursive: true, force: true })
    } catch {}
  }, 500)
}

console.log(
  failures.length === 0
    ? '\nRESULT: PASS — 声明面 / 归因 / 重装配收据 在 release 二进制上全部成立'
    : `\nRESULT: FAIL — ${failures.length} 条未通过：${failures.join('；')}`,
)
process.exit(failures.length === 0 ? 0 : 1)
