// ssr-h 流式输出的**实机验收**：界面到底是"逐字长出来"，还是"每 2 秒跳一大段"。
//
// 做法（零 API 花费、不碰用户 `~/.a-da`）：
//   1. 假模型：按 120ms 一段慢慢吐正文（真实模型的节奏）；
//   2. 真网关 + 真 ada-coding（`A_DA_HOME` 指向临时目录）；
//   3. 真 ssr-h，真实走一遍 /threads/{id}/send 与 /threads/{id}/stream；
//   4. 断言 SSE 里的 assistant item 更新：数量够多、间隔远小于 2 秒、正文单调变长。
//
// 这条脚本守的是**接线**：网关心跳/增量帧 → 服务端投影 → SSE → 浏览器侧原位替换。
// 而不是"某个函数返回了字符串"。

import { createServer } from 'node:http'
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from 'node:fs'
import { join } from 'node:path'
import { spawn, spawnSync } from 'node:child_process'
import { createServer as createNetServer } from 'node:net'

const REPO = 'E:\\codes\\rust_projects\\a_da'
const GATEWAY = join(REPO, 'dist', 'ada-gateway.exe')
const SSR = process.env.SSR_H_EXE || join(REPO, '..', 'cargo_target_ada', 'debug', 'ssr-h.exe')

const failures = []
const check = (ok, what, extra = '') => {
  console.log(`  ${ok ? '[PASS]' : '[FAIL]'} ${what}${ok || !extra ? '' : ` — ${extra}`}`)
  if (!ok) failures.push(what)
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

async function freePort() {
  const server = createNetServer()
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address()
  await new Promise((resolve) => server.close(resolve))
  return port
}

// 🔴 临时根目录**必须放在工作区内**：某些带沙箱的开发环境（本仓库的验收机就是）
// 会拒绝子进程读写系统 TEMP——症状是 ada-coding 报「创建会话目录失败」，
// 和流式实现毫无关系。放在 target/ 下既不污染仓库（已被 .gitignore），又一定可写。
const verifyRoot = join(REPO, 'target', 'ssr-verify')
mkdirSync(verifyRoot, { recursive: true })
const appHome = mkdtempSync(join(verifyRoot, 'home-'))
const wsDir = mkdtempSync(join(verifyRoot, 'ws-'))

// ── 假模型：14 段、每 120ms 一段 ────────────────────────────────────────────
const CHUNKS = [
  '第一段：',
  '我在慢慢写。',
  '这是流式输出，',
  '不是每两秒',
  '跳出来的一大块。',
  '你应当看到',
  '文字一段一段地',
  '长出来。',
  '第二行继续。',
  '这里还有内容，',
  '用来覆盖多帧。',
  '最后再补两句，',
  '确保总时长',
  '超过一秒。',
]
const model = createServer((req, res) => {
  let body = ''
  req.on('data', (chunk) => (body += chunk))
  req.on('end', async () => {
    res.writeHead(200, {
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache',
      Connection: 'keep-alive',
    })
    const send = (delta, finish) =>
      res.write(
        `data: ${JSON.stringify({
          id: 'stub',
          object: 'chat.completion.chunk',
          choices: [{ index: 0, delta, ...(finish ? { finish_reason: finish } : {}) }],
        })}\n\n`,
      )
    try {
      send({ role: 'assistant', content: '' })
      for (const chunk of CHUNKS) {
        send({ content: chunk })
        await sleep(120)
      }
      // 用量帧要在 finish_reason 之前：适配器见到 stop 就收尾，后面的帧不再消费。
      res.write(
        `data: ${JSON.stringify({
          id: 'stub',
          object: 'chat.completion.chunk',
          choices: [],
          usage: { prompt_tokens: 12000, completion_tokens: 300, total_tokens: 12300 },
        })}\n\n`,
      )
      send({}, 'stop')
      res.write('data: [DONE]\n\n')
      res.end()
    } catch {
      res.end()
    }
  })
})
await new Promise((resolve) => model.listen(0, '127.0.0.1', resolve))
const modelPort = model.address().port

writeFileSync(
  join(appHome, 'config.json'),
  JSON.stringify(
    {
      activeProviderId: 'prov_stub',
      apiKey: 'stub-key',
      baseUrl: `http://127.0.0.1:${modelPort}/v1`,
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
          baseUrl: `http://127.0.0.1:${modelPort}/v1`,
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

const children = []
function start(name, exe, args, env = {}) {
  const child = spawn(exe, args, {
    cwd: REPO,
    env: { ...process.env, ...env },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  children.push({ name, child })
  let output = ''
  child.stdout.on('data', (chunk) => (output += chunk.toString()))
  child.stderr.on('data', (chunk) => (output += chunk.toString()))
  child.output = () => output
  return child
}

function waitFor(child, predicate, timeoutMs, label) {
  return new Promise((resolve, reject) => {
    const started = Date.now()
    const timer = setInterval(() => {
      const output = child.output()
      if (predicate(output)) {
        clearInterval(timer)
        resolve(output)
      } else if (Date.now() - started > timeoutMs || child.exitCode !== null) {
        clearInterval(timer)
        reject(new Error(`等待 ${label} 超时（退出码 ${child.exitCode}）：\n${output.slice(-3000)}`))
      }
    }, 100)
  })
}

function cleanup() {
  if (process.env.SSR_VERIFY_DEBUG) {
    for (const { name, child } of children) {
      const output = typeof child.output === 'function' ? child.output() : child.output
      console.log(`  --- ${name} 输出末尾 ---\n${String(output || '').slice(-3000)}`)
    }
  }
  for (const { child } of children) {
    if (child.exitCode !== null) continue
    if (process.platform === 'win32') {
      spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore' })
    } else {
      child.kill('SIGKILL')
    }
  }
  try {
    model.close()
  } catch {}
  if (!process.env.KEEP_SSR_TEMP) {
    for (const dir of [appHome, wsDir]) {
      try {
        rmSync(dir, { recursive: true, force: true })
      } catch {}
    }
    try {
      rmSync(verifyRoot, { recursive: true, force: true })
    } catch {}
  } else {
    console.log('  保留临时目录：', appHome, wsDir)
  }
}

try {
  check(existsSync(GATEWAY), `网关二进制存在：${GATEWAY}`)
  check(existsSync(SSR), `ssr-h 二进制存在：${SSR}`)
  if (failures.length) throw new Error('缺少二进制')

  const gatewayPort = await freePort()
  const gateway = start(
    'gateway',
    GATEWAY,
    ['--port', String(gatewayPort), '--workspace', wsDir],
    { A_DA_HOME: appHome },
  )
  const readyLine = await waitFor(
    gateway,
    (output) => output.includes('A_DA_GATEWAY_READY'),
    20_000,
    '网关就绪',
  )
  const ready = JSON.parse(readyLine.split('A_DA_GATEWAY_READY')[1].trim().split('\n')[0])
  const gatewayUrl = `http://127.0.0.1:${ready.port}`
  console.log(`  网关就绪：${gatewayUrl}`)

  const uiPort = await freePort()
  const ssr = start('ssr-h', SSR, [
    '--port',
    String(uiPort),
    '--gateway',
    gatewayUrl,
    '--workspace',
    wsDir,
    '--plugins-dir',
    join(REPO, 'ssr-h', 'frontend-plugins'),
  ])
  await waitFor(ssr, (output) => output.includes('ssr-h listening on'), 20_000, 'ssr-h 就绪')
  const ui = `http://127.0.0.1:${uiPort}`
  console.log(`  ssr-h 就绪：${ui}`)

  // 建会话（走网关 HTTP，和 ssr-h 的 create_thread 同一条路）
  const created = await fetch(`${gatewayUrl}/api/threads?workspace=${encodeURIComponent(wsDir)}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ title: '流式验收', workspace: wsDir }),
  })
  const createdBody = await created.json().catch(() => ({}))
  const threadId = createdBody.threadId || createdBody.id
  check(Boolean(threadId), '会话创建成功', JSON.stringify(createdBody).slice(0, 300))
  if (!threadId) throw new Error('没有 threadId')

  // 发消息（走 ssr-h 自己的表单端点，和用户点发送完全一致）
  const sent = await fetch(`${ui}/threads/${threadId}/send`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ text: '慢慢写一段，验证流式输出。' }).toString(),
    redirect: 'manual',
  })
  check(sent.status === 303, '发送后被 303 跳回会话页', `HTTP ${sent.status}`)

  // 页面壳必须带线程身份（SSE 客户端靠它确认为哪条会话开流）
  const page = await (await fetch(`${ui}/threads/${threadId}`)).text()
  check(page.includes(`data-thread-id="${threadId}"`), '会话页带 data-thread-id')
  check(page.includes('data-item-id='), '对话行带 data-item-id（客户端按它原位替换）')

  // 直接读 SSE：解析事件名、正文与到达时间
  const controller = new AbortController()
  const stream = await fetch(`${ui}/threads/${threadId}/stream`, { signal: controller.signal })
  check(stream.status === 200, 'SSE 端点返回 200', `HTTP ${stream.status}`)
  check(
    (stream.headers.get('content-type') || '').includes('text/event-stream'),
    'SSE content-type 正确',
    stream.headers.get('content-type') || '',
  )

  const decoder = new TextDecoder()
  const events = []
  let buffer = ''
  const started = Date.now()
  const reader = stream.body.getReader()
  try {
    while (Date.now() - started < 6000) {
      const next = await Promise.race([
        reader.read(),
        sleep(6000).then(() => ({ done: true, value: undefined })),
      ])
      if (next.done) break
      buffer += decoder.decode(next.value, { stream: true })
      let cut
      while ((cut = buffer.indexOf('\n\n')) >= 0) {
        const block = buffer.slice(0, cut)
        buffer = buffer.slice(cut + 2)
        let event = 'message'
        const data = []
        for (const line of block.split('\n')) {
          if (line.startsWith('event:')) event = line.slice(6).trim()
          else if (line.startsWith('data:')) data.push(line.slice(5).trimStart())
        }
        if (data.length) {
          events.push({ event, data: data.join('\n'), at: Date.now() - started })
        }
      }
      // 不在"看到结尾文本"时立刻停：用量/收尾的 parts 在正文之后才到，
      // 提前退出会把它们漏掉（这正是"插件不更新"的观测盲区）。
    }
  } finally {
    controller.abort()
  }

  const transcripts = events.filter((ev) => ev.event === 'transcript')
  const items = events.filter((ev) => ev.event === 'item')
  const assistant = items.filter((ev) => ev.data.includes('ada-msg--assistant'))
  const parts = events.filter((ev) => ev.event === 'parts')

  const counts = {}
  for (const ev of events) counts[ev.event] = (counts[ev.event] || 0) + 1
  console.log('  事件统计：', counts)
  if (process.env.SSR_VERIFY_DEBUG) {
    console.log('  最后一次 transcript：', (transcripts.at(-1)?.data || '').slice(0, 2000))
    if (items.length) console.log('  第一条 item：', items[0].data.slice(0, 600))
  }

  check(transcripts.length >= 1, '收到整段 transcript 同步', `实际 ${transcripts.length} 次`)
  check(parts.length >= 1, '收到托盘/任务规划等动态区域（parts）', `实际 ${parts.length} 次`)
  if (process.env.SSR_VERIFY_DEBUG) {
    const snapshot = await (
      await fetch(`${gatewayUrl}/api/snapshot?workspace=${encodeURIComponent(wsDir)}`)
    ).json()
    const thread = (snapshot.threads || []).find((t) => t.id === threadId)
    console.log(
      '  快照里的助手用量：',
      (thread?.items || [])
        .filter((item) => item.kind === 'assistant')
        .map((item) => JSON.stringify(item.usage))
        .join(' | '),
      'running=',
      (snapshot.runningThreadIds || []).includes(threadId),
    )
    const paged = await (
      await fetch(
        `${gatewayUrl}/api/threads/${threadId}/items?workspace=${encodeURIComponent(wsDir)}&from=0&count=500`,
      )
    ).json()
    console.log(
      '  items.page 的助手用量：',
      (paged.items || [])
        .filter((item) => item.kind === 'assistant')
        .map((item) => JSON.stringify(item.usage))
        .join(' | '),
    )
  }
  const above = parts
    .map((ev) => {
      try {
        return JSON.parse(ev.data).above || ''
      } catch {
        return ''
      }
    })
    .filter(Boolean)
    .at(-1) || ''
  if (process.env.SSR_VERIFY_DEBUG) {
    console.log(
      '  parts 里的天气：',
      parts
        .map((ev) => {
          try {
            const above = JSON.parse(ev.data).above || ''
            return `${ev.at}ms:${(above.match(/晴天|多云|阴|暴雨|Token/) || ['?'])[0]}${
              (above.match(/· (\d+)%/) || [])[1] || ''
            }`
          } catch {
            return `${ev.at}ms:bad`
          }
        })
        .join(' | '),
    )
  }
  check(
    above.includes('37%'),
    'Token 天气插件拿到真实用量（不是晴天 0%）',
    above.slice(0, 240) || '（插件插槽为空）',
  )
  check(assistant.length >= 4, '助手正文至少被拆成 4 次增量推送', `实际 ${assistant.length} 次`)
  check(
    assistant.some((ev) => ev.data.includes('超过一秒')),
    '最后一帧包含结尾文本（没有丢流）',
    assistant.at(-1)?.data?.slice(0, 200) || '',
  )

  const marks = assistant.map((ev) => ev.at)
  const gaps = marks.slice(1).map((at, index) => at - marks[index])
  const maxGap = gaps.length ? Math.max(...gaps) : 0
  check(maxGap < 1200, '相邻增量间隔远小于旧的 2 秒整段刷新', `最大间隔 ${maxGap}ms`)
  check(
    marks.length >= 2 && marks[marks.length - 1] - marks[0] >= 400,
    '正文确实跨多帧长出来（不是一口气推完）',
    `首尾跨度 ${(marks.at(-1) || 0) - (marks[0] || 0)}ms`,
  )

  // 正文长度单调变长：说明每次推的是"当前全文"，不是随机片段
  const lengths = assistant.map((ev) => {
    const match = ev.data.match(/ada-markdown[^>]*>([\s\S]*)<\/div>/)
    return match ? match[1].length : ev.data.length
  })
  const growing = lengths.every((len, index) => index === 0 || len >= lengths[index - 1])
  check(growing, '增量正文长度单调变长', lengths.join(' → '))
} catch (error) {
  failures.push(String(error?.message || error))
  console.error(error)
} finally {
  cleanup()
}

console.log('')
if (failures.length) {
  console.error(`[FAIL] ssr-h 流式验收未通过（${failures.length} 项）`)
  process.exit(1)
}
console.log('[PASS] ssr-h 对话流是增量推送，不再是每 2 秒一大段')
