// 权限档位的**实机验收**：`Full`（完全访问）到底放开了什么、没放开什么。
//
// 为什么必须有这条：引擎在工具执行前用 `Scope` 判过一次路径，**工具内部又判了一次**
// （`check_workspace_sandbox`）。两处各判一次，`Full` 就在第二处被挡回去——
// 用户开着「完全访问」，界面横幅写着"可读写工作区之外的路径"，模型却一直拿到
// "拒绝访问工作区外的路径"。这条脚本用**真二进制 + 真协议 + 真一轮对话**把它钉死。
//
// 四个场景（假模型，零 API 花费；应用目录全部指向临时目录）：
//   A. `full` + 工作区**外**的普通文件  → 必须读成功（这是修复的主症状）
//   B. `full` + `~/.a-da/agent-coding/extensions/...` → 必须读成功（插件源码属于白名单）
//   C. `full` + `~/.a-da/config.json`   → 必须仍被拒（凭据红线，任何档位都不开）
//   D. `ask`  + 工作区外的普通文件      → 必须被拒，且提示"需要开启完全访问"
//
// 判定不依赖卡片字段的具体形状：直接看工具卡序列化后的文本里有没有文件内容 / 拒绝字样。

import { createServer } from 'node:http'
import { mkdtempSync, writeFileSync, mkdirSync, rmSync, existsSync } from 'node:fs'
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

const appHome = mkdtempSync(join(tmpdir(), 'ada_fa_home_'))
const wsDir = mkdtempSync(join(tmpdir(), 'ada_fa_ws_'))
const outDir = mkdtempSync(join(tmpdir(), 'ada_fa_out_'))
const outsideFile = join(outDir, 'outside.txt')
writeFileSync(outsideFile, 'OUTSIDE_OK\n')
const outsidePath = outsideFile.replace(/\\/g, '/')
const secretPath = join(appHome, 'config.json').replace(/\\/g, '/')

// `~/.a-da` 下**允许**读写的子树：装好的插件（与技能同构）
const pluginDir = join(appHome, 'agent-coding', 'extensions', 'demo')
mkdirSync(pluginDir, { recursive: true })
writeFileSync(join(pluginDir, 'index.ts'), 'PLUGIN_OK\n')
const pluginPath = join(pluginDir, 'index.ts').replace(/\\/g, '/')

// `~/.a-da` 下的 "秘密文件"：config.json 里是 API key，正是红线要护住的东西
writeFileSync(join(wsDir, 'inside.txt'), 'INSIDE_OK\n')

// ── 假模型：先要一次 read_file，拿到工具结果后收尾 ──────────────────────────
let currentPath = outsidePath
const stub = createServer((req, res) => {
  let body = ''
  req.on('data', (c) => (body += c))
  req.on('end', () => {
    let payload = {}
    try {
      payload = JSON.parse(body)
    } catch {}
    const hasToolResult = (payload.messages || []).some((m) => m.role === 'tool')
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' })
    const send = (delta, finish) =>
      res.write(
        `data: ${JSON.stringify({
          id: 'stub',
          object: 'chat.completion.chunk',
          choices: [{ index: 0, delta, ...(finish ? { finish_reason: finish } : {}) }],
        })}\n\n`,
      )
    if (!hasToolResult) {
      send({ role: 'assistant', content: '我看一下这个文件。' })
      send({
        tool_calls: [
          {
            index: 0,
            id: 'call_fa_1',
            type: 'function',
            function: { name: 'read_file', arguments: JSON.stringify({ path: currentPath }) },
          },
        ],
      })
      send({}, 'tool_calls')
    } else {
      send({ role: 'assistant', content: '看完了。' })
      send({}, 'stop')
    }
    res.write('data: [DONE]\n\n')
    res.end()
  })
})
await new Promise((r) => stub.listen(0, '127.0.0.1', r))
const stubPort = stub.address().port
console.log(`假模型就绪：http://127.0.0.1:${stubPort}/v1`)

writeFileSync(
  join(appHome, 'config.json'),
  JSON.stringify(
    {
      activeProviderId: 'prov_stub',
      apiKey: 'stub-key',
      baseUrl: `http://127.0.0.1:${stubPort}/v1`,
      contextWindow: 32768,
      maxOutputTokens: 1024,
      model: 'stub-model',
      protocol: 'openai_chat',
      retryCount: 1,
      disabledPlugins: [],
      providers: [
        {
          id: 'prov_stub',
          name: 'stub',
          baseUrl: `http://127.0.0.1:${stubPort}/v1`,
          apiKey: 'stub-key',
          protocol: 'openai_chat',
          models: [{ id: 'stub-model', name: 'Stub', contextWindow: 32768, maxOutputTokens: 1024 }],
        },
      ],
    },
    null,
    2,
  ),
)

if (!existsSync(EXE)) {
  console.error(`缺少已编译的产品：${EXE}`)
  process.exit(2)
}

const child = spawn(EXE, ['--port', '0', '--workspace', wsDir], {
  stdio: ['ignore', 'pipe', 'pipe'],
  env: { ...process.env, A_DA_HOME: appHome },
})
let out = ''
const ready = await new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error(`等 READY 超时；stdout=${out.slice(0, 400)}`)), 20000)
  child.stdout.on('data', (d) => {
    out += d.toString()
    const m = out.match(/A_DA_HOST_READY (\{.*\})/)
    if (m) {
      clearTimeout(timer)
      resolve(JSON.parse(m[1]))
    }
  })
  child.on('exit', (code) => reject(new Error(`产品提前退出（code=${code}）：${out.slice(-400)}`)))
})
console.log(`产品就绪：port=${ready.port} pid=${ready.pid} 工作区=${wsDir}`)

const socket = new WebSocket(`ws://127.0.0.1:${ready.port}/?token=${encodeURIComponent(ready.token)}`)
let nextId = 1
const pending = new Map()
// 响应分发：没有它，所有 `call()` 都会一直等到超时（只发不收）
socket.addEventListener('message', (ev) => {
  let msg
  try {
    msg = JSON.parse(ev.data)
  } catch {
    return
  }
  if (msg.id != null && pending.has(msg.id)) {
    pending.get(msg.id)(msg)
    pending.delete(msg.id)
  }
})
const call = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++
    pending.set(id, (m) => (m.error ? reject(new Error(`${method}: ${JSON.stringify(m.error)}`)) : resolve(m.result)))
    socket.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }))
    setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id)
        reject(new Error(`${method} 超时`))
      }
    }, 30000)
  })
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

const toolItems = (snap, threadId) => {
  const t = (snap?.threads || []).find((x) => x.id === threadId)
  return (t?.items || []).filter(
    (i) => i.kind === 'tool' || i.kind === 'toolCall' || i.role === 'tool' || i.tool || i.callId,
  )
}

/** 跑一轮：设档位 → 发消息 → 等收尾 → 把最后一张工具卡串化。 */
async function runTurn(mode, path, label) {
  currentPath = path
  await call('config.setApproval', { mode })
  const created = await call('thread.create', { workspace: wsDir })
  const threadId = created?.threadId || created?.id
  await call('thread.send', { threadId, text: `${label}：读一下这个文件` })

  let last = null
  for (let i = 0; i < 60; i++) {
    await sleep(400)
    const snap = await call('session.snapshot', {})
    const t = (snap?.threads || []).find((x) => x.id === threadId)
    const tools = toolItems(snap, threadId)
    last = tools[tools.length - 1] ?? null
    if (last && !t?.isRunning && !t?.running) break
  }
  return { threadId, text: last ? JSON.stringify(last) : '' }
}

try {
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve)
    socket.addEventListener('error', () => reject(new Error('WS 连接失败')))
  })
  await call('session.initialize', {
    protocolVersion: '1.0',
    client: { name: 'verify-full-access', version: '0.0.1', platform: 'win32', capabilities: [] },
  })

  // ── A. 完全访问 + 工作区外 ────────────────────────────────────────────
  console.log('\n== A. full 模式读工作区外的文件（修复的主症状）==')
  const a = await runTurn('full', outsidePath, 'A')
  console.log(`  工具卡：${a.text.slice(0, 260)}`)
  check(a.text.includes('OUTSIDE_OK'), '完全访问下能读到工作区外的文件内容', a.text.slice(0, 200))

  // ── B. 完全访问 + ~/.a-da 下的插件源码（收窄后的白名单）──────────────────
  console.log('\n== B. full 模式读 ~/.a-da/agent-coding/extensions（允许）==')
  const b = await runTurn('full', pluginPath, 'B')
  console.log(`  工具卡：${b.text.slice(0, 260)}`)
  check(b.text.includes('PLUGIN_OK'), '完全访问下能读到装好的插件源码', b.text.slice(0, 200))

  // ── C. 完全访问 + ~/.a-da/config.json（凭据红线）────────────────────────
  console.log('\n== C. full 模式读 ~/.a-da/config.json（凭据红线）==')
  const c = await runTurn('full', secretPath, 'C')
  console.log(`  工具卡：${c.text.slice(0, 260)}`)
  check(!c.text.includes('stub-key'), '完全访问下**读不到** ~/.a-da 里的密钥内容', c.text.slice(0, 200))
  check(
    c.text.includes('拒绝访问') && (c.text.includes('应用配置') || c.text.includes('.a-da')),
    '拒绝文案说清是"应用自身目录"，而不是含糊的"工作区外"',
    c.text.slice(0, 200),
  )

  // ── D. 默认档 + 工作区外 ─────────────────────────────────────────────
  console.log('\n== D. ask 模式读工作区外的文件（必须被拦）==')
  const d = await runTurn('ask', outsidePath, 'D')
  console.log(`  工具卡：${d.text.slice(0, 260)}`)
  check(!d.text.includes('OUTSIDE_OK'), '非完全访问档仍守工作区边界', d.text.slice(0, 200))
  check(d.text.includes('完全访问'), '拒绝文案告诉用户"需要开启完全访问"', d.text.slice(0, 200))
} finally {
  try {
    socket.close()
  } catch {}
  child.kill()
  stub.close()
  setTimeout(() => {
    for (const d of [wsDir, appHome, outDir]) {
      try {
        rmSync(d, { recursive: true, force: true })
      } catch {}
    }
  }, 500)
}

console.log(
  failures.length === 0
    ? '\nRESULT: PASS — 完全访问真的放开了工作区边界，红线仍然拦得住'
    : `\nRESULT: FAIL — ${failures.length} 条未通过：${failures.join('；')}`,
)
process.exit(failures.length === 0 ? 0 : 1)
