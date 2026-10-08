/**
 * 跨端平台抽象接口定义（UI Platform Adapter Interface）。
 *
 * 隔离底层原生桌面（GPUIX / Win32 / Cocoa）与现代浏览器 Web / H5 之间的能力差异，
 * 实现一套 React 业务组件，多端目标运行。
 */

export interface WindowOptions {
  title?: string
  width?: number
  height?: number
  titlebarTransparent?: boolean
  windowBackground?: 'opaque' | 'blurred'
  trafficLightX?: number
  trafficLightY?: number
  focus?: boolean
  onKeyDown?: (event: unknown) => void
}

export interface PlatformCapabilities {
  /** 是否支持原生多窗口/桌面窗口拖拽 */
  hasNativeWindow: boolean
  /** 是否支持系统原生托盘 */
  hasTray: boolean
  /** 是否具备原生文件系统弹窗 */
  hasNativeFilePicker: boolean
  /** 当前运行环境类型 */
  env: 'desktop' | 'web' | 'mobile'
}

export interface UIPlatformAdapter {
  /** 平台类型标识 */
  readonly type: 'desktop' | 'web'

  /** 当前平台支持的能力特征 */
  readonly capabilities: PlatformCapabilities

  /** 挂载根应用组件 */
  mount(rootComponent: React.ReactNode, options?: WindowOptions): void

  /** 复制纯文本到剪贴板 */
  clipboardCopy(text: string): Promise<boolean>

  /** 读取系统剪贴板图片（如有） */
  clipboardReadImage?(): Promise<{ saved: boolean; path?: string; reason?: string }>

  /** 唤起系统级通知 */
  notify(title: string, body: string, options?: { level?: 'info' | 'warn' | 'error' }): void

  /** 原生窗口控制操作（最小化、最大化、置顶、关闭） */
  windowControl(action: 'minimize' | 'maximize' | 'close' | 'activate'): void

  /** 选择本地文件或目录 */
  pickFile(options?: { directory?: boolean; multiple?: boolean }): Promise<string[] | null>
}
