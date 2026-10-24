// ssr-h 工作区树 / 添加 / 选择工作区的**实机验收**。
//
// 覆盖：
//   1. 侧栏树把不同工作区的会话分组、当前工作区标"当前"、空工作区不丢；
//   2. `GET /workspaces` 真的能浏览网关所在机器的目录；
//   3. `POST /workspaces/select` 能选新目录（没有会话就建一条）并落到该工作区；
//   4. 已有会话的工作区被选中时打开**已有**会话，而不是再造一条；
//   5. `POST /workspaces/mkdir` 新建目录后能继续浏览。
//
// 真二进制 + 真网关 + 真协议；不碰用户 `~/.a-da`。临时目录放在 `target/ssr-verify`
// （某些沙箱环境拒绝子进程写系统 TEMP，见 verify-ssr-streaming.mjs 的说明）。

import { mkdtempSync, mkdirSync, rmSync, existsSync } from 'node:fs'
import { join } from 'node:path'
import { spawn, spawnSync } from 'node:child_process'
import { createServer as createNetServer } from 'node:net'

const REPO = 'E:\\codes\\rust_projects\\a_da'
const GATEWAY = join(REPO, 'dist', 'ada-gateway.exe')
// ssr-h 的构建目录是 <repo>/cargo_target_ada（ssr-h/ 用 CARGO_TARGET_DIR=../ 编译）；
// 旧路径在仓库外一级，是历史上留下的陈旧二进制——只作兜底，别让它盖住新构建。
const SSR =
  process.env.SSR_H_EXE ||
  [
    join(REPO, 'cargo_target_ada', 'debug', 'ssr-h.exe'),
    join(REPO, '..', 'cargo_target_ada', 'debug', 'ssr-h.exe'),
  ].find((candidate) => existsSync(candidate)) ||
  join(REPO, 'cargo_target_ada', 'debug', 'ssr-h.exe')

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

const verifyRoot = join(REPO, 'target', 'ssr-verify-workspaces')
mkdirSync(verifyRoot, { recursive: true })
const appHome = mkdtempSync(join(verifyRoot, 'home-'))
const wsBase = mkdtempSync(join(verifyRoot, 'base-'))
const wsAlpha = join(wsBase, 'alpha')
const wsBeta = join(wsBase, 'beta')
const wsGamma = join(wsBase, 'gamma')
for (const dir of [wsAlpha, wsBeta, wsGamma]) mkdirSync(dir, { recursive: true })

const children = []
function start(name, exe, args, env = {}) {
  const child = spawn(exe, args, {
    cwd: REPO,
    env: { ...process.env, ...env },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  children.push({ name, child })
  child.output = ''
  child.stdout.on('data', (chunk) => (child.output += chunk.toString()))
  child.stderr.on('data', (chunk) => (child.output += chunk.toString()))
  return child
}

function waitFor(child, predicate, timeoutMs, label) {
  return new Promise((resolve, reject) => {
    const started = Date.now()
    const timer = setInterval(() => {
      if (predicate(child.output)) {
        clearInterval(timer)
        resolve(child.output)
      } else if (Date.now() - started > timeoutMs || child.exitCode !== null) {
        clearInterval(timer)
        reject(new Error(`等待 ${label} 超时：\n${child.output.slice(-3000)}`))
      }
    }, 100)
  })
}

function cleanup() {
  for (const { child } of children) {
    if (child.exitCode !== null) continue
    if (process.platform === 'win32') {
      spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore' })
    } else {
      child.kill('SIGKILL')
    }
  }
  if (!process.env.KEEP_SSR_TEMP) {
    for (const dir of [appHome, wsBase]) {
      try {
        rmSync(dir, { recursive: true, force: true })
      } catch {}
    }
    try {
      rmSync(verifyRoot, { recursive: true, force: true })
    } catch {}
  } else {
    console.log('  保留临时目录：', appHome, wsBase)
  }
}

const get = async (url) => {
  const response = await fetch(url)
  return { status: response.status, body: await response.text(), headers: response.headers }
}
const postForm = (url, fields) =>
  fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams(fields).toString(),
    redirect: 'manual',
  })

try {
  check(existsSync(GATEWAY), `网关二进制存在：${GATEWAY}`)
  check(existsSync(SSR), `ssr-h 二进制存在：${SSR}`)
  if (failures.length) throw new Error('缺少二进制')

  const gatewayPort = await freePort()
  const gateway = start('gateway', GATEWAY, ['--port', String(gatewayPort), '--workspace', wsBase], {
    A_DA_HOME: appHome,
  })
  const readyLine = await waitFor(
    gateway,
    (output) => output.includes('A_DA_GATEWAY_READY'),
    20_000,
    '网关就绪',
  )
  const gatewayUrl = `http://127.0.0.1:${JSON.parse(readyLine.split('A_DA_GATEWAY_READY')[1].trim().split('\n')[0]).port}`
  console.log(`  网关就绪：${gatewayUrl}`)

  const uiPort = await freePort()
  const ssr = start('ssr-h', SSR, [
    '--port',
    String(uiPort),
    '--gateway',
    gatewayUrl,
    '--workspace',
    wsAlpha,
    '--plugins-dir',
    join(REPO, 'ssr-h', 'frontend-plugins'),
  ])
  await waitFor(ssr, (output) => output.includes('ssr-h listening on'), 20_000, 'ssr-h 就绪')
  const ui = `http://127.0.0.1:${uiPort}`
  console.log(`  ssr-h 就绪：${ui}`)

  const createThread = async (workspace, title) => {
    const response = await fetch(
      `${gatewayUrl}/api/threads?workspace=${encodeURIComponent(workspace)}`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ title, workspace }),
      },
    )
    const body = await response.json().catch(() => ({}))
    return body.threadId || body.id || ''
  }

  const alphaThread = await createThread(wsAlpha, 'Alpha 会话')
  const betaThread = await createThread(wsBeta, 'Beta 会话')
  check(Boolean(alphaThread && betaThread), '两个工作区各建一条会话')
  if (!alphaThread || !betaThread) throw new Error('建会话失败')

  if (process.env.SSR_VERIFY_DEBUG) {
    for (const ws of [wsAlpha, wsBeta]) {
      const snapshot = await (
        await fetch(`${gatewayUrl}/api/snapshot?workspace=${encodeURIComponent(ws)}`)
      ).json()
      console.log(
        `  快照(${ws})：`,
        (snapshot.threads || []).map((t) => `${t.title}@${t.workspace}`).join(' | '),
      )
    }
  }

  // ── 1. 侧栏树 ──────────────────────────────────────────────────────────
  const page = await get(`${ui}/threads/${alphaThread}`)
  check(page.status === 200, '会话页可打开', `HTTP ${page.status}`)
  // v0.6 侧栏收进组件库后：列表容器是 ada-thread-list，其它工作区是 ada-ws 折叠组
  check(
    page.body.includes('id="thread-list"') && page.body.includes('class="ada-ws"'),
    '侧栏渲染工作区树',
  )
  check(page.body.includes('>alpha<'), '树里出现 alpha 工作区')
  check(page.body.includes('>beta<'), '树里出现 beta 工作区（跨工作区会话）')
  if (!page.body.includes('>beta<')) {
    console.log(
      '  树里的工作区：',
      (page.body.match(/ada-ws__name[^>]*>[^<]+/g) || []).join(' | '),
    )
    console.log('  会话标题：', (page.body.match(/ada-thread__title[^>]*>[^<]+/g) || []).join(' | '))
  }
  // 「当前」徽标已废除（侧栏单测专门守着）：当前工作区不再有组头，切换器就是它的表头
  check(page.body.includes('ada-ws-switch__name">alpha<'), '当前工作区有"当前"标记')
  // 新会话的选择挪进了空会话页：模式分段 + 工作区药丸；不再有新建弹窗
  check(
    !page.body.includes('data-modal="new-thread"') &&
      page.body.includes('ada-newconv__controls') &&
      page.body.includes(`action="/threads/${alphaThread}/mode"`) &&
      page.body.includes(`action="/threads/${alphaThread}/workspace"`),
    '空会话页渲染模式分段与工作区选择（无新建弹窗）',
  )
  check(
    page.body.includes(`/workspaces?bind=${alphaThread}`),
    '空会话页的「选择工作区」带 bind 上下文',
  )
  check(
    page.body.includes(`name="workspace" value="${wsAlpha}"`),
    '顶部"新会话"提交到当前工作区',
  )
  check(page.body.includes(`name="workspace" value="${wsBeta}"`), '分组"+"提交到它自己的工作区')
  check(page.body.includes('/workspaces"'), '侧栏提供"添加/切换工作区"入口')

  // ── 2. 目录浏览 ────────────────────────────────────────────────────────
  const picker = await get(`${ui}/workspaces?path=${encodeURIComponent(wsBase)}`)
  check(picker.status === 200, '工作区选择页可打开', `HTTP ${picker.status}`)
  check(picker.body.includes('使用此目录'), '选择页有"使用此目录"按钮')
  check(picker.body.includes('>alpha<') && picker.body.includes('>beta<') && picker.body.includes('>gamma<'), '目录列表列出三个子目录')
  check(picker.body.includes('最近工作区'), '选择页列出最近工作区')

  // ── 3. 选择新目录（没有会话 → 建一条）──────────────────────────────────
  const selected = await postForm(`${ui}/workspaces/select`, { path: wsGamma })
  const location = selected.headers.get('location') || ''
  check(selected.status === 303, '选择工作区后 303 跳转', `HTTP ${selected.status}`)
  check(location.startsWith('/threads/'), '跳转到该工作区的会话页', location)
  const gammaPage = location ? await get(`${ui}${location}`) : { body: '' }
  check(gammaPage.body.includes('>gamma<'), '新工作区出现在侧栏树里')
  check(gammaPage.body.includes('ada-ws-switch__name">gamma<'), '新工作区成为当前工作区')
  check(gammaPage.body.includes('新会话'), '没有会话的工作区被建了一条新会话')

  // ── 4. 选择已有会话的工作区：打开已有会话，不重复建 ─────────────────────
  const selectBeta = await postForm(`${ui}/workspaces/select`, { path: wsBeta })
  check(
    (selectBeta.headers.get('location') || '') === `/threads/${betaThread}?notice=workspace_selected`,
    '选择已有会话的工作区时打开那条会话',
    selectBeta.headers.get('location') || '',
  )

  // ── 5. 新建目录 ────────────────────────────────────────────────────────
  const mkdir = await postForm(`${ui}/workspaces/mkdir`, { parent: wsGamma, name: 'child' })
  check(mkdir.status === 303, '新建目录后 303 跳转', `HTTP ${mkdir.status}`)
  const childPage = await get(`${ui}${mkdir.headers.get('location') || ''}`)
  check(childPage.body.includes(`${wsGamma.replace(/\\/g, '/')}/child`), '新建目录后停留在该目录', childPage.body.slice(0, 200))
  check(childPage.body.includes('使用此目录'), '新建目录可直接作为工作区')

  // ── 6. 弹窗目录选择器（htmx 片段）────────────────────────────────────────
  const fragment = await get(
    `${ui}/workspaces/picker?bind=${alphaThread}&path=${encodeURIComponent(wsBase)}`,
  )
  check(
    fragment.status === 200 &&
      fragment.body.includes('wp-picker') &&
      fragment.body.includes('>alpha<') &&
      fragment.body.includes(`action="/threads/${alphaThread}/workspace"`),
    '弹窗片段列出目录并可绑定到该会话',
  )
  const pickerMkdir = await fetch(`${ui}/workspaces/picker/mkdir`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded',
      'HX-Request': 'true',
    },
    body: new URLSearchParams({
      parent: wsBase,
      name: 'picker-new',
      thread_id: alphaThread,
    }).toString(),
  })
  const pickerMkdirBody = await pickerMkdir.text()
  check(
    pickerMkdir.status === 200 &&
      pickerMkdirBody.includes('picker-new') &&
      pickerMkdirBody.includes('选定此工作区'),
    '弹窗内新建文件夹后进入新目录',
  )

  if (process.env.SSR_VERIFY_SERVE === '1') {
    // 手动浏览器验收：保留整套服务不退出，打印入口 URL。
    console.log(`  SERVE_URL=${ui}/threads/${alphaThread}`)
    await new Promise(() => {})
  }
} catch (error) {
  failures.push(String(error?.message || error))
  console.error(error)
} finally {
  cleanup()
}

console.log('')
if (failures.length) {
  console.error(`[FAIL] ssr-h 工作区验收未通过（${failures.length} 项）`)
  process.exit(1)
}
console.log('[PASS] ssr-h 工作区树 / 添加 / 选择均可用')
