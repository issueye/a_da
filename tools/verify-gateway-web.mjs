// 网关内置 SSR 界面的**单进程**验收。
//
// 合并后的目标是"一个 ada-gateway 进程 + 一个端口 = 整站"：
//   /                         → SSR 聊天页
//   /static/*                 → 内嵌静态资源
//   /web-ui/frontend-plugins  → 前端插件自省（不再占用 /api/plugins）
//   /api/gateway/info         → 网关 API（不带 SSR 的 CSP）
//   /rpc                      → WebSocket（本脚本只验证它仍由网关路由，见 gateway_e2e）
//
// 不需要 ada-coding：页面在 agent 未就绪时会渲染"未连接"壳，这仍然证明根路径由 SSR 接管。

import { mkdtempSync, mkdirSync, rmSync, existsSync } from 'node:fs'
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
const WEB_PLUGINS = join(REPO, 'crates', 'agent-web', 'frontend-plugins')

const failures = []
const check = (ok, what, extra = '') => {
  console.log(`  ${ok ? '[PASS]' : '[FAIL]'} ${what}${ok || !extra ? '' : ` — ${extra}`}`)
  if (!ok) failures.push(what)
}

const children = []
function start(name, exe, args, env = {}) {
  const child = spawn(exe, args, {
    cwd: REPO,
    env: { ...process.env, ...env },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  children.push(child)
  let output = ''
  child.stdout.on('data', (chunk) => {
    output += chunk.toString()
    if (process.env.SSR_VERIFY_DEBUG) process.stdout.write(`[${name}] ${chunk}`)
  })
  child.stderr.on('data', (chunk) => {
    output += chunk.toString()
    if (process.env.SSR_VERIFY_DEBUG) process.stderr.write(`[${name}] ${chunk}`)
  })
  return {
    child,
    get output() {
      return output
    },
  }
}

function stopAll() {
  for (const child of children) {
    try {
      if (child.exitCode === null) child.kill()
    } catch {}
  }
}

async function waitFor(handle, predicate, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (predicate(handle.output)) return handle.output
    if (handle.child.exitCode !== null) {
      throw new Error(`${label} 提前退出：\n${handle.output}`)
    }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`${label} 超时：\n${handle.output}`)
}

async function freePort() {
  const server = createNetServer()
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address()
  await new Promise((resolve) => server.close(resolve))
  return port
}

const root = join(REPO, 'target', 'gateway-web-verify')
mkdirSync(root, { recursive: true })
const appHome = mkdtempSync(join(root, 'home-'))
const workspace = mkdtempSync(join(root, 'ws-'))

try {
  check(existsSync(GATEWAY), `网关二进制存在：${GATEWAY}`)
  if (failures.length) throw new Error('缺少二进制')

  const port = await freePort()
  const gateway = start(
    'gateway',
    GATEWAY,
    [
      '--port',
      String(port),
      '--workspace',
      workspace,
      '--web-ui',
      'ssr',
      '--web-plugins-dir',
      WEB_PLUGINS,
    ],
    { A_DA_HOME: appHome },
  )
  const ready = await waitFor(
    gateway,
    (output) => output.includes('A_DA_GATEWAY_READY'),
    20_000,
    '网关就绪',
  )
  const info = JSON.parse(ready.split('A_DA_GATEWAY_READY')[1].trim().split('\n')[0])
  const base = `http://127.0.0.1:${info.port}`
  console.log(`  网关 + SSR 就绪：${base}（webUi=${info.webUi}）`)
  check(info.webUi === 'ssr', 'READY 行报告 webUi=ssr', JSON.stringify(info))

  const rootPage = await fetch(`${base}/`)
  const rootBody = await rootPage.text()
  check(rootPage.status === 200, '根路径返回 200', `HTTP ${rootPage.status}`)
  check(
    (rootPage.headers.get('content-type') || '').includes('text/html'),
    '根路径是 HTML（不是 desktop-ui 的 index）',
    rootPage.headers.get('content-type') || '',
  )
  check(rootBody.includes('· a_da'), '根路径是 SSR 页面（标题含 a_da）')
  check(rootBody.includes('id="transcript"'), '根路径带对话区')

  const css = await fetch(`${base}/static/app.css`)
  check(css.status === 200, '内嵌静态资源可访问', `HTTP ${css.status}`)
  check(
    (css.headers.get('content-type') || '').includes('text/css'),
    '静态资源 Content-Type 正确',
    css.headers.get('content-type') || '',
  )

  const plugins = await fetch(`${base}/web-ui/frontend-plugins`)
  const pluginsBody = await plugins.text()
  check(plugins.status === 200, '前端插件自省端点 200', `HTTP ${plugins.status}`)
  check(pluginsBody.includes('"plugins"'), '自省端点返回插件快照')

  const api = await fetch(`${base}/api/gateway/info`)
  const apiBody = await api.text()
  check(api.status === 200, '网关 API 不受 SSR 影响', `HTTP ${api.status}`)
  check(apiBody.includes('protocolVersion'), 'API 返回网关信息')
  check(!api.headers.get('content-security-policy'), 'API 响应不带 SSR 的 CSP')

  const crossSite = await fetch(`${base}/threads`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded',
      Origin: 'http://evil.example',
    },
    body: 'title=x',
    redirect: 'manual',
  })
  check(crossSite.status === 403, 'SSR guard 拒绝跨站写请求', `HTTP ${crossSite.status}`)

  // 桌面端的「浏览器打开」会带网关 token；SSR 不需要，应 303 摘掉。
  const withToken = await fetch(`${base}/?token=gw-secret`, { redirect: 'manual' })
  check(withToken.status === 303, '带 token 的 GET 被 303 重定向', `HTTP ${withToken.status}`)
  check(
    (withToken.headers.get('location') || '') === '/',
    '重定向去掉了 URL 上的 token',
    withToken.headers.get('location') || '',
  )
} finally {
  stopAll()
  if (!process.env.KEEP_SSR_TEMP) {
    try {
      rmSync(workspace, { recursive: true, force: true })
      rmSync(appHome, { recursive: true, force: true })
    } catch {}
  } else {
    console.log('  保留临时目录：', appHome, workspace)
  }
}

if (failures.length) {
  console.error(`\n[FAIL] 网关内置 SSR 验收未通过（${failures.length} 项）`)
  process.exit(1)
}
console.log('\n[PASS] 网关内置 SSR：单进程单端口、根路径/静态/插件自省/API 隔离均可用')
