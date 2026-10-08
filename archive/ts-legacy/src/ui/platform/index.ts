/**
 * 跨端平台适配器统一出口（UI Platform Gateway）。
 *
 * 自动识别执行环境并导出当前单例，亦支持上层手动替换。
 */

import type { UIPlatformAdapter } from './types'
import { DesktopPlatformAdapter } from './desktop'
import { WebPlatformAdapter } from './web'

export * from './types'
export { DesktopPlatformAdapter } from './desktop'
export { WebPlatformAdapter } from './web'

function detectPlatform(): UIPlatformAdapter {
  // 检查是否为浏览器 DOM / H5 环境
  if (typeof window !== 'undefined' && typeof document !== 'undefined') {
    return new WebPlatformAdapter()
  }
  return new DesktopPlatformAdapter()
}

/** 当前环境激活的平台适配器单例 */
export const platform: UIPlatformAdapter = detectPlatform()
