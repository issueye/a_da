// 动态后端路由的**实机验收**：真网关 + 真 agent + 仓库里的 unified-demo 插件。
//
// 不启动假模型、不创建会话：路由是独立于模型轮次的后端数据面。
// 断言覆盖：路径参数 / 查询串、JSON 与表单请求体、敏感头不转发、text 响应、
// 404、HEAD，以及停用插件后入口立即消失。

import { mkdtempSync, rmSync, existsSync, mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawn, spawnSync } from 'node:child_process'
import { createServer as createNetServer } from 'node:net'

const REPO = 'E:\\codes\\rust_projects\\a_da'
const GATEWAY = join(REPO, '..', 'cargo_target_ada', 'debug', 'ada-gateway.exe')
const WEB_PLUGINS = join(REPO, 'crates', 'agent-web', 'frontend-plugins')
const UNIFIED_PLUGINS = join(REPO, 'plugins')

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
    await new Promise((resolve) => setTimeout(resolve, 200))
  }
  throw new Error(`${label} 超时（最后结果：${last}）`)
}

const appHome = mkdtempSync(join(tmpdir(), 'ada_route_home_'))
const workspace = mkdtempSync(join(tmpdir(), 'ada_route_ws_'))
mkdirSync(join(workspace, '.ada'), { recursive: true })

if (!existsSync(GATEWAY)) {
  console.error(`缺少已编译的网关：${GATEWAY}`)
  process.exit(2)
}

const port = await freePort()
const gateway = spawn(
  GATEWAY,
  ['--port', String(port), '--workspace', workspace, '--web-plugins-dir', WEB_PLUGINS],
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
  if (process.platform === 'win32' && gateway.exitCode === null) {
    spawnSync('taskkill', ['/PID', String(gateway.pid), '/T', '/F'], { stdio: 'ignore' })
  }
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

  const routeUrl = (path) => `${base}/ext/unified-demo/route/${path}`

  // 首次调用会顺手拉起 agent；agent 装载插件可能需要几秒，失败就重试。
  const hello = await waitFor(async () => {
    const response = await fetch(routeUrl('hello/ada?q=1&token=SECRET'))
    if (response.status !== 200) return null
    return { response, body: await response.json() }
  }, 25_000, 'GET /hello/:name')
  check(hello.response.status === 200, 'GET 动态路由返回 200')
  check(hello.body.hello === 'ada', '路径参数 :name 正确', JSON.stringify(hello.body))
  check(hello.body.q === '1', '查询串传入插件', JSON.stringify(hello.body))
  check(hello.body.query?.token === undefined, '网关 token 不出现在插件 query 里')
  check(typeof hello.body.at === 'number', '上下文 now 可用')

  const head = await fetch(routeUrl('hello/ada?q=1'), { method: 'HEAD' })
  check(head.status === 200, 'HEAD 回退到 GET 路由', `HTTP ${head.status}`)
  check((await head.text()) === '', 'HEAD 不返回响应体')

  const echo = await fetch(routeUrl('echo'), {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'X-Demo': 'yes',
      Cookie: 'session=secret',
      Authorization: 'Bearer secret',
    },
    body: JSON.stringify({ a: 1 }),
  })
  check(echo.status === 200, 'POST JSON 路由返回 200', `HTTP ${echo.status}`)
  const echoed = await echo.json()
  check(echoed.body?.a === 1, 'JSON 请求体传入插件', JSON.stringify(echoed))
  check(echoed.bodyKind === 'json', 'bodyKind=json')
  check(echoed.header === 'yes', '普通自定义头传入插件')
  check(echoed.headers?.cookie === undefined, 'Cookie 不转发给插件')
  check(echoed.headers?.authorization === undefined, 'Authorization 不转发给插件')

  const form = await fetch(routeUrl('echo'), {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: 'a=1&a=2',
  })
  const formEcho = await form.json()
  check(formEcho.bodyKind === 'form', '表单请求体标记为 form')
  check(
    Array.isArray(formEcho.body?.a) && formEcho.body.a.join(',') === '1,2',
    '重复表单键变成数组',
    JSON.stringify(formEcho.body),
  )

  const text = await fetch(routeUrl('text'))
  check(text.status === 200, 'text 响应返回 200')
  check((text.headers.get('content-type') || '').includes('text/plain'), 'text Content-Type 正确')
  check(text.headers.get('x-demo') === 'text', '自定义响应头生效')
  check((await text.text()) === 'unified-demo route ok', 'text 响应体正确')

  const sqliteFirst = await fetch(routeUrl('sqlite'))
  check(sqliteFirst.status === 200, 'SQLite 路由返回 200', `HTTP ${sqliteFirst.status}`)
  const sqliteFirstBody = await sqliteFirst.json()
  check(
    typeof sqliteFirstBody.total === 'number' && sqliteFirstBody.total >= 1,
    'SQLite 建表与写入生效',
    JSON.stringify(sqliteFirstBody),
  )
  check(
    typeof sqliteFirstBody.last?.id === 'number' && typeof sqliteFirstBody.last?.at === 'number',
    'SQLite 查询返回 last 行',
    JSON.stringify(sqliteFirstBody),
  )
  const sqliteSecond = await fetch(routeUrl('sqlite'))
  const sqliteSecondBody = await sqliteSecond.json()
  check(
    sqliteSecondBody.total === sqliteFirstBody.total + 1,
    'SQLite 文件在工作区内持续累加',
    `${JSON.stringify(sqliteFirstBody)} -> ${JSON.stringify(sqliteSecondBody)}`,
  )

  const execRoute = await fetch(routeUrl('exec'))
  check(execRoute.status === 200, 'child_process 路由返回 200', `HTTP ${execRoute.status}`)
  const execBody = await execRoute.json()
  check(
    String(execBody.output || '').includes('exec-ok'),
    'child_process.execFileSync 的 stdout 回传',
    JSON.stringify(execBody),
  )

  const missing = await fetch(routeUrl('nope'))
  check(missing.status === 404, '未声明的路由返回 404', `HTTP ${missing.status}`)
  const missingBody = await missing.json()
  check(String(missingBody.error || '').includes('路由不存在'), '404 带可读说明')

  const disabled = await fetch(`${base}/extensions/unified-demo/disable`, {
    method: 'POST',
    redirect: 'manual',
  })
  check(disabled.status === 303, '停用插件成功', `HTTP ${disabled.status}`)
  const afterDisable = await fetch(routeUrl('hello/ada'))
  check(afterDisable.status === 404, '停用后动态路由立即消失', `HTTP ${afterDisable.status}`)
} catch (error) {
  check(false, '实机验收未完成', String(error?.message || error))
  if (gatewayOutput) console.error(gatewayOutput.slice(-4000))
} finally {
  await cleanup()
}

if (failures.length) {
  console.error(`\n${failures.length} 项失败：${failures.join('；')}`)
  process.exit(1)
}
console.log('\n动态后端路由实机验收通过。')
