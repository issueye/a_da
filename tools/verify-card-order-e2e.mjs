// 卡片顺序的**实机验收**：轮次收尾时，磁盘回填把内存卡片整份换掉，顺序不许变。
//
// 为什么必须有这条：回填那一帧走的是增量通道（`evt.state.patch`），而增量帧的 `upserted`
// 是**按 id 排序**发的（帧内容要可复现），所以界面给新卡片定位**只能按 `at`**。
// 一条 assistant 消息派生的几张卡（思考 / 工具 / 正文）原本共用一个毫秒时间戳，
// 先后就落到 id 字典序上——`..._restored_asst_*` 排在 `..._restored_think_*` 前面，
// 于是**对话一跑完，思考块从最上面掉到最下面**（用户实拍）。库里的用例锁的是回填侧的时间戳，
// 这条锁的是"真二进制 + 真协议 + 真一轮对话"过完之后，界面拿到的东西。
//
// 做法：本地假模型（零 API 花费）走两轮——
//   场景 A：只回"思考 + 正文"（用户报的那一档），断言**收尾前后顺序完全一致**；
//   场景 B：再带一次工具调用，断言思考卡在正文上面（工具卡与正文的先后是折叠口径决定的，
//           不在这条用例的断言范围里）。
//
// 网络与供应商配置都不碰：模型是本脚本起的本地 stub，应用目录指向临时目录。

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

// ── 本地假模型：按请求次序产出"思考 → 正文（→ 工具调用）" ────────────────
const withTool = process.argv.includes('--with-tool')
let round = 0
const stub = createServer((req, res) => {
  let body = ''
  req.on('data', (c) => (body += c))
  req.on('end', () => {
    round += 1
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' })
    const send = (delta, finish) =>
      res.write(
        `data: ${JSON.stringify({
          id: 'stub',
          object: 'chat.completion.chunk',
          choices: [{ index: 0, delta, ...(finish ? { finish_reason: finish } : {}) }],
        })}\n\n`,
      )
    // 思考链走 `reasoning_content`（适配器认的字段），与正文在**同一条消息**里
    send({ role: 'assistant', reasoning_content: `第 ${round} 轮：先想一下再回答。` })
    if (withTool && round === 1) {
      send({ content: '我先看一眼工作区里有什么。' })
      send({
        tool_calls: [
          {
            index: 0,
            id: 'call_order_1',
            type: 'function',
            function: { name: 'list_files', arguments: JSON.stringify({ path: '.' }) },
          },
        ],
      })
      send({}, 'tool_calls')
    } else {
      send({ content: `第 ${round} 轮的回答正文。` })
      send({}, 'stop')
    }
    res.write('data: [DONE]\n\n')
    res.end()
  })
})
await new Promise((r) => stub.listen(0, '127.0.0.1', r))
const stubPort = stub.address().port
console.log(`假模型就绪：http://127.0.0.1:${stubPort}/v1（${withTool ? '含一轮工具调用' : '仅思考+正文'}）`)

// ── 隔离的应用目录 + 指向假模型的配置 ─────────────────────────────────────
const appHome = mkdtempSync(join(tmpdir(), 'ada_order_home_'))
const wsDir = mkdtempSync(join(tmpdir(), 'ada_order_ws_'))
mkdirSync(join(wsDir, '.ada'), { recursive: true })
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
  console.error(`缺少已编译的产品：${EXE}（先 bun run electron:pack 或 cargo build --release -p ada-coding）`)
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
console.log(`产品就绪：port=${ready.port} pid=${ready.pid}`)

const socket = new WebSocket(`ws://127.0.0.1:${ready.port}/?token=${encodeURIComponent(ready.token)}`)
let nextId = 1
const pending = new Map()

// ── 客户端镜像：**事故发生在前端**，所以这里必须站在前端的角度记账 ────────────
//
// 界面拿到卡片走两条路：全量快照（首帧/自愈）与增量帧 `evt.state.patch`。
// 增量帧的 `upserted` 是**按 id 排序**发的，界面给新卡片定位只能按 `at`
// （`desktop-ui/src/client/ws-client.ts` 的 `withItemDelta` + `insertByItemTime`）。
// 只读服务端快照是看不出问题的——服务端数组一直是对的，被换乱的是**界面拼出来的那份**。
// 下面这段必须与 `withItemDelta` 同口径：先摘 removed、再按 upserted 的**到达顺序**
// 逐条合并，新 id 按 `at` 稳定插入。
const mirror = new Map() // threadId -> { kind, id, at }[]
let lastSeq = 0
let mirrorApplications = 0

const applyItems = (list, upserts, dropIds) => {
  let items = dropIds && dropIds.size ? list.filter((x) => !dropIds.has(x.id)) : [...list]
  for (const it of upserts) {
    const idx = items.findIndex((x) => x.id === it.id)
    if (idx >= 0) {
      items = items.map((x, i) => (i === idx ? it : x))
      continue
    }
    const at = typeof it.at === 'number' ? it.at : null
    if (at === null) {
      items = [...items, it]
      continue
    }
    let i = items.length
    while (i > 0 && typeof items[i - 1]?.at === 'number' && items[i - 1].at > at) i--
    items = [...items.slice(0, i), it, ...items.slice(i)]
  }
  return items
}

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
    return
  }
  // 事件：把卡片流按界面那条规则重放一遍
  const method = msg.method
  const params = msg.params || {}
  if (method === 'evt.state.patch') {
    if (params.baseSeq !== lastSeq) return // 界面此时会去自愈要全量；这里不管那一拍
    lastSeq = params.seq
    mirrorApplications += 1
    const drops = new Map()
    for (const r of params.removed || []) {
      if (!r?.threadId || !r.itemId) continue
      if (!drops.has(r.threadId)) drops.set(r.threadId, new Set())
      drops.get(r.threadId).add(r.itemId)
    }
    const ups = new Map()
    for (const u of params.upserted || []) {
      if (!u?.threadId || !u.item) continue
      if (!ups.has(u.threadId)) ups.set(u.threadId, [])
      ups.get(u.threadId).push(u.item)
    }
    for (const t of params.meta?.threads || []) {
      if (!ups.has(t.id) && !drops.has(t.id)) continue
      mirror.set(t.id, applyItems(mirror.get(t.id) || [], ups.get(t.id) || [], drops.get(t.id)))
    }
  } else if (method === 'evt.state.snapshot' || method === 'evt.snapshot') {
    const snap = params.payload || params
    if (typeof snap.seq === 'number') lastSeq = snap.seq
    for (const t of snap.threads || []) mirror.set(t.id, [...(t.items || [])])
  } else if (method === 'evt.thread.item') {
    const { threadId, item } = params
    if (threadId && item) mirror.set(threadId, applyItems(mirror.get(threadId) || [], [item]))
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

const kindsOf = (snap, threadId) => {
  const t = (snap?.threads || []).find((x) => x.id === threadId)
  return (t?.items || []).map((i) => i.kind || i.role)
}
const mirrorKinds = (threadId) => (mirror.get(threadId) || []).map((i) => i.kind || i.role)
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

try {
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve)
    socket.addEventListener('error', () => reject(new Error('WS 连接失败')))
  })
  // 声明 `state.patch`：宿主随后发**增量帧**——出事的那条路
  await call('session.initialize', {
    protocolVersion: '1.0',
    client: { name: 'verify-card-order', version: '0.0.1', platform: 'win32', capabilities: ['state.patch'] },
  })

  const created = await call('thread.create', { workspace: wsDir })
  const threadId = created?.threadId || created?.id || (await call('session.snapshot', {})).activeThreadId
  // 与界面一致：先吃一份全量快照当基线
  const baseSnap = await call('session.snapshot', {})
  if (typeof baseSnap?.seq === 'number') lastSeq = baseSnap.seq
  for (const t of baseSnap?.threads || []) mirror.set(t.id, [...(t.items || [])])

  await call('thread.send', { threadId, text: '你好' })

  // 等这一轮跑完（假模型是同步吐的，很快）
  let live = []
  for (let i = 0; i < 60; i++) {
    await sleep(500)
    const snap = await call('session.snapshot', {})
    const t = (snap?.threads || []).find((x) => x.id === threadId)
    live = kindsOf(snap, threadId)
    if (live.includes('assistant') && !t?.isRunning && !t?.running) break
  }
  const liveMirror = mirrorKinds(threadId)
  console.log(`  收尾前：宿主 ${live.join(' → ')}｜界面 ${liveMirror.join(' → ')}（增量帧已应用 ${mirrorApplications} 次）`)
  check(live.includes('thinking'), '收尾前能看到思考卡（假模型确实吐了 reasoning_content）', live.join(','))
  check(
    JSON.stringify(liveMirror) === JSON.stringify(live),
    '收尾前界面与宿主顺序一致（增量帧这条路本身就正确）',
    `宿主 ${live.join(' → ')} / 界面 ${liveMirror.join(' → ')}`,
  )

  // 磁盘回填：`sync_threads_from_disk` 周期性跑，1.5s 一次，等两拍再取
  await sleep(4000)
  const snapAfter = await call('session.snapshot', {})
  const after = kindsOf(snapAfter, threadId)
  const afterMirror = mirrorKinds(threadId)
  console.log(`  收尾后：宿主 ${after.join(' → ')}｜界面 ${afterMirror.join(' → ')}`)

  check(
    JSON.stringify(afterMirror) === JSON.stringify(after),
    '磁盘回填后界面顺序 == 宿主顺序（id 全变的那一帧按 at 定位，不许换乱）',
    `宿主 ${after.join(' → ')} / 界面 ${afterMirror.join(' → ')}`,
  )
  check(
    JSON.stringify(afterMirror) === JSON.stringify(liveMirror),
    '轮次收尾前后，界面上的卡片顺序没有变化',
    `收尾前 ${liveMirror.join(' → ')} / 收尾后 ${afterMirror.join(' → ')}`,
  )
  check(
    afterMirror.indexOf('thinking') < afterMirror.indexOf('assistant'),
    '思考卡在正文上面（用户报的就是它掉到了下面）',
    afterMirror.join(' → '),
  )
} finally {
  try {
    socket.close()
  } catch {}
  child.kill()
  stub.close()
  setTimeout(() => {
    try {
      rmSync(wsDir, { recursive: true, force: true })
      rmSync(appHome, { recursive: true, force: true })
    } catch {}
  }, 500)
}

console.log(
  failures.length === 0
    ? '\nRESULT: PASS — 轮次收尾不再打乱卡片顺序'
    : `\nRESULT: FAIL — ${failures.length} 条未通过：${failures.join('；')}`,
)
process.exit(failures.length === 0 ? 0 : 1)
