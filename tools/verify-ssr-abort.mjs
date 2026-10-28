// SSR 浏览器端「流式 → 中止」实机验收。
//
// 覆盖：模型持续流式输出 → 页面正文增长 → 点「中止」→ 运行状态结束、正文不再增长。
//
// 零 API 花费、不碰用户 `~/.a-da`：假模型 + 真网关 + 真 ada-coding + 真 SSR 路由。

import { createServer } from 'node:http'
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync} from 'node:fs'
import { join } from 'node:path'
import { spawn, spawnSync } from 'node:child_process'
import { createServer as createNetServer } from 'node:net'

const REPO = 'E:\\codes\\rust_projects\\a_da'
const GATEWAY =
  process.env.A_DA_GATEWAY_EXE ||
  [
    join(REPO, '..', 'cargo_target_ada', 'debug', 'ada-gateway.exe'),
    join(REPO, 'cargo_target_ada', 'debug', 'ada-gateway.exe'),
    join(REPO, 'target', 'debug', 'ada-gateway.exe'),
    join(REPO, 'dist', 'ada-gateway.exe'),
  ].find((candidate) => existsSync(candidate)) ||
  join(REPO, 'dist', 'ada-gateway.exe')
const WEB_PLUGINS = join(REPO, 'plugins')

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

const verifyRoot = join(REPO, 'target', 'ssr-verify-abort')
mkdirSync(verifyRoot, { recursive: true })
const appHome = mkdtempSync(join(verifyRoot, 'home-'))
const wsDir = mkdtempSync(join(verifyRoot, 'ws-'))

// ── 假模型：直接慢速流式输出，给中止留出窗口 ────────────────────────────────
const modelRequests = []
const model = createServer((req, res) => {
  let body = ''
  req.on('data', (chunk) => (body += chunk))
  req.on('end', async () => {
    modelRequests.push(body)
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
      for (let i = 0; i < 80; i += 1) {
        send({ content: `第 ${i} 段：中止验收。` })
        await sleep(150)
      }
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
      console.log(`  --- ${name} 输出末尾 ---\n${String(child.output()).slice(-3000)}`)
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

async function getJson(url) {
  const response = await fetch(url)
  return response.json()
}

async function snapshotThread(gatewayUrl, threadId) {
  const snapshot = await getJson(
    `${gatewayUrl}/api/snapshot?workspace=${encodeURIComponent(wsDir)}`,
  )
  const thread = (snapshot.threads || []).find((item) => item.id === threadId)
  const running = (snapshot.runningThreadIds || []).includes(threadId)
  return { snapshot, thread, running }
}

async function assistantText(gatewayUrl, threadId) {
  const { thread } = await snapshotThread(gatewayUrl, threadId)
  return (thread?.items || [])
    .filter((item) => item.kind === 'assistant')
    .map((item) => item.text || '')
    .join('\n')
}

try {
  check(existsSync(GATEWAY), `网关二进制存在：${GATEWAY}`)
  if (failures.length) throw new Error('缺少二进制')

  const gatewayPort = await freePort()
  const gateway = start(
    'gateway',
    GATEWAY,
    [
      '--port',
      String(gatewayPort),
      '--workspace',
      wsDir,
      '--web-ui',
      'ssr',
      '--web-plugins-dir',
      WEB_PLUGINS,
    ],
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
  console.log(`  网关 + SSR 就绪：${gatewayUrl}`)

  const created = await fetch(
    `${gatewayUrl}/api/threads?workspace=${encodeURIComponent(wsDir)}`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ title: '中止验收', workspace: wsDir }),
    },
  )
  const createdBody = await created.json().catch(() => ({}))
  const threadId = createdBody.threadId || createdBody.id
  check(Boolean(threadId), '会话创建成功', JSON.stringify(createdBody).slice(0, 300))
  if (!threadId) throw new Error('没有 threadId')

  const sent = await fetch(`${gatewayUrl}/threads/${threadId}/send`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ text: '请持续输出一段长文，用于中止验收。' }).toString(),
    redirect: 'manual',
  })
  check(sent.status === 303, '发送后被 303 跳回会话页', `HTTP ${sent.status}`)

  let beforeAbort = ''
  for (let i = 0; i < 100; i += 1) {
    beforeAbort = await assistantText(gatewayUrl, threadId)
    if (beforeAbort.length > 0) break
    await sleep(100)
  }
  check(beforeAbort.length > 0, '正文已开始流式输出', beforeAbort.slice(0, 80))

  const aborted = await fetch(`${gatewayUrl}/threads/${threadId}/abort`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    redirect: 'manual',
  })
  check(aborted.status === 303, '中止请求 303 回跳', `HTTP ${aborted.status}`)

  let stopped = false
  for (let i = 0; i < 100; i += 1) {
    const state = await snapshotThread(gatewayUrl, threadId)
    if (!state.running) {
      stopped = true
      break
    }
    await sleep(100)
  }
  check(stopped, '中止后 runningThreadIds 不再包含该会话')

  const textAtAbort = await assistantText(gatewayUrl, threadId)
  await sleep(1000)
  const textAfter = await assistantText(gatewayUrl, threadId)
  check(
    textAfter.length <= textAtAbort.length + 40,
    '中止后助手正文不再持续增长',
    `${textAtAbort.length} → ${textAfter.length}`,
  )
  check(modelRequests.length >= 1, '假模型至少收到一次请求')
} finally {
  cleanup()
}

if (failures.length) {
  console.error(`\n[FAIL] SSR 流式 → 中止验收未通过（${failures.length} 项）`)
  process.exit(1)
}
console.log('\n[PASS] SSR 浏览器端：流式输出 → 中止 → 状态收尾全流程可用')
