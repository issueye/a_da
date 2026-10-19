import { app, BrowserWindow, ipcMain, dialog, screen } from 'electron'
import * as path from 'path'
import * as fs from 'fs'
import { spawn, execSync, type ChildProcess } from 'child_process'
import * as readline from 'readline'
import * as http from 'http'
import type { DesktopConfig, CoreInfo, ProcessReport, ManagedProcessInfo } from './types.js'

/**
 * 🔴 解析随应用一起打包的资源路径（**不要**直接用 `__dirname`）。
 *
 * `bun build --target node --format cjs`（`bun run electron:build:main`）会把
 * `__dirname` **硬编码成构建机上的源码目录**（产物里是
 * `var __dirname = "<repo>/electron"`）。于是 `path.join(__dirname, 'preload.cjs')`
 * 指向 `electron/preload.cjs`——那个文件根本不存在，preload 静默加载失败，
 * `window.electronAPI` 变成 undefined：界面既拿不到网关端口（显示"未连接"），
 * 窗口最小化/最大化/关闭也全部变成空操作。
 *
 * 所以：按候选列表解析 + **真的检查文件存在**（asar 内部路径也被 Electron 的 fs 补丁
 * 支持），`app.getAppPath()` 在 dev 是仓库根、打包后是 `resources/app.asar`，两边都对。
 */
function resolveAppPath(...relParts: string[]): string | null {
  const rel = path.join(...relParts)
  const candidates = [
    path.join(app.getAppPath(), rel),
    process.resourcesPath ? path.join(process.resourcesPath, rel) : '',
    path.join(__dirname, '..', rel),
    path.join(__dirname, rel),
  ].filter(Boolean) as string[]
  for (const candidate of candidates) {
    try {
      if (fs.existsSync(candidate)) return candidate
    } catch {
      /* 候选路径不可读就试下一个 */
    }
  }
  return null
}

// 1. 单实例互斥锁判定
appendLauncherLog(`[启动] a_da 桌面宿主启动 argv=${process.argv.slice(1).join(' ') || '(无参数)'}`)
const gotTheLock = app.requestSingleInstanceLock()
if (!gotTheLock) {
  appendLauncherLog('[启动] 已有实例在运行，本次启动退出（新实例只负责把既有窗口顶到前台）')
  app.quit()
  process.exit(0)
}

let mainWindow: BrowserWindow | null = null

// 2. 命令行参数解析
interface CliArgs {
  connect?: string
  token?: string
  workspace: string
  gatewayBin?: string
}

function parseCliArgs(): CliArgs {
  const args = process.argv.slice(1)
  const result: CliArgs = { workspace: '' }

  for (let i = 0; i < args.length; i++) {
    const arg = args[i]
    if (arg === '--connect' && i + 1 < args.length) {
      result.connect = args[++i]
    } else if (arg.startsWith('--connect=')) {
      result.connect = arg.slice('--connect='.length)
    } else if (arg === '--token' && i + 1 < args.length) {
      result.token = args[++i]
    } else if (arg.startsWith('--token=')) {
      result.token = arg.slice('--token='.length)
    } else if (arg === '--workspace' && i + 1 < args.length) {
      result.workspace = args[++i]
    } else if (arg.startsWith('--workspace=')) {
      result.workspace = arg.slice('--workspace='.length)
    } else if (arg === '--gateway-bin' && i + 1 < args.length) {
      result.gatewayBin = args[++i]
    } else if (arg.startsWith('--gateway-bin=')) {
      result.gatewayBin = arg.slice('--gateway-bin='.length)
    }
  }

  return result
}

// 3. 应用根目录与持久化配置
function getAppHome(): string {
  if (process.env.A_DA_HOME && process.env.A_DA_HOME.trim()) {
    return path.resolve(process.env.A_DA_HOME.trim())
  }
  const userProfile = process.env.USERPROFILE || process.env.HOME || ''
  return userProfile ? path.join(userProfile, '.a-da') : path.resolve('.a-da')
}

function getDesktopConfigPath(): string {
  return path.join(getAppHome(), 'desktop_launcher.json')
}

function loadDesktopConfig(): DesktopConfig {
  try {
    const cfgPath = getDesktopConfigPath()
    if (fs.existsSync(cfgPath)) {
      const content = fs.readFileSync(cfgPath, 'utf-8')
      return JSON.parse(content)
    }
  } catch (err) {
    console.warn('[DesktopConfig] 加载配置失败:', err)
  }
  return {}
}

function saveDesktopConfig(cfg: DesktopConfig): void {
  try {
    const cfgPath = getDesktopConfigPath()
    fs.mkdirSync(path.dirname(cfgPath), { recursive: true })
    fs.writeFileSync(cfgPath, JSON.stringify(cfg, null, 2), 'utf-8')
  } catch (err) {
    console.error('[DesktopConfig] 保存配置失败:', err)
  }
}

// 4. 日志记录
function appendLauncherLog(line: string): void {
  try {
    const logPath = path.join(getAppHome(), 'launcher.log')
    fs.mkdirSync(path.dirname(logPath), { recursive: true })
    fs.appendFileSync(logPath, `[${new Date().toISOString()}] ${line}\n`, 'utf-8')
  } catch {}
}

// 5. 探测二进制可执行文件
function findBinary(baseName: string, customBin?: string | null): string | null {
  if (customBin && customBin.trim()) {
    const p = path.resolve(customBin.trim())
    if (fs.existsSync(p)) return p
  }

  const exeName = process.platform === 'win32' ? `${baseName}.exe` : baseName

  // 候选 1: 打包资源路径 (resources/bin or resources)
  if ((process as any).resourcesPath) {
    const inResBin = path.join((process as any).resourcesPath, 'bin', exeName)
    if (fs.existsSync(inResBin)) return inResBin
    const inRes = path.join((process as any).resourcesPath, exeName)
    if (fs.existsSync(inRes)) return inRes
  }

  // 候选 2: 当前可执行文件同目录
  const currentExeDir = path.dirname(process.execPath)
  const nextToExe = path.join(currentExeDir, exeName)
  if (fs.existsSync(nextToExe)) return nextToExe

  // 候选 3: 开发阶段 target / dist 目录
  const appRootDir = app.getAppPath()
  const candidateDirs: string[] = []

  if (process.env.CARGO_TARGET_DIR) {
    candidateDirs.push(path.join(process.env.CARGO_TARGET_DIR, 'release'))
    candidateDirs.push(path.join(process.env.CARGO_TARGET_DIR, 'debug'))
  }

  candidateDirs.push(
    path.resolve(appRootDir, '../cargo_target_ada/release'),
    path.resolve(appRootDir, '../cargo_target_ada/debug'),
    path.resolve(appRootDir, 'dist'),
    path.resolve(appRootDir, 'target/release'),
    path.resolve(appRootDir, 'target/debug')
  )

  for (const dir of candidateDirs) {
    const candidate = path.join(dir, exeName)
    if (fs.existsSync(candidate)) {
      return candidate
    }
  }

  return null
}

/**
 * 前端产物目录（供网关 `--ui-dir` 使用）。
 *
 * 打包后网关读不到 asar 里的资源，所以 electron-builder 把 `desktop-ui/dist`
 * 额外复制到了 `resources/bin/ui`；开发时直接用仓库里的 dist。
 * 只认**含 index.html** 的目录，与网关侧 `StaticSite::resolve` 的判据保持一致。
 */
function findUiDir(): string | null {
  const candidates = [
    process.resourcesPath ? path.join(process.resourcesPath, 'bin', 'ui') : '',
    resolveAppPath('desktop-ui', 'dist') ?? '',
    path.resolve(app.getAppPath(), '../desktop-ui/dist'),
  ].filter(Boolean) as string[]

  for (const dir of candidates) {
    try {
      if (fs.existsSync(path.join(dir, 'index.html'))) return dir
    } catch {
      /* 路径不可读就试下一个 */
    }
  }
  return null
}

// 6. 进程状态与看门狗
interface CoreState {
  alive: boolean
  mode: 'gateway'
  port: number
  token: string
  url: string
  childPid: number | null
  error: string | null
}

const coreState: CoreState = {
  alive: false,
  mode: 'gateway',
  port: 0,
  token: '',
  url: '',
  childPid: null,
  error: null,
}

let childProcess: ChildProcess | null = null
let resolveReadySignal: () => void = () => {}
/**
 * 「网关已就绪」信号。
 *
 * 🔴 必须是**可重新武装**的：`get_core_info` 会 await 它，而重启网关时旧 promise
 * 早已兑现——不换一把新的，重启后界面就会拿着过期信号去连旧端口。
 */
let readySignalPromise: Promise<void> = new Promise<void>((resolve) => {
  resolveReadySignal = resolve
})

function resetReadySignal(): void {
  readySignalPromise = new Promise<void>((resolve) => {
    resolveReadySignal = resolve
  })
}

function extractPortFromUrl(url: string): number | null {
  try {
    const match = url.match(/:(\d+)(\/|$)/)
    if (match) return parseInt(match[1], 10)
  } catch {}
  return null
}

function isPidAlive(pid: number | null | undefined): boolean {
  if (!pid || pid <= 0) return false
  try {
    process.kill(pid, 0)
    return true
  } catch (err: any) {
    return err.code === 'EPERM'
  }
}

function killProcess(pid: number): void {
  if (!pid || pid <= 0) return
  if (process.platform === 'win32') {
    try {
      execSync(`taskkill /pid ${pid} /f /t`, { stdio: 'ignore' })
    } catch {
      try {
        process.kill(pid, 'SIGKILL')
      } catch {}
    }
  } else {
    try {
      process.kill(pid, 'SIGTERM')
    } catch {}
  }
}

function cleanupChildProcess(): void {
  if (childProcess && !childProcess.killed) {
    const pid = childProcess.pid
    if (pid) {
      appendLauncherLog(`退出时清理子进程 PID: ${pid}`)
      killProcess(pid)
    }
    childProcess = null
  }
  coreState.alive = false
}

// 7. 启动并监督 ada-gateway 网关服务
async function startGatewaySupervisor(): Promise<void> {
  const cli = parseCliArgs()
  const cfg = loadDesktopConfig()

  const externalUrl = cli.connect || cfg.gateway_url
  const token = cli.token || cfg.token || ''

  // 场景 A: 外部指定网关服务地址
  if (externalUrl) {
    const port = extractPortFromUrl(externalUrl) || 4000
    appendLauncherLog(`连接至外部网关: ${externalUrl}`)
    coreState.alive = true
    coreState.port = port
    coreState.token = token
    coreState.url = externalUrl
    coreState.childPid = null
    coreState.error = null
    resolveReadySignal()
    return
  }

  // 场景 B: 探测并拉起独立 ada-gateway 二进制
  const customBin = cli.gatewayBin || cfg.gateway_bin_path
  const gatewayBin = findBinary('ada-gateway', customBin) || findBinary('a-da-gateway', customBin)

  if (!gatewayBin) {
    const msg = '未找到 ada-gateway 二进制文件，请确认已编译或在设置中指定网关地址'
    appendLauncherLog(`[错误] ${msg}`)
    dialog.showErrorBox('a_da 启动错误', msg)
    coreState.error = msg
    resolveReadySignal()
    return
  }

  appendLauncherLog(`正在拉起网关进程: ${gatewayBin}`)

  // 🔴 **固定端口**：网页访问要"打开就能用"，端口就不能是随机的（以前是 `--port 0`，
  // 浏览器根本猜不到桌面端把网关起在哪个端口）。被占用时网关自己会顺延，实际端口
  // 从 READY 行里拿，界面上的"浏览器打开"按钮据此拼地址。
  const args = ['--port', '52353', '--allow-origin', '*']
  const effectiveWorkspace = cli.workspace.trim()

  // 前端产物目录：网关是独立的 Rust 进程，**读不到 asar 里的文件**。
  // 打包时它被复制到 `resources/bin/ui`（electron-builder.json 的 extraResources），
  // 开发时指向仓库里的 desktop-ui/dist。
  const uiDir = findUiDir()
  if (uiDir) {
    args.push('--ui-dir', uiDir)
  } else {
    appendLauncherLog('[警告] 未找到前端产物（desktop-ui/dist），网页访问只会看到一页提示')
  }

  if (effectiveWorkspace) {
    args.push('--workspace', effectiveWorkspace)
  }
  if (token) {
    args.push('--token', token)
  }

  const cwd = effectiveWorkspace && fs.existsSync(effectiveWorkspace) ? effectiveWorkspace : getAppHome()

  try {
    childProcess = spawn(gatewayBin, args, {
      cwd,
      windowsHide: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
  } catch (err: any) {
    const msg = `拉起 ada-gateway 进程失败: ${err?.message || String(err)}`
    appendLauncherLog(`[错误] ${msg}`)
    coreState.error = msg
    resolveReadySignal()
    return
  }

  const pid = childProcess.pid || 0
  coreState.childPid = pid
  appendLauncherLog(`ada-gateway 进程已拉起, PID: ${pid}`)

  let isReady = false

  // 监听标准输出并提取就绪行
  if (childProcess.stdout) {
    const rl = readline.createInterface({ input: childProcess.stdout })
    rl.on('line', (line) => {
      appendLauncherLog(`[ada-gateway stdout] ${line}`)
      if (!isReady && line.startsWith('A_DA_GATEWAY_READY ')) {
        const jsonStr = line.slice('A_DA_GATEWAY_READY '.length).trim()
        try {
          const payload = JSON.parse(jsonStr)
          const port = Number(payload.port)
          if (port > 0) {
            isReady = true
            coreState.alive = true
            coreState.port = port
            coreState.token = token
            coreState.url = token
              ? `ws://127.0.0.1:${port}/rpc?token=${encodeURIComponent(token)}`
              : `ws://127.0.0.1:${port}/rpc`
            coreState.error = null
            appendLauncherLog(`ada-gateway 网关就绪: ${coreState.url}`)
            appendLauncherLog(
              `网页访问: http://127.0.0.1:${port}/（页面与 /rpc 同端口，可直接在浏览器打开）`
            )
            resolveReadySignal()
          }
        } catch (e) {
          appendLauncherLog(`解析就绪行 JSON 异常: ${e}`)
        }
      }
    })
  }

  // 监听标准错误
  if (childProcess.stderr) {
    const rlErr = readline.createInterface({ input: childProcess.stderr })
    rlErr.on('line', (line) => {
      appendLauncherLog(`[ada-gateway stderr] ${line}`)
    })
  }

  // 子进程异常退出回调
  childProcess.on('exit', (code, signal) => {
    appendLauncherLog(`ada-gateway 退出, code: ${code}, signal: ${signal}`)
    coreState.alive = false
    if (!isReady) {
      coreState.error = `ada-gateway 启动失败并在就绪前退出 (code: ${code})`
      resolveReadySignal()
    }
  })

  // 启动超时看门狗 (15 秒)
  setTimeout(() => {
    if (!isReady) {
      coreState.error = '启动 ada-gateway 超时（15 秒内未收到就绪信号）'
      appendLauncherLog('[超时] 未在 15 秒内收到 ada-gateway 就绪信号')
      resolveReadySignal()
    }
  }, 15000)
}

/** 汇总当前托管的进程（桌面端自己 spawn 的网关；外部网关单列一条） */
function collectProcesses(): ManagedProcessInfo[] {
  const processes: ManagedProcessInfo[] = []
  if (coreState.childPid) {
    const alive = isPidAlive(coreState.childPid)
    processes.push({
      name: 'ada-gateway',
      role: '统一网关与路由平台',
      pid: coreState.childPid,
      port: coreState.port,
      alive,
      status: alive ? 'running' : 'stopped',
      url: coreState.url || null,
    })
  } else if (coreState.alive) {
    processes.push({
      name: 'ada-gateway (外部)',
      role: '外部接入网关平台',
      pid: null,
      port: coreState.port,
      alive: true,
      status: 'running',
      url: coreState.url || null,
    })
  }
  return processes
}

/**
 * **重新唤醒网关**：杀掉当前托管的网关子进程，按同一套参数重新拉起。
 *
 * 网关进程意外死掉、或改了网关配置之后，
 * 界面上"进程管理"可以只重启网关，而不必重启整个桌面端（会话与界面状态都还在）。
 */
async function restartGatewaySupervisor(): Promise<ProcessReport> {
  appendLauncherLog('收到「重新唤醒网关」请求，正在重启网关...')
  cleanupChildProcess()
  coreState.alive = false
  coreState.error = null
  coreState.port = 0
  coreState.url = ''
  coreState.childPid = null
  resetReadySignal()

  await startGatewaySupervisor()
  // 等就绪行（或超时/失败）再回话，避免前端拿到一份"中间态"报告
  await Promise.race([readySignalPromise, new Promise((resolve) => setTimeout(resolve, 16000))])

  appendLauncherLog(
    `重新唤醒网关结束: alive=${coreState.alive} port=${coreState.port} error=${coreState.error ?? 'none'}`
  )
  return {
    mode: 'gateway',
    core_alive: coreState.alive,
    core_url: coreState.url,
    core_port: coreState.port,
    error: coreState.error,
    processes: collectProcesses(),
  }
}

// 8. 窗口管理
function getAppIcon(): string | undefined {
  const root = app.getAppPath()
  const ico = path.resolve(root, 'assets/logo.ico')
  if (fs.existsSync(ico)) return ico
  const png = path.resolve(root, 'assets/logo.png')
  if (fs.existsSync(png)) return png
  return undefined
}

// 检测前端本地 Vite 开发服务器是否处于活动状态 (1420 端口)
function checkDevServer(url: string, timeoutMs = 800): Promise<boolean> {
  return new Promise((resolve) => {
    const req = http.get(url, (res) => {
      resolve(Boolean(res.statusCode && res.statusCode < 400))
    })
    req.on('error', () => resolve(false))
    req.setTimeout(timeoutMs, () => {
      req.destroy()
      resolve(false)
    })
  })
}

/**
 * 窗口尺寸的**下限**（与桌面端 `utils/layout.ts` 的 `WINDOW_MIN_WIDTH` 同值）。
 *
 * 这三列布局在 860 宽时已经是最紧的合法状态：侧边栏 392 + 主区 420 + 右栏图标栏 48。
 * 再窄就只能靠右栏变抽屉 + 侧栏折叠来撑，不值得。
 */
const WINDOW_MIN_WIDTH = 860
const WINDOW_MIN_HEIGHT = 620

/**
 * 初始窗口大小：按主屏**工作区**推算，而不是写死一组像素。
 *
 * 为什么改：原来固定 `1120x780`。这个值在 1080p 上凑合，但在 2K/4K 屏上开局就占不到
 * 半屏（右侧 Agent 面板一展开，主区只剩 586px）；在 1366x768 的小屏上又贴着边。
 *
 * 规则（三步，都可预测）：
 * 1. 目标 = 工作区宽的 72% / 高的 78%（"留出桌面感"，不铺满）；
 * 2. 夹在 `1120..1360` × `720..900` 之间（小于 1120 时三列已经很挤，大于 1360 时
 *    对话行宽会超过舒适阅读区）；
 * 3. 最后再夹一次"不超出工作区"（小屏兜底，留 40px 边距）。
 *
 * 注意：这里只决定**首次**大小。用户拖过窗口之后不记忆（Electron 不会自动保存），
 * 想要"记得上次大小"需要额外持久化，属另一件事。
 */
function resolveInitialWindowSize(): { width: number; height: number } {
  const clamp = (value: number, lo: number, hi: number) => Math.min(Math.max(value, lo), hi)
  const { width: areaWidth, height: areaHeight } = screen.getPrimaryDisplay().workAreaSize

  const preferredWidth = Math.round(clamp(areaWidth * 0.72, 1120, 1360))
  const preferredHeight = Math.round(clamp(areaHeight * 0.78, 720, 900))

  return {
    width: Math.min(preferredWidth, Math.max(WINDOW_MIN_WIDTH, areaWidth - 40)),
    height: Math.min(preferredHeight, Math.max(WINDOW_MIN_HEIGHT, areaHeight - 40)),
  }
}

async function createMainWindow(): Promise<BrowserWindow> {
  // preload 是**宿主能力的唯一入口**（网关发现 / 窗口控制 / 配置 / 进程管理全靠它注入的
  // `window.electronAPI`）。路径解析失败 = 整个壳子哑掉，必须当场喊出来，不能静默降级。
  const preloadPath = resolveAppPath('dist-electron', 'preload.cjs')
  if (!preloadPath) {
    const msg =
      '未找到 dist-electron/preload.cjs —— 宿主 IPC 桥无法注入，界面将拿不到网关端口、窗口按钮也会失效。\n' +
      '请重新构建：bun run electron:build:main'
    appendLauncherLog(`[错误] ${msg}`)
    dialog.showErrorBox('a_da 启动错误', msg)
  } else {
    appendLauncherLog(`preload 路径: ${preloadPath}`)
  }

  const initial = resolveInitialWindowSize()
  appendLauncherLog(
    `初始窗口: ${initial.width}x${initial.height}（工作区 ${screen.getPrimaryDisplay().workAreaSize.width}x${screen.getPrimaryDisplay().workAreaSize.height}）`
  )

  mainWindow = new BrowserWindow({
    title: 'a_da',
    width: initial.width,
    height: initial.height,
    minWidth: WINDOW_MIN_WIDTH,
    minHeight: WINDOW_MIN_HEIGHT,
    center: true,
    frame: false,
    backgroundColor: '#18181b',
    icon: getAppIcon(),
    webPreferences: {
      preload: preloadPath ?? undefined,
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
      webSecurity: false,
      allowRunningInsecureContent: true,
    },
  })

  // 转发渲染进程控制台与错误日志到本地 launcher.log。
  //
  // 🔴 Electron ≥32 把这个事件的签名改成了 `(event, details)`（旧的是
  // `(event, level, message, line, sourceId)`）——按旧签名写会拿到一堆 undefined，
  // 界面里的错就再也进不了日志（"打包后的 exe 收不到自动化通道"，只能靠应用日志当证据）。
  mainWindow.webContents.on('console-message', (_event: any, ...args: any[]) => {
    const first = args[0]
    if (args.length === 1 && first && typeof first === 'object') {
      appendLauncherLog(
        `[Renderer ${first.level ?? 'log'}] ${first.message ?? ''} (${first.sourceId ?? '?'}:${first.lineNumber ?? '?'})`
      )
      return
    }
    const [level, message, line, sourceId] = args
    appendLauncherLog(`[Renderer console ${level}] ${message} (${sourceId}:${line})`)
  })

  // preload 自己加载失败时（语法错、路径被删）这是唯一能看见的线索
  mainWindow.webContents.on('preload-error', (_event, preloadPath2, error) => {
    appendLauncherLog(`[preload-error] ${preloadPath2}: ${error?.message ?? String(error)}`)
  })

  mainWindow.webContents.on('render-process-gone', (_event, details) => {
    appendLauncherLog(`[render-process-gone] reason=${details?.reason} exitCode=${details?.exitCode}`)
  })

  mainWindow.webContents.on('did-fail-load', (_event, errorCode, errorDescription, validatedURL) => {
    appendLauncherLog(`[did-fail-load] ${errorCode}: ${errorDescription} on ${validatedURL}`)
  })

  // 窗口最大化/还原状态广播给前端
  mainWindow.on('maximize', () => {
    mainWindow?.webContents.send('window:maximize-change', true)
  })
  mainWindow.on('unmaximize', () => {
    mainWindow?.webContents.send('window:maximize-change', false)
  })

  // 快捷键支持：按 F12 或 Ctrl+Shift+I 唤出开发者工具
  mainWindow.webContents.on('before-input-event', (event, input) => {
    if ((input.control && input.shift && input.key.toLowerCase() === 'i') || input.key === 'F12') {
      mainWindow?.webContents.toggleDevTools()
      event.preventDefault()
    }
  })

  // 优先尝试开发服务器热重载 (http://127.0.0.1:1420)
  const devUrl = 'http://127.0.0.1:1420'
  const isDevRunning = !app.isPackaged && (await checkDevServer(devUrl))

  if (isDevRunning) {
    appendLauncherLog(`加载开发服务器地址: ${devUrl}`)
    await mainWindow.loadURL(devUrl)
  } else {
    // 生产模式加载静态 dist 文件。
    // 🔴 `app.getAppPath()` 优先：dev 时它是仓库根、打包后是 `resources/app.asar`，两边都指向
    // **本该使用**的那份前端产物；`__dirname` 在 bun 的产物里被硬编码成构建机源码目录，
    // 放在前面会让"打包后运行"悄悄去读构建机上的源码树（换台机器就是另一份界面）。
    const candidatePaths = [
      resolveAppPath('desktop-ui', 'dist', 'index.html'),
      path.join(app.getAppPath(), 'desktop-ui/dist/index.html'),
      path.resolve(app.getAppPath(), '../desktop-ui/dist/index.html'),
      path.resolve(__dirname, '../desktop-ui/dist/index.html'),
    ].filter(Boolean) as string[]
    let loaded = false
    for (const p of candidatePaths) {
      if (fs.existsSync(p)) {
        appendLauncherLog(`加载本地静态打包文件: ${p}`)
        await mainWindow.loadFile(p)
        loaded = true
        break
      }
    }
    if (!loaded) {
      appendLauncherLog(`[警告] 未探测到静态文件，尝试默认路径: ${candidatePaths[0]}`)
      await mainWindow.loadFile(candidatePaths[0])
    }
  }

  mainWindow.on('closed', () => {
    mainWindow = null
  })

  return mainWindow
}

// 9. 注册 IPC 接口通信处理器
function registerIpcHandlers(): void {
  ipcMain.handle('desktop:get-core-info', async (): Promise<CoreInfo> => {
    await readySignalPromise
    return {
      alive: coreState.alive,
      mode: 'gateway',
      port: coreState.port,
      token: coreState.token,
      url: coreState.url,
      error: coreState.error,
    }
  })

  ipcMain.handle('desktop:get-config', async (): Promise<DesktopConfig> => {
    return loadDesktopConfig()
  })

  ipcMain.handle('desktop:set-config', async (_, cfg: DesktopConfig): Promise<void> => {
    saveDesktopConfig(cfg)
  })

  ipcMain.handle('desktop:restart', async (): Promise<void> => {
    appendLauncherLog('前端请求重启桌面端...')
    cleanupChildProcess()
    app.relaunch()
    app.exit(0)
  })

  // 只重启网关
  ipcMain.handle('desktop:restart-gateway', async (): Promise<ProcessReport> => {
    return await restartGatewaySupervisor()
  })

  ipcMain.handle('desktop:get-process-report', async (): Promise<ProcessReport> => {
    return {
      mode: 'gateway',
      core_alive: coreState.alive,
      core_url: coreState.url,
      core_port: coreState.port,
      error: coreState.error,
      processes: collectProcesses(),
    }
  })

  ipcMain.handle('desktop:kill-process', async (_, pid: number): Promise<void> => {
    killProcess(pid)
  })

  ipcMain.handle('desktop:window-minimize', async (): Promise<void> => {
    mainWindow?.minimize()
  })

  ipcMain.handle('desktop:window-maximize', async (): Promise<void> => {
    if (!mainWindow) return
    if (mainWindow.isMaximized()) {
      mainWindow.unmaximize()
    } else {
      mainWindow.maximize()
    }
  })

  ipcMain.handle('desktop:window-close', async (): Promise<void> => {
    mainWindow?.close()
  })

  ipcMain.handle('desktop:window-is-maximized', async (): Promise<boolean> => {
    return mainWindow?.isMaximized() ?? false
  })
}

// 10. 生命周期挂钩
app.on('second-instance', () => {
  if (mainWindow) {
    if (mainWindow.isMinimized()) mainWindow.restore()
    mainWindow.show()
    mainWindow.focus()
  }
})

app.on('before-quit', () => {
  cleanupChildProcess()
})

app.on('window-all-closed', () => {
  cleanupChildProcess()
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.whenReady().then(async () => {
  registerIpcHandlers()
  startGatewaySupervisor()
  await createMainWindow()

  app.on('activate', async () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      await createMainWindow()
    }
  })
})
