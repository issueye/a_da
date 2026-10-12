/**
 * Electron 桌面端类型定义
 */

export interface CoreInfo {
  alive: boolean
  mode: 'gateway'
  port: number
  token: string
  url: string
  error: string | null
}

export interface DesktopConfig {
  gateway_url?: string | null
  gateway_bin_path?: string | null
  token?: string | null
}

export interface ManagedProcessInfo {
  name: string
  role: string
  pid: number | null
  port: number | null
  alive: boolean
  status: string
  url: string | null
  products?: string[]
}

export interface ProcessReport {
  mode: 'gateway'
  core_alive: boolean
  core_url: string
  core_port: number
  error: string | null
  processes: ManagedProcessInfo[]
}

export interface ElectronDesktopAPI {
  getCoreInfo: () => Promise<CoreInfo>
  getDesktopConfig: () => Promise<DesktopConfig>
  setDesktopConfig: (cfg: DesktopConfig) => Promise<void>
  restartDesktopApp: () => Promise<void>
  /** 只重启网关（终止当前托管的 ada-gateway 并按原参数重新拉起） */
  restartGateway: () => Promise<ProcessReport>
  getProcessReport: () => Promise<ProcessReport>
  killProcess: (pid: number) => Promise<void>
  windowMinimize: () => Promise<void>
  windowMaximize: () => Promise<void>
  windowClose: () => Promise<void>
  isWindowMaximized: () => Promise<boolean>
  onWindowMaximizeChange?: (callback: (isMax: boolean) => void) => () => void
}
