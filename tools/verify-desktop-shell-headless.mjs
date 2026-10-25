// Electron 桌面壳的**无头验收**（锁屏 / CI 环境可用）。
//
// 官方脚本 `verify-desktop-shell.ps1` 用真实鼠标点击标题栏像素；工作站锁屏时
// 无法把窗口顶到前台，那一步会直接失败。本脚本改用 Electron 的远程调试协议：
//
//   · `Input.dispatchMouseEvent` 真实命中渲染层（标题栏按钮、输入框、发送键）；
//   · `tools/window-state.ps1` 读原生窗口状态（最小化 / 最大化 / 还原）；
//   · `window.electronAPI` 读界面看到的窗口状态（按钮图标同步）；
//   · 真实 UI 发一条消息（假模型），再从标题栏关闭窗口，验证进程与网关清理。
//
// 它不替代官方脚本的**像素级**检查（按钮簇位置、图标截图对比），但覆盖
// 「窗口控制 / 状态同步 / 退出清理 / 发消息」这些行为断言，且不依赖前台焦点。
//
// 不碰用户 `~/.a-da`：临时 A_DA_HOME + 临时 Electron user-data-dir。

import { createServer } from 'node:http'
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from 'node:fs'
import { join } from 'node:path'
import { spawn, spawnSync } from 'node:child_process'
import { createServer as createNetServer } from 'node:net'

const REPO = 'E:\\codes\\rust_projects\\a_da'
const APP =
  process.env.A_DA_ELECTRON_EXE ||
  join(REPO, 'dist', 'electron', 'win-unpacked', 'ada.exe')

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

const root = join(REPO, 'target', 'desktop-shell-headless')
mkdirSync(root, { recursive: true })
const home = mkdtempSync(join(root, 'home-'))
const workspace = mkdtempSync(join(root, 'ws-'))
const profile = mkdtempSync(join(root, 'profile-'))

// ── 假模型：回答一句固定文本，供"从真实 UI 发消息"断言 ──────────────────
const modelRequests = []
const model = createServer((req, res) => {
  let body = ''
  req.on('data', (chunk) => (body += chunk))
  req.on('end', () => {
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
    send({ role: 'assistant', content: '' })
    send({ content: '无头验收回复：窗口与消息链路正常。' })
    send({}, 'stop')
    res.write('data: [DONE]\n\n')
    res.end()
  })
})
await new Promise((resolve) => model.listen(0, '127.0.0.1', resolve))
const modelPort = model.address().port

writeFileSync(
  join(home, 'config.json'),
  JSON.stringify(
    {
      activeProviderId: 'prov_stub',
      apiKey: 'stub-key',
      baseUrl: `http://127.0.0.1:${modelPort}/v1`,
      contextWindow: 32768,
      maxOutputTokens: 1024,
      model: 'stub-model',
      protocol: 'openai_chat',
      approval: 'auto',
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

const cdpPort = await freePort()
const child = spawn(
  APP,
  [
    `--remote-debugging-port=${cdpPort}`,
    '--workspace',
    workspace,
    '--user-data-dir',
    profile,
  ],
  {
    cwd: workspace,
    env: { ...process.env, A_DA_HOME: home },
    stdio: ['ignore', 'pipe', 'pipe'],
  },
)
let appOutput = ''
child.stdout.on('data', (chunk) => (appOutput += chunk.toString()))
child.stderr.on('data', (chunk) => (appOutput += chunk.toString()))

function killTree(pid) {
  try {
    spawnSync('taskkill', ['/pid', String(pid), '/T', '/F'], { stdio: 'ignore' })
  } catch {}
}

function cleanup() {
  if (process.env.VERIFY_DEBUG) {
    console.log('  --- app 输出末尾 ---')
    console.log(appOutput.slice(-3000))
  }
  if (child.exitCode === null) killTree(child.pid)
  try {
    model.close()
  } catch {}
  if (!process.env.KEEP_VERIFY_TEMP) {
    for (const dir of [home, workspace, profile]) {
      try {
        rmSync(dir, { recursive: true, force: true })
      } catch {}
    }
  } else {
    console.log('  保留临时目录：', home, workspace, profile)
  }
}

// ── CDP ────────────────────────────────────────────────────────────────────
let pageWs = null
let nextId = 1

async function connect(url) {
  const ws = new WebSocket(url)
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true })
    ws.addEventListener('error', reject, { once: true })
  })
  const pending = new Map()
  ws.addEventListener('message', (event) => {
    const msg = JSON.parse(event.data)
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg)
      pending.delete(msg.id)
    }
  })
  return {
    ws,
    send(method, params = {}) {
      const id = nextId++
      ws.send(JSON.stringify({ id, method, params }))
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error(`CDP 超时：${method}`)), 10_000)
        pending.set(id, (msg) => {
          clearTimeout(timer)
          if (msg.error) reject(new Error(`${method}: ${msg.error.message}`))
          else resolve(msg.result)
        })
      })
    },
  }
}

async function waitUntil(fn, timeoutMs, label) {
  const started = Date.now()
  while (Date.now() - started < timeoutMs) {
    try {
      if (await fn()) return true
    } catch {}
    await sleep(200)
  }
  throw new Error(`等待 ${label} 超时`)
}

async function evaluate(expression, awaitPromise = false) {
  const result = await pageWs.send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise,
  })
  if (result.exceptionDetails) {
    throw new Error(`页面求值失败：${result.exceptionDetails.text}`)
  }
  return result.result?.value
}

async function rectOf(label) {
  return evaluate(`(() => {
    const b = [...document.querySelectorAll('button[aria-label]')].find(
      (x) => x.getAttribute('aria-label') === ${JSON.stringify(label)}
    );
    if (!b) return null;
    const r = b.getBoundingClientRect();
    return { x: r.x + r.width / 2, y: r.y + r.height / 2, disabled: Boolean(b.disabled) };
  })()`)
}

async function clickLabel(label) {
  const rect = await rectOf(label)
  if (!rect) throw new Error(`找不到按钮：${label}`)
  await pageWs.send('Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x: rect.x,
    y: rect.y,
    button: 'left',
    clickCount: 1,
  })
  await pageWs.send('Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x: rect.x,
    y: rect.y,
    button: 'left',
    clickCount: 1,
  })
  await sleep(500)
}

async function bodyText() {
  return evaluate('document.body.innerText')
}

/// 原生窗口状态（锁屏下 `document.hidden` 恒为 true，不能用来判断最小化）。
function windowState(action = 'state') {
  const out = spawnSync(
    'pwsh',
    [
      '-NoProfile',
      '-File',
      join(REPO, 'tools', 'window-state.ps1'),
      '-ProcessId',
      String(child.pid),
      '-Action',
      action,
    ],
    { encoding: 'utf8' },
  )
  const line = (out.stdout || '')
    .trim()
    .split(/\r?\n/)
    .filter(Boolean)
    .at(-1) || '{}'
  try {
    return JSON.parse(line)
  } catch {
    return { found: false, raw: line }
  }
}

try {
  check(existsSync(APP), `Electron 打包产物存在：${APP}`)
  if (failures.length) throw new Error('缺少打包产物')

  // 1. 连接 CDP
  let target = null
  await waitUntil(
    async () => {
      try {
        const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()
        target = list.find((t) => t.type === 'page' && t.title.includes('a_da')) || list.find((t) => t.type === 'page')
        return Boolean(target)
      } catch {
        return false
      }
    },
    25_000,
    'CDP 页面目标',
  )
  pageWs = await connect(target.webSocketDebuggerUrl)
  await pageWs.send('Runtime.enable')
  await pageWs.send('Page.enable')

  // Browser 域在 Electron 的 page 会话里可用（browser 端点反而没有 getWindowForTarget）。

  // 2. 界面就绪 + 网关在线
  await waitUntil(
    () =>
      evaluate(
        'Boolean(window.electronAPI) && Boolean(document.querySelector(\'button[aria-label="最大化"]\')) && document.body.innerText.includes("网关在线")',
      ),
    30_000,
    '界面就绪与网关握手',
  )
  console.log('  界面已就绪，网关在线')

  const initial = windowState()
  check(initial.found === true, '找到 Electron 主窗口（原生句柄）', JSON.stringify(initial))
  check(initial.zoomed === false, '初始窗口不是最大化状态')

  // 3. 最大化 / 还原 + 按钮状态同步
  check((await rectOf('最大化')) !== null, '标题栏暴露「最大化」按钮')
  await clickLabel('最大化')
  await waitUntil(
    () => evaluate('window.electronAPI.isWindowMaximized()', true),
    5_000,
    '窗口最大化',
  )
  const maxState = windowState()
  check(maxState.zoomed === true, '点击「最大化」→ 原生窗口已最大化', JSON.stringify(maxState))
  await waitUntil(() => rectOf('向下还原').then(Boolean), 5_000, '按钮切到「向下还原」')
  check(true, '最大化后按钮图标状态同步为「向下还原」')

  await clickLabel('向下还原')
  await waitUntil(
    () => evaluate('window.electronAPI.isWindowMaximized()', true).then((v) => v === false),
    5_000,
    '窗口还原',
  )
  const restoredState = windowState()
  check(restoredState.zoomed === false, '点击「向下还原」→ 窗口已还原', JSON.stringify(restoredState))
  await waitUntil(() => rectOf('最大化').then(Boolean), 5_000, '按钮切回「最大化」')
  check(true, '还原后按钮图标状态同步为「最大化」')

  // 4. 应用内弹窗打开时，标题栏仍可点（z-[100] 红线）
  await clickLabel('设置中心')
  await waitUntil(
    () => evaluate('Boolean(document.querySelector(".fixed.inset-0.z-50"))'),
    5_000,
    '设置弹窗打开',
  )
  check(true, '设置弹窗已打开（fixed inset-0 z-50）')
  await clickLabel('最大化')
  await waitUntil(
    () => evaluate('window.electronAPI.isWindowMaximized()', true),
    5_000,
    '弹窗打开时最大化',
  )
  check(true, '弹窗打开时点「最大化」仍生效（标题栏未被模态层盖住）')
  await clickLabel('向下还原')
  await waitUntil(
    () => evaluate('window.electronAPI.isWindowMaximized()', true).then((v) => v === false),
    5_000,
    '弹窗打开时还原',
  )
  check(true, '弹窗打开时点「向下还原」仍生效')
  // 关掉设置弹窗（点弹窗头部第一个按钮，不依赖 Esc）。
  await evaluate(`(() => {
    const modal = document.querySelector('.fixed.inset-0.z-50');
    const btn = modal && modal.querySelector('button');
    if (btn) btn.click();
    return Boolean(btn);
  })()`)
  await waitUntil(
    () => evaluate('!document.querySelector(".fixed.inset-0.z-50")'),
    5_000,
    '设置弹窗关闭',
  )

  // 5. 最小化 / 恢复（按钮触发，原生 IsIconic 读状态）
  await clickLabel('最小化')
  let minimized = false
  for (let i = 0; i < 25; i += 1) {
    if (windowState().iconic === true) {
      minimized = true
      break
    }
    await sleep(200)
  }
  check(minimized, '点击「最小化」→ 原生窗口已最小化')
  const restoredNative = windowState('restore')
  check(restoredNative.iconic === false, '窗口已恢复可见', JSON.stringify(restoredNative))

  // 6. 从真实 UI 发一条消息
  await waitUntil(
    () => evaluate('Boolean(document.querySelector("textarea"))'),
    10_000,
    '输入框出现',
  )
  await evaluate('document.querySelector("textarea").focus()')
  await pageWs.send('Input.insertText', { text: '无头验收：请回复一句话。' })
  await waitUntil(
    () => evaluate('document.querySelector("textarea").value.includes("无头验收")'),
    5_000,
    '输入框收到文本',
  )
  const sendRect = await rectOf('发送')
  check(Boolean(sendRect) && !sendRect.disabled, '发送按钮可用')
  await clickLabel('发送')
  await waitUntil(
    () => bodyText().then((text) => text.includes('无头验收回复')),
    30_000,
    '助手回复上屏',
  )
  check(true, '从真实 UI 发消息 → 助手回复上屏')
  check(modelRequests.length >= 1, '假模型收到请求', `实际 ${modelRequests.length} 次`)

  // 7. 关闭窗口 + 退出清理
  const report = await evaluate('window.electronAPI.getProcessReport()', true)
  const gatewayPid = report?.processes?.find((p) => p.name.startsWith('ada-gateway'))?.pid || null
  check(Boolean(gatewayPid), '关闭前报告了网关 pid', String(gatewayPid))

  await clickLabel('关闭窗口')
  let appGone = false
  for (let i = 0; i < 60; i += 1) {
    if (child.exitCode !== null) {
      appGone = true
      break
    }
    await sleep(250)
  }
  check(appGone, '点击「关闭窗口」→ 桌面端主进程已退出')
  if (gatewayPid) {
    let gatewayGone = false
    for (let i = 0; i < 40; i += 1) {
      try {
        process.kill(gatewayPid, 0)
      } catch {
        gatewayGone = true
        break
      }
      await sleep(250)
    }
    check(gatewayGone, '关窗后没有遗留 ada-gateway 孤儿进程')
  }
} finally {
  cleanup()
}

if (failures.length) {
  console.error(`\n[FAIL] Electron 无头验收未通过（${failures.length} 项）`)
  process.exit(1)
}
console.log('\n[PASS] Electron 无头验收：窗口控制 / 状态同步 / 发消息 / 退出清理全部通过')
