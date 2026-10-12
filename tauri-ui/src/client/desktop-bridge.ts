/**
 * 跨壳桌面环境适配网桥 (Desktop Bridge)
 * 自动识别并无缝支持 Electron 宿主、Tauri v2 宿主与纯 Web 浏览器环境。
 */

export type DesktopMode = 'gateway'

export interface CoreInfo {
  alive: boolean
  mode?: DesktopMode
  port: number
  token: string
  url: string
  error?: string | null
}

export interface DesktopConfig {
  gateway_url?: string | null
  gateway_bin_path?: string | null
  token?: string | null
}

export interface ManagedProcessInfo {
  name: string
  role: string
  pid?: number | null
  port?: number | null
  alive: boolean
  status: string
  url?: string | null
  products?: string[]
}

export interface ProcessReport {
  mode: DesktopMode
  core_alive: boolean
  core_url: string
  core_port: number
  error?: string | null
  processes: ManagedProcessInfo[]
}

/** 是否处于 Electron 宿主容器**且 preload 已注入** IPC 桥 */
export function isElectron(): boolean {
  return typeof window !== 'undefined' && Boolean((window as any).electronAPI)
}

/**
 * 是否运行在 Electron 容器里（**即使 preload 挂了也能认出来**：看 UA）。
 *
 * 🔴 为什么要和 `isElectron()` 分开：preload 一旦没被加载，`window.electronAPI` 就是
 * undefined，只按它判会把桌面端**误当成"浏览器预览"**——网关发现悄悄回落到默认端口、
 * 窗口最小化/最大化/关闭变成空操作，界面上却一个提示都没有。
 * 这正是"Electron 版功能缺失"最隐蔽的形态，所以这里额外用 UA 认壳。
 */
export function isElectronShell(): boolean {
  if (typeof window === 'undefined') return false
  return /electron/i.test(window.navigator?.userAgent || '')
}

/** 是否处于 Tauri 宿主容器 */
export function isTauri(): boolean {
  return (
    typeof window !== 'undefined' &&
    Boolean((window as any).__TAURI_INTERNALS__ || (window as any).__TAURI__)
  )
}

/** 是否处于原生桌面应用环境 (Electron 或 Tauri) */
export function isDesktop(): boolean {
  return isElectronShell() || isTauri()
}

/** 获取底层桌面环境平台名称标识 */
export function getDesktopPlatform(): 'electron' | 'tauri' | 'web' {
  if (isElectronShell()) return 'electron'
  if (isTauri()) return 'tauri'
  return 'web'
}

let bridgeMissingReported = false

/**
 * 取 Electron 宿主 API。宿主在、但 preload 缺失时**大声报一次错**，
 * 绝不静默退化成"浏览器模式"（那样用户只会看到"未连接"和一个点不动的标题栏）。
 */
function electronApi(): any | null {
  if (typeof window === 'undefined') return null
  const api = (window as any).electronAPI
  if (api) return api
  if (isElectronShell() && !bridgeMissingReported) {
    bridgeMissingReported = true
    console.error(
      '[DesktopBridge] 检测到 Electron 宿主，但 window.electronAPI 不存在（preload 没有加载）：' +
        '网关发现与窗口控制都会失效。请检查 dist-electron/preload.cjs 是否随应用一起加载。'
    )
  }
  return null
}

/**
 * 调 Tauri 宿主自定义命令。
 *
 * 🔴 窗口控制一律优先走**宿主自定义命令**（`window_*`）：应用命令不经过 webview 的
 * capability/ACL 授权面，不会因为漏配一条 `core:window:allow-*` 就整体失效
 * （JS 侧的 `@tauri-apps/api/window` 只作为回退）。
 */
async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core')
  return await invoke<T>(cmd, args)
}

/** Tauri 窗口控制回退路径（宿主命令不可用时才用） */
async function tauriWindowApi(): Promise<typeof import('@tauri-apps/api/window')> {
  return await import('@tauri-apps/api/window')
}

/** 获取核心网关连接信息 */
export async function getCoreInfo(): Promise<CoreInfo | null> {
  const api = electronApi()
  if (api) {
    try {
      return await api.getCoreInfo()
    } catch (err) {
      console.warn('[DesktopBridge] Electron getCoreInfo 失败:', err)
      return null
    }
  }

  if (isTauri()) {
    try {
      return await tauriInvoke<CoreInfo>('get_core_info')
    } catch (err) {
      console.warn('[DesktopBridge] Tauri get_core_info 失败:', err)
      return null
    }
  }

  return null
}

/** 获取桌面端连接配置 */
export async function getDesktopConfig(): Promise<DesktopConfig> {
  const api = electronApi()
  if (api) {
    try {
      return await api.getDesktopConfig()
    } catch (err) {
      console.warn('[DesktopBridge] Electron getDesktopConfig 失败:', err)
    }
  } else if (isTauri()) {
    try {
      return await tauriInvoke<DesktopConfig>('get_desktop_config')
    } catch (err) {
      console.warn('[DesktopBridge] Tauri get_desktop_config 失败:', err)
    }
  }
  return {}
}

/** 保存桌面端连接配置 */
export async function setDesktopConfig(config: DesktopConfig): Promise<void> {
  const api = electronApi()
  if (api) {
    try {
      await api.setDesktopConfig(config)
      return
    } catch (err) {
      console.error('[DesktopBridge] Electron setDesktopConfig 失败:', err)
      throw err
    }
  } else if (isTauri()) {
    try {
      await tauriInvoke('set_desktop_config', { config })
      return
    } catch (err) {
      console.error('[DesktopBridge] Tauri set_desktop_config 失败:', err)
      throw err
    }
  }
}

/** 重启桌面端程序 */
export async function restartDesktopApp(): Promise<void> {
  const api = electronApi()
  if (api) {
    try {
      await api.restartDesktopApp()
      return
    } catch (err) {
      console.error('[DesktopBridge] Electron restartDesktopApp 失败:', err)
    }
  } else if (isTauri()) {
    try {
      await tauriInvoke('restart_desktop_app')
      return
    } catch (err) {
      console.error('[DesktopBridge] Tauri restart_desktop_app 失败:', err)
    }
  }
  console.log('[DesktopBridge] 非桌面环境，忽略重启请求')
}

/**
 * 壳子：重新唤醒网关（网关进程意外退出 / 改了网关配置后不用重启整个桌面端）。
 *
 * 两个宿主都实现了：Electron 走 `desktop:restart-gateway`，Tauri 走 `restart_gateway` 命令。
 */
export async function restartGateway(): Promise<void> {
  const api = electronApi()
  if (api) {
    if (typeof api.restartGateway !== 'function') {
      throw new Error('当前 Electron 宿主的 preload 未暴露 restartGateway（请重新构建 dist-electron）')
    }
    await api.restartGateway()
    return
  }
  if (isTauri()) {
    await tauriInvoke('restart_gateway')
    return
  }
  throw new Error('当前宿主不支持重启网关')
}

/** 获取托管子进程报告 */
export async function getProcessReport(): Promise<ProcessReport | null> {
  const api = electronApi()
  if (api) {
    try {
      return await api.getProcessReport()
    } catch (err) {
      console.warn('[DesktopBridge] Electron getProcessReport 失败:', err)
      return null
    }
  }

  if (isTauri()) {
    try {
      return await tauriInvoke<ProcessReport>('get_process_report')
    } catch (err) {
      console.warn('[DesktopBridge] Tauri get_process_report 失败:', err)
      return null
    }
  }

  return null
}

/** 终止指定的托管子进程 */
export async function killProcess(pid: number): Promise<void> {
  const api = electronApi()
  if (api) {
    try {
      await api.killProcess(pid)
      return
    } catch (err) {
      console.error('[DesktopBridge] Electron killProcess 失败:', err)
      throw err
    }
  } else if (isTauri()) {
    try {
      await tauriInvoke('kill_process', { pid })
      return
    } catch (err) {
      console.error('[DesktopBridge] Tauri kill_process 失败:', err)
      throw err
    }
  }
}

/** 窗口控制：最小化 */
export async function windowMinimize(): Promise<void> {
  const api = electronApi()
  if (api) {
    try {
      await api.windowMinimize()
      return
    } catch (err) {
      console.warn('[DesktopBridge] Electron windowMinimize 失败:', err)
    }
  } else if (isTauri()) {
    try {
      await tauriInvoke('window_minimize')
      return
    } catch (err) {
      console.warn('[DesktopBridge] Tauri window_minimize 命令失败，回退 JS API:', err)
      try {
        const { getCurrentWindow } = await tauriWindowApi()
        await getCurrentWindow().minimize()
        return
      } catch (apiErr) {
        console.error('[DesktopBridge] Tauri 最小化失败:', apiErr)
      }
    }
  }
  console.log('[DesktopBridge] 最小化窗口 (浏览器预览模式)')
}

/** 窗口控制：最大化 / 还原切换 */
export async function windowMaximize(): Promise<void> {
  const api = electronApi()
  if (api) {
    try {
      await api.windowMaximize()
      return
    } catch (err) {
      console.warn('[DesktopBridge] Electron windowMaximize 失败:', err)
    }
  } else if (isTauri()) {
    try {
      await tauriInvoke('window_maximize')
      return
    } catch (err) {
      console.warn('[DesktopBridge] Tauri window_maximize 命令失败，回退 JS API:', err)
      try {
        const { getCurrentWindow } = await tauriWindowApi()
        await getCurrentWindow().toggleMaximize()
        return
      } catch (apiErr) {
        console.error('[DesktopBridge] Tauri 最大化失败:', apiErr)
      }
    }
  }
  console.log('[DesktopBridge] 最大化窗口 (浏览器预览模式)')
}

/** 窗口控制：关闭窗口 */
export async function windowClose(): Promise<void> {
  const api = electronApi()
  if (api) {
    try {
      await api.windowClose()
      return
    } catch (err) {
      console.warn('[DesktopBridge] Electron windowClose 失败:', err)
    }
  } else if (isTauri()) {
    try {
      await tauriInvoke('window_close')
      return
    } catch (err) {
      console.warn('[DesktopBridge] Tauri window_close 命令失败，回退 JS API:', err)
      try {
        const { getCurrentWindow } = await tauriWindowApi()
        await getCurrentWindow().close()
        return
      } catch (apiErr) {
        console.error('[DesktopBridge] Tauri 关闭窗口失败:', apiErr)
      }
    }
  }
  console.log('[DesktopBridge] 关闭窗口 (浏览器预览模式)')
}

/** 窗口控制：查询当前是否处于最大化状态 */
export async function isWindowMaximized(): Promise<boolean> {
  const api = electronApi()
  if (api) {
    try {
      return await api.isWindowMaximized()
    } catch {
      return false
    }
  } else if (isTauri()) {
    try {
      return await tauriInvoke<boolean>('window_is_maximized')
    } catch {
      try {
        const { getCurrentWindow } = await tauriWindowApi()
        return await getCurrentWindow().isMaximized()
      } catch {
        return false
      }
    }
  }
  return false
}

/**
 * 监听窗口最大化/还原状态改变。
 *
 * - Electron：主进程在 `maximize` / `unmaximize` 时推 `window:maximize-change`。
 * - Tauri：宿主在 `Resized` 与每次 `window_maximize` 之后广播同名事件，所以不需要轮询。
 */
export function onWindowMaximizeChange(callback: (isMax: boolean) => void): () => void {
  const api = electronApi()
  if (api && typeof api.onWindowMaximizeChange === 'function') {
    return api.onWindowMaximizeChange(callback)
  }
  if (isTauri()) {
    let unlistenFn: (() => void) | null = null
    let disposed = false
    import('@tauri-apps/api/event')
      .then(({ listen }) => listen<boolean>('window:maximize-change', (event) => callback(event.payload)))
      .then((unlisten: () => void) => {
        if (disposed) unlisten()
        else unlistenFn = unlisten
      })
      .catch(() => {
        /* 事件面不可用时静默降级：按钮图标退化为不跟随状态 */
      })
    return () => {
      disposed = true
      if (unlistenFn) unlistenFn()
    }
  }
  return () => {}
}

/** 壳子：切换开发者工具（等价 Electron 的 F12 / Ctrl+Shift+I） */
export async function toggleDevtools(): Promise<void> {
  if (isTauri()) {
    try {
      await tauriInvoke('toggle_devtools')
    } catch (err) {
      console.warn('[DesktopBridge] Tauri toggle_devtools 失败:', err)
    }
    return
  }
  console.log('[DesktopBridge] 开发者工具仅在桌面宿主中可用')
}

/**
 * 注册桌面壳快捷键：F12 / Ctrl+Shift+I 打开开发者工具。
 *
 * Tauri 宿主自己不认识这两个键，需要前端帮忙；Electron 宿主在主进程里
 * （`before-input-event`）已经处理，这里不再重复注册。幂等，可在应用挂载时直接调用。
 */
export function registerDesktopShortcuts(): () => void {
  if (!isTauri()) return () => {}
  const onKeyDown = (event: KeyboardEvent) => {
    const isDevtoolsKey =
      event.key === 'F12' || (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === 'i')
    if (!isDevtoolsKey) return
    event.preventDefault()
    void toggleDevtools()
  }
  window.addEventListener('keydown', onKeyDown)
  return () => window.removeEventListener('keydown', onKeyDown)
}
