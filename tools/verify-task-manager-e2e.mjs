// 任务管理插件的**实机验收**：真网关 + 真 agent + 真一轮工具调用。
//
// 做法：本地假模型（零 API 花费）让模型调用一次 `run_background`，起一个
// 30 秒的后台命令；然后打开会话页，断言 `task-manager` 插件渲染出命令 / PID /
// 终止表单，再走核心 `POST /tasks/{id}/kill` 终止它，最后断言面板变成"已结束"。
//
// 这条用例守的是"工具起任务 → task.list → props.data.tasks → 插件 → task.kill"
// 整条链真的通，而不只是单测里注入一张假任务表。

import { createServer } from 'node:http'
import { mkdtempSync, writeFileSync, rmSync, existsSync, mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawn, spawnSync } from 'node:child_process'
import { createServer as createNetServer } from 'node:net'

const REPO = 'E:\\codes\\rust_projects\\a_da'
const GATEWAY = join(REPO, '..', 'cargo_target_ada', 'debug', 'ada-gateway.exe')
const WEB_PLUGINS = join(REPO, 'crates', 'agent-web', 'frontend-plugins')
const UNIFIED_PLUGINS = join(REPO, 'plugins')
const COMMAND = 'ping 127.0.0.1 -n 60'

const failures = []
const check = (ok, what, extra = '') => {
  console.log(`  ${ok ? '[PASS]' : '[FAIL]'} ${what}${ok || !extra ? '' : ` — ${extra}`}`)
  if (!ok) failures.push(what)
}

async function freePort() {
  const server = createNetServer()
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address()
  await new Promise((resolve) => server.close(resolve))
  return port
}

async function waitFor(fn, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs
  let last
  while (Date.now() < deadline) {
    last = await fn()
    if (last) return last
    await new Promise((resolve) => setTimeout(resolve, 150))
  }
  throw new Error(`${label} 超时`)
}

// ── 本地假模型：第一轮调用 run_background，第二轮收尾 ─────────────────────
let modelRound = 0
const stub = createServer((req, res) => {
  let body = ''
  req.on('data', (chunk) => (body += chunk))
  req.on('end', () => {
    modelRound += 1
    if (process.env.TASK_VERIFY_DEBUG) {
      console.log(`  [stub] 第 ${modelRound} 次模型请求`)
    }
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' })
    const send = (delta, finish) =>
      res.write(
        `data: ${JSON.stringify({
          id: 'stub',
          object: 'chat.completion.chunk',
          choices: [{ index: 0, delta, ...(finish ? { finish_reason: finish } : {}) }],
        })}\n\n`,
      )
    if (modelRound === 1) {
      send({ role: 'assistant', content: '我先起一个后台任务。' })
      send({
        tool_calls: [
          {
            index: 0,
            id: 'call_task_manager_1',
            type: 'function',
            function: {
              name: 'run_background',
              arguments: JSON.stringify({ command: COMMAND }),
            },
          },
        ],
      })
      send({}, 'tool_calls')
    } else {
      send({ role: 'assistant', content: '后台任务已启动。' })
      send({}, 'stop')
    }
    res.write('data: [DONE]\n\n')
    res.end()
  })
})
await new Promise((resolve) => stub.listen(0, '127.0.0.1', resolve))
const stubPort = stub.address().port
console.log(`假模型就绪：http://127.0.0.1:${stubPort}/v1`)

const appHome = mkdtempSync(join(tmpdir(), 'ada_task_home_'))
const workspace = mkdtempSync(join(tmpdir(), 'ada_task_ws_'))
mkdirSync(join(workspace, '.ada'), { recursive: true })
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
      approval: 'auto',
      disabledPlugins: [],
      providers: [
        {
          id: 'prov_stub',
          name: 'stub',
          baseUrl: `http://127.0.0.1:${stubPort}/v1`,
          apiKey: 'stub-key',
          protocol: 'openai_chat',
          models: [
            { id: 'stub-model', name: 'Stub', contextWindow: 32768, maxOutputTokens: 1024 },
          ],
        },
      ],
    },
    null,
    2,
  ),
)

if (!existsSync(GATEWAY)) {
  console.error(`缺少已编译的网关：${GATEWAY}`)
  process.exit(2)
}

const gatewayPort = await freePort()
const gateway = spawn(
  GATEWAY,
  [
    '--port',
    String(gatewayPort),
    '--workspace',
    workspace,
    '--web-plugins-dir',
    WEB_PLUGINS,
  ],
  {
    cwd: REPO,
    env: {
      ...process.env,
      A_DA_HOME: appHome,
      A_DA_UNIFIED_PLUGINS_DIR: UNIFIED_PLUGINS,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  },
)
let gatewayOutput = ''
gateway.stdout.on('data', (chunk) => (gatewayOutput += chunk.toString()))
gateway.stderr.on('data', (chunk) => (gatewayOutput += chunk.toString()))

async function cleanup() {
  try {
    if (gateway.exitCode === null) {
      gateway.kill()
      await new Promise((resolve) => setTimeout(resolve, 500))
    }
  } catch {}
  // 网关退出不清子进程时补一刀（任务命令本身可能还挂着）；已经退出就不再按 PID 追打，
  // 避免覆盖到被系统复用的进程。
  if (process.platform === 'win32' && gateway.exitCode === null) {
    spawnSync('taskkill', ['/PID', String(gateway.pid), '/T', '/F'], { stdio: 'ignore' })
  }
  stub.close()
  try {
    rmSync(appHome, { recursive: true, force: true })
    rmSync(workspace, { recursive: true, force: true })
  } catch {}
}

try {
  const ready = await waitFor(
    () => (gatewayOutput.includes('A_DA_GATEWAY_READY') ? gatewayOutput : null),
    20_000,
    '网关就绪',
  )
  const info = JSON.parse(ready.split('A_DA_GATEWAY_READY')[1].trim().split('\n')[0])
  const base = `http://127.0.0.1:${info.port}`
  console.log(`  网关 + SSR 就绪：${base}`)

  const created = await fetch(`${base}/threads`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: 'title=任务管理验收',
    redirect: 'manual',
  })
  const location = created.headers.get('location') || ''
  check(created.status === 303 || created.status === 302, '创建会话成功', `HTTP ${created.status}`)
  const threadId = location.match(/\/threads\/([^?/]+)/)?.[1]
  check(Boolean(threadId), '拿到会话 id', location)
  if (!threadId) throw new Error('缺少会话 id，后续断言无法进行')

  const sent = await fetch(`${base}/threads/${threadId}/send`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: 'text=起一个后台任务',
    redirect: 'manual',
  })
  check(sent.status === 303 || sent.status === 302, '发送消息成功', `HTTP ${sent.status}`)

  const pageWithTask = await waitFor(async () => {
    const page = await fetch(`${base}/threads/${threadId}`)
    const html = await page.text()
    globalThis.__lastPage = html
    return html.includes('data-plugin-scope="task-manager"') && html.includes(COMMAND)
      ? html
      : null
  }, 30_000, '任务管理面板出现')
  check(true, '会话页出现 task-manager 面板并列出后台命令')
  check(pageWithTask.includes('PID '), '面板显示进程 PID')
  check(/\/tasks\/task_\d+\/kill/.test(pageWithTask), '运行中的任务带终止表单')
  check(
    pageWithTask.includes('/ext/task-manager/assets/task-manager.css'),
    '插件样式已挂载',
  )

  const taskId = pageWithTask.match(/\/tasks\/(task_\d+)\/kill/)?.[1]
  check(Boolean(taskId), '从表单解析出任务 id', taskId || '')

  if (taskId) {
    const killed = await fetch(`${base}/tasks/${taskId}/kill`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
        Referer: `${base}/threads/${threadId}`,
      },
      body: `thread_id=${encodeURIComponent(threadId)}`,
      redirect: 'manual',
    })
    check(killed.status === 303 || killed.status === 302, '终止动作返回重定向', `HTTP ${killed.status}`)

    const pageAfterKill = await waitFor(async () => {
      const page = await fetch(`${base}/threads/${threadId}`)
      const html = await page.text()
      return html.includes('data-plugin-scope="task-manager"') && html.includes('已结束')
        ? html
        : null
    }, 15_000, '任务状态刷新为已结束')
    check(!pageAfterKill.includes(`/tasks/${taskId}/kill`), '已结束的任务不再显示终止按钮')
    check(pageAfterKill.includes('已结束'), '面板显示已结束状态')
  }
} catch (error) {
  check(false, '实机验收未完成', String(error?.message || error))
  console.error(`  [debug] 模型请求次数=${modelRound}`)
  if (globalThis.__lastPage) {
    const text = globalThis.__lastPage.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').slice(0, 1200)
    console.error(`  [debug] 最后页面文本：${text}`)
  }
  if (gatewayOutput) console.error(gatewayOutput.slice(-4000))
} finally {
  await cleanup()
}

if (failures.length) {
  console.error(`\n${failures.length} 项失败：${failures.join('；')}`)
  process.exit(1)
}
console.log('\n任务管理插件实机验收通过。')
