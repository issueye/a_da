/**
 * 桌面原生平台适配器实现（Desktop Platform Adapter）。
 *
 * 基于 @gpuix/react 与平台底层 API（Win32 / macOS / Linux）。
 */

import React from 'react'
import type { UIPlatformAdapter, WindowOptions, PlatformCapabilities } from './types'
import { copyToClipboard, saveClipboardImageToTemp } from '../../platform/clipboard'
import { showCompletionNotification } from '../../platform/notification'
import { activateAndShowWindow } from '../../platform/win32'

export class DesktopPlatformAdapter implements UIPlatformAdapter {
  readonly type = 'desktop' as const

  readonly capabilities: PlatformCapabilities = {
    hasNativeWindow: true,
    hasTray: true,
    hasNativeFilePicker: true,
    env: 'desktop',
  }

  mount(rootComponent: React.ReactNode, options: WindowOptions = {}): void {
    // 动态引入 @gpuix/react，保证在非桌面或单测环境下不会误触发原生 addon 加载
    try {
      // eslint-disable-next-line @typescript-eslint/no-require-imports
      const { render } = require('@gpuix/react')
      render(rootComponent, {
        title: options.title ?? 'a_da',
        width: options.width ?? 1370,
        height: options.height ?? 950,
        titlebarTransparent: options.titlebarTransparent ?? true,
        windowBackground: options.windowBackground ?? 'opaque',
        trafficLightX: options.trafficLightX ?? 16,
        trafficLightY: options.trafficLightY ?? 17,
        focus: options.focus ?? true,
        onKeyDown: options.onKeyDown,
      })
    } catch (err) {
      console.error('[DesktopPlatformAdapter] GPUIX 渲染器挂载失败:', err)
    }
  }

  async clipboardCopy(text: string): Promise<boolean> {
    return copyToClipboard(text)
  }

  async clipboardReadImage(): Promise<{ saved: boolean; path?: string; reason?: string }> {
    return saveClipboardImageToTemp()
  }

  notify(title: string, body: string, _options?: { level?: 'info' | 'warn' | 'error' }): void {
    void showCompletionNotification({ title, body })
  }

  windowControl(action: 'minimize' | 'maximize' | 'close' | 'activate'): void {
    if (action === 'activate') {
      try {
        activateAndShowWindow()
      } catch (err) {
        console.warn('[DesktopPlatformAdapter] activate 失败:', err)
      }
    } else if (action === 'close') {
      process.exit(0)
    }
  }

  async pickFile(_options?: { directory?: boolean; multiple?: boolean }): Promise<string[] | null> {
    // 桌面端可通过文件选择器或对话框协议完成
    return null
  }
}
