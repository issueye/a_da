// Electron **默认 SSR 界面**的端到端冒烟（锁屏/CI 可用）。
//
// 与 `verify-desktop-shell-headless.mjs` 的分工：
//   · 那个脚本验证 `--ui=desktop` 的 React 自绘标题栏；
//   · 本脚本验证默认 `--ui=ssr`：Electron 等网关就绪后加载 `http://127.0.0.1:<port>/`，
//     preload 在 SSR 顶栏注入无边框窗口控制，消息走 SSR 表单 → 网关 → agent。
//
// 不碰用户 `~/.a-da`：临时 A_DA_HOME + 临时 workspace + 临时 Electron user-data-dir。

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

const root = join(REPO, 'target', 'electron-ssr-verify')
mkdirSync(root, { recursive: true })
const home = mkdtempSync(join(root, 'home-'))
const workspace = mkdtempSync(join(root, 'ws-'))
const profile = mkdtempSync(join(root, 'profile-'))

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
    send({ content: 'Electron SSR 回复：页面与消息链路正常。' })
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
  const line =
    (out.stdout || '')
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

let wsClient = null
let nextId = 1
async function cdpConnect(url) {
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
  wsClient = ws
  return {
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

async function evaluate(cdp, expression, awaitPromise = false) {
  const result = await cdp.send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise,
  })
  if (result.exceptionDetails) {
    throw new Error(`页面求值失败：${result.exceptionDetails.text}`)
  }
  return result.result?.value
}

async function rectOf(cdp, label) {
  return evaluate(
    cdp,
    `(() => {
      const b = [...document.querySelectorAll('button[aria-label], summary[aria-label]')].find(
        (x) => x.getAttribute('aria-label') === ${JSON.stringify(label)}
      );
      if (!b) return null;
      const r = b.getBoundingClientRect();
      return { x: r.x + r.width / 2, y: r.y + r.height / 2, disabled: Boolean(b.disabled) };
    })()`,
  )
}

async function clickLabel(cdp, label) {
  const rect = await rectOf(cdp, label)
  if (!rect) throw new Error(`找不到按钮：${label}`)
  await cdp.send('Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x: rect.x,
    y: rect.y,
    button: 'left',
    clickCount: 1,
  })
  await cdp.send('Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x: rect.x,
    y: rect.y,
    button: 'left',
    clickCount: 1,
  })
  await sleep(350)
}

async function clickSelector(cdp, selector) {
  const rect = await evaluate(
    cdp,
    `(() => {
      const e = document.querySelector(${JSON.stringify(selector)});
      if (!e) return null;
      const r = e.getBoundingClientRect();
      return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
    })()`,
  )
  if (!rect) throw new Error(`找不到元素：${selector}`)
  await cdp.send('Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x: rect.x,
    y: rect.y,
    button: 'left',
    clickCount: 1,
  })
  await cdp.send('Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x: rect.x,
    y: rect.y,
    button: 'left',
    clickCount: 1,
  })
  await sleep(350)
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

try {
  check(existsSync(APP), `Electron 打包产物存在：${APP}`)
  if (failures.length) throw new Error('缺少打包产物')

  let target = null
  await waitUntil(
    async () => {
      try {
        const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()
        target =
          list.find((t) => t.type === 'page' && /^http:\/\/127\.0\.0\.1:/.test(t.url)) ||
          list.find((t) => t.type === 'page')
        return Boolean(target && /^http:\/\/127\.0\.0\.1:/.test(target.url))
      } catch {
        return false
      }
    },
    30_000,
    'Electron 加载 SSR 页面',
  )
  console.log(`  Electron 页面：${target.url}`)
  const cdp = await cdpConnect(target.webSocketDebuggerUrl)
  await cdp.send('Runtime.enable')
  await cdp.send('Page.enable')

  await waitUntil(
    () => evaluate(cdp, 'Boolean(document.querySelector("#transcript")) && Boolean(document.querySelector("textarea"))'),
    20_000,
    'SSR 对话区与输入框',
  )
  check(true, 'Electron 加载的是 SSR 页面（对话区 + 输入框）')
  const pageUrl = await evaluate(cdp, 'location.href')
  check(/^http:\/\/127\.0\.0\.1:\d+\/$/.test(pageUrl), '页面来源是网关根路径（不是 file://）', pageUrl)
  check((await evaluate(cdp, 'document.title')).includes('a_da'), 'SSR 页面标题包含 a_da')
  check(
    await evaluate(cdp, `Boolean(document.querySelector('button[aria-label="最大化"]'))`),
    'SSR 顶栏注入了无边框窗口控制（最小化/最大化/关闭）',
  )

  // 无边框窗口控制：按钮走 preload IPC，原生状态用 IsIconic/IsZoomed 复核。
  const initial = windowState()
  check(initial.found === true && initial.zoomed === false, '找到窗口且未最大化', JSON.stringify(initial))

  await clickLabel(cdp, '最大化')
  await waitUntil(
    () => evaluate(cdp, 'window.electronAPI.isWindowMaximized()', true),
    5_000,
    '窗口最大化',
  )
  check(windowState().zoomed === true, '点「最大化」→ 窗口已最大化')
  await waitUntil(
    () => evaluate(cdp, `Boolean(document.querySelector('button[aria-label="向下还原"]'))`),
    5_000,
    '按钮切到「向下还原」',
  )
  check(true, '最大化后按钮状态同步为「向下还原」')

  await clickLabel(cdp, '向下还原')
  await waitUntil(
    () => evaluate(cdp, 'window.electronAPI.isWindowMaximized()', true).then((v) => v === false),
    5_000,
    '窗口还原',
  )
  check(windowState().zoomed === false, '点「向下还原」→ 窗口已还原')

  await clickLabel(cdp, '最小化')
  let minimized = false
  for (let i = 0; i < 25; i += 1) {
    if (windowState().iconic === true) {
      minimized = true
      break
    }
    await sleep(200)
  }
  check(minimized, '点「最小化」→ 窗口已最小化')
  const shown = windowState('restore')
  check(shown.iconic === false, '窗口恢复可见', JSON.stringify(shown))

  // 建一条会话，直接导航到 SSR 会话页（避免依赖侧栏点击的时序）
  const gatewayPort = Number(new URL(pageUrl).port)
  const created = await fetch(
    `http://127.0.0.1:${gatewayPort}/api/threads?workspace=${encodeURIComponent(workspace)}`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ title: 'Electron SSR 验收', workspace }),
    },
  )
  const createdBody = await created.json().catch(() => ({}))
  const threadId = createdBody.threadId || createdBody.id
  check(Boolean(threadId), '通过网关创建会话', JSON.stringify(createdBody).slice(0, 200))
  if (!threadId) throw new Error('没有 threadId')

  await cdp.send('Page.navigate', {
    url: `http://127.0.0.1:${gatewayPort}/threads/${threadId}`,
  })
  await waitUntil(
    () =>
      evaluate(
        cdp,
        'Boolean(document.querySelector("textarea")) && Boolean(document.querySelector(\'button[aria-label="发送"]\'))',
      ),
    20_000,
    'SSR 会话页输入框与发送键',
  )

  // 1. 审批档位弹层：每个选项都有 SVG 图标（4 图标 + 当前项勾 + 药丸 + caret）。
  const approvalPicker = await evaluate(
    cdp,
    'document.querySelector("#approval-picker")?.outerHTML || ""',
  )
  check(
    (approvalPicker.match(/<svg/g) || []).length >= 5,
    '审批档位弹层每个选项都带 SVG 图标',
    `${(approvalPicker.match(/<svg/g) || []).length} 个 svg`,
  )
  check(
    !(await evaluate(cdp, 'document.body.innerText')).includes('TSX 插槽'),
    '工具栏不再显示「TSX 插槽」演示胶囊',
  )

  // 2. 最窄 450px：侧栏变抽屉、无横向溢出。
  await cdp.send('Emulation.setDeviceMetricsOverride', {
    width: 450,
    height: 800,
    deviceScaleFactor: 1,
    mobile: false,
  })
  await sleep(400)
  const narrow = await evaluate(
    cdp,
    '({ inner: window.innerWidth, scroll: document.documentElement.scrollWidth, body: document.body.scrollWidth, menu: Boolean(document.querySelector(".ada-menu-toggle")) && getComputedStyle(document.querySelector(".ada-menu-toggle")).display !== "none" })',
  )
  check(narrow.inner === 450, '450px 视口生效', JSON.stringify(narrow))
  check(narrow.scroll <= narrow.inner + 1, '450px 下无横向溢出', JSON.stringify(narrow))
  check(narrow.menu === true, '450px 下侧栏折叠为抽屉按钮', JSON.stringify(narrow))

  // 抽屉是 z-index:60 的全高浮层：顶栏必须不透明且层级更高，
  // 否则抽屉品牌会透上来和标题重叠，菜单按钮也会被盖住点不到。
  await clickSelector(cdp, '.ada-menu-toggle')
  await waitUntil(
    () => evaluate(cdp, `document.querySelector('.ada-sidebar')?.classList.contains('is-open')`),
    5_000,
    '侧栏抽屉打开',
  )
  const layers = await evaluate(
    cdp,
    `(() => {
      const top = getComputedStyle(document.querySelector('.ada-topbar'));
      const side = getComputedStyle(document.querySelector('.ada-sidebar'));
      const b = document.querySelector('.ada-menu-toggle').getBoundingClientRect();
      const hit = document.elementFromPoint(b.x + b.width / 2, b.y + b.height / 2);
      return {
        topZ: Number(top.zIndex),
        sideZ: Number(side.zIndex),
        bg: top.backgroundColor,
        menuHit: Boolean(hit && hit.closest('.ada-menu-toggle')),
      };
    })()`,
  )
  check(layers.topZ > layers.sideZ, '顶栏层级高于侧栏抽屉', JSON.stringify(layers))
  check(layers.bg !== 'rgba(0, 0, 0, 0)', '顶栏不透明，抽屉品牌不会透上来', layers.bg)
  check(layers.menuHit === true, '抽屉打开时菜单按钮仍可点击', JSON.stringify(layers))
  await clickSelector(cdp, '.ada-menu-toggle')
  await waitUntil(
    () => evaluate(cdp, `!document.querySelector('.ada-sidebar')?.classList.contains('is-open')`),
    5_000,
    '侧栏抽屉关闭',
  )

  await cdp.send('Emulation.clearDeviceMetricsOverride')
  await sleep(300)

  // 3. 进程管理浮窗（frontend-plugins/process-manager 的 topbar.right 插槽）。
  check(
    await evaluate(
      cdp,
      `Boolean(document.querySelector('[data-plugin-scope="process-manager"] #process-manager'))`,
    ),
    '顶栏由插件提供进程管理入口',
  )
  // 🔴 顶栏在 Electron 里是拖拽区；summary 必须显式 no-drag，
  // 否则真实鼠标点击会变成拖窗口（CDP 注入点击绕过拖拽区，测不出来）。
  check(
    (await evaluate(
      cdp,
      `getComputedStyle(document.querySelector('#process-manager > summary')).getPropertyValue('-webkit-app-region').trim()`,
    )) === 'no-drag',
    '进程管理入口退出 Electron 拖拽区（真实点击可展开）',
  )
  await clickLabel(cdp, '进程管理')
  await waitUntil(
    () => evaluate(cdp, `document.querySelector("#process-manager")?.open === true`),
    5_000,
    '进程管理浮窗打开',
  )
  await waitUntil(
    async () => {
      const text = await evaluate(
        cdp,
        'document.querySelector("#process-manager")?.innerText || ""',
      )
      return text.includes('ada-gateway') && text.includes('ada-coding')
    },
    15_000,
    '进程列表渲染',
  )
  const processText = await evaluate(
    cdp,
    'document.querySelector("#process-manager")?.innerText || ""',
  )
  check(
    processText.includes('AGENT 实例') && processText.includes('PID'),
    '浮窗显示网关与 AGENT 实例',
    processText.slice(0, 200),
  )
  check(!processText.includes('token='), '进程面板不泄漏 token')

  // 弹窗是页面内的 z-index:90 层；标题栏窗口按钮必须仍可点（z-index:100）。
  await clickLabel(cdp, '最大化')
  await waitUntil(
    () => evaluate(cdp, 'window.electronAPI.isWindowMaximized()', true),
    5_000,
    '弹窗打开时最大化',
  )
  check(true, '进程浮窗打开时「最大化」仍生效（标题栏未被模态层盖住）')
  await clickLabel(cdp, '向下还原')
  await waitUntil(
    () => evaluate(cdp, 'window.electronAPI.isWindowMaximized()', true).then((v) => v === false),
    5_000,
    '弹窗打开时还原',
  )

  await cdp.send('Input.dispatchKeyEvent', {
    type: 'keyDown',
    key: 'Escape',
    code: 'Escape',
    windowsVirtualKeyCode: 27,
  })
  await cdp.send('Input.dispatchKeyEvent', {
    type: 'keyUp',
    key: 'Escape',
    code: 'Escape',
    windowsVirtualKeyCode: 27,
  })
  await clickLabel(cdp, '进程管理')
  await waitUntil(
    () => evaluate(cdp, `document.querySelector("#process-manager")?.open === false`),
    5_000,
    '进程浮窗关闭',
  )

  await evaluate(cdp, 'document.querySelector("textarea").focus()')
  await cdp.send('Input.insertText', { text: 'Electron SSR 请回复一句话。' })
  await waitUntil(
    () => evaluate(cdp, 'document.querySelector("textarea").value.includes("Electron SSR")'),
    5_000,
    '输入框收到文本',
  )
  const rect = await evaluate(
    cdp,
    `(() => {
      const b = document.querySelector('button[aria-label="发送"]');
      if (!b) return null;
      const r = b.getBoundingClientRect();
      return { x: r.x + r.width / 2, y: r.y + r.height / 2, disabled: Boolean(b.disabled) };
    })()`,
  )
  check(rect && !rect.disabled, 'SSR 发送键可用')
  await cdp.send('Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x: rect.x,
    y: rect.y,
    button: 'left',
    clickCount: 1,
  })
  await cdp.send('Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x: rect.x,
    y: rect.y,
    button: 'left',
    clickCount: 1,
  })
  await waitUntil(
    async () => {
      try {
        return (await evaluate(cdp, 'document.body.innerText')).includes('Electron SSR 回复')
      } catch {
        return false
      }
    },
    30_000,
    'SSR 助手回复上屏',
  )
  check(true, 'SSR 表单发送 → 助手回复上屏')
  check(modelRequests.length >= 1, '假模型收到请求', `实际 ${modelRequests.length} 次`)
  check(
    await evaluate(cdp, `Boolean(document.querySelector('button[aria-label="关闭窗口"]'))`),
    '整页跳转后窗口控制仍被重新注入',
  )

  const report = await evaluate(cdp, 'window.electronAPI.getProcessReport()', true)
  const gatewayPid = report?.processes?.find((p) => p.name.startsWith('ada-gateway'))?.pid || null
  check(Boolean(gatewayPid), '关闭前报告了网关 pid', String(gatewayPid))
  // 关闭会立刻销毁页面，CDP 的响应不会回来；点击后只等进程退出。
  try {
    await clickLabel(cdp, '关闭窗口')
  } catch {
    // 页面已销毁 / WebSocket 已断开都算正常
  }
  let appGone = false
  for (let i = 0; i < 60; i += 1) {
    if (child.exitCode !== null) {
      appGone = true
      break
    }
    await sleep(250)
  }
  check(appGone, '关闭窗口 → 桌面端主进程已退出')
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
  try {
    wsClient?.close()
  } catch {}
  cleanup()
}

if (failures.length) {
  console.error(`\n[FAIL] Electron SSR 验收未通过（${failures.length} 项）`)
  process.exit(1)
}
console.log('\n[PASS] Electron 默认 SSR：加载 / 原生窗口控制 / 发消息 / 关闭清理全部通过')
