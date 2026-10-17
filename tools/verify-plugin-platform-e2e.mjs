// 脚本系统/插件平台的**协议级实机验收**（对 release 二进制跑真 JSON-RPC）。
//
// 验的是"编译出来的那一版里，这套机制真的活着"，而不是"库里的用例过了"：
//   1. `plugin.validate` 报出**这个插件实际拿到的**能力（P4·SP-71 的声明面）
//   2. `plugin.list` 的贡献字段来自**装配期活体视图**（P4：静态目录扫描恒为空）
//   3. `skill.list` 里插件技能带得住 `pluginId`（P2·SP-45 的归因）
//   4. `plugin.setEnabled` 真的**改变贡献集合**并回报差异收据（P2·SP-46），
//      且停用后贡献视图一起清空（P4）
//   5. 认不出的能力 token **不授予**且被报出来（SP-71 的失败安全）
//
// 不跑模型：这一版里所有插件/技能/装配路径都不需要模型，所以这几条是
// **纯协议面**能验的、且不依赖网络与供应商配置。
//
// 🔴 插件形态必须是**包根 `index.ts` + `metadata` 清单**（pi-style Extension Factory）：
// 散装文件与 `.ada/plugins` 旧目录已不再被发现，`plugin.validate` 也会直接拒。
// 跑法：先把 release 产物放到 `dist/ada-coding.exe`（`cargo xtask ship ada-coding`）。

import { mkdtempSync, writeFileSync, mkdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawn } from 'node:child_process'

const REPO = 'E:\\codes\\rust_projects\\a_da'
// 默认对交付产物跑；本机想验刚编出来那一版时用
// `A_DA_E2E_EXE=../cargo_target_ada/release/ada-coding.exe bun tools/verify-plugin-platform-e2e.mjs`
// （这样不必先覆写 `dist/` 里的既有交付物）。
const EXE = process.env.A_DA_E2E_EXE
  ? process.env.A_DA_E2E_EXE
  : join(REPO, 'dist', 'ada-coding.exe')

const failures = []
const check = (ok, what, extra = '') => {
  console.log(`  ${ok ? '[PASS]' : '[FAIL]'} ${what}${ok || !extra ? '' : ` — ${extra}`}`)
  if (!ok) failures.push(what)
}

// ── 临时工作区：一个声明了 net + 未知 token 的插件**包**（含钩子/技能/提示词/工具）──
//
// 🔴 形态必须是**包根 `index.ts` + `metadata` 清单**（pi-style Extension Factory）：
// 散装文件与 `.ada/plugins` 旧目录已不再被发现——`plugin.validate` 也会直接拒。
const wsDir = mkdtempSync(join(tmpdir(), 'ada_rel_ws_'))
const pkgDir = join(wsDir, '.ada', 'agent-coding', 'extensions', 'demo')
mkdirSync(pkgDir, { recursive: true })
const entry = join(pkgDir, 'index.ts')
writeFileSync(
  entry,
  `// @ada-needs net,nett
const metadata = {
  name: 'demo',
  description: 'e2e 插件包',
  tools: { count: 1, names: ['git_status'] },
  systemPrompt: { count: 1, names: ['house-style'] },
  skills: { count: 1, names: ['deploy-runbook'] },
  hooks: { count: 1, names: ['beforeToolCall'] },
};
export default function (ada) {
  ada.metadata = metadata;
  ada.registerTool({
    name: 'git_status',
    description: '插件版 git 状态',
    parameters: { type: 'object', properties: {} },
    execute: async () => ({ ok: true, output: 'plugin-ran' }),
  });
  ada.registerSystemPrompt({ id: 'house-style', content: '本项目一律用 tabs。' });
  ada.registerSkill({ name: 'deploy-runbook', description: '发布流程', body: '先跑测试再打标签' });
  ada.on('beforeToolCall', async () => ({ action: 'deny', reason: '测试用：拦下' }));
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
  const report = await call('plugin.validate', { filePath: entry, workspace: wsDir })
  const needs = String(report?.needs ?? '')
  check(needs.includes('net: 开'), '声明 net → 自检报告如实说网络开着', needs)
  check(needs.includes('nett'), '认不出的 token 要报出来（不能静默）', needs)

  // ② P4：`plugin.list` 的贡献字段来自**装配期活体视图**（静态目录扫描恒为空）
  const listed = await call('plugin.list', { workspace: wsDir })
  const demo = (listed?.plugins || []).find((p) => p.id === 'workspace:demo')
  check(!!demo, '磁盘插件包出现在 plugin.list 里', JSON.stringify(listed).slice(0, 300))
  check(
    (demo?.tools || []).some((t) => t.name === 'git_status'),
    '工具贡献由装配期执行结果填入（不再恒为空）',
    JSON.stringify(demo?.tools),
  )
  check(
    (demo?.hooks || []).includes('beforeToolCall'),
    '钩子点位清单也如实列出（"这个插件会动手做哪些事"）',
    JSON.stringify(demo?.hooks),
  )

  // ③ SP-45：插件技能带得住归因
  const skills = await call('skill.list', { workspace: wsDir })
  const pluginSkill = (skills || []).find((s) => s.name === 'deploy-runbook')
  check(!!pluginSkill, '插件技能出现在 skill.list 里', JSON.stringify(skills).slice(0, 200))
  check(pluginSkill?.pluginId === 'workspace:demo' || !!pluginSkill?.pluginId, '插件技能带得住 pluginId', JSON.stringify(pluginSkill))

  // ④ SP-46：启停**真的改变贡献集合**，并回报差异
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
  // ⑤ P4：停用之后**贡献视图**也必须跟着空（否则"关掉了插件、卡片还列着它的工具"）
  const listedOff = await call('plugin.list', { workspace: wsDir })
  const demoOff = (listedOff?.plugins || []).find((p) => p.id === 'workspace:demo')
  check(
    (demoOff?.tools || []).length === 0 && (demoOff?.hooks || []).length === 0,
    '停用后贡献视图随重装配一起清空',
    JSON.stringify({ tools: demoOff?.tools, hooks: demoOff?.hooks }),
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
