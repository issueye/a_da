/**
 * Web / H5 平台适配器实现（Web Platform Adapter）。
 *
 * 针对现代浏览器、移动端 H5 及 PWA 环境。
 * 纯标准 Web API，零桌面原生依赖。
 */

import React from 'react'
import type { UIPlatformAdapter, WindowOptions, PlatformCapabilities } from './types'

export class WebPlatformAdapter implements UIPlatformAdapter {
  readonly type = 'web' as const

  readonly capabilities: PlatformCapabilities = {
    hasNativeWindow: false,
    hasTray: false,
    hasNativeFilePicker: false,
    env: typeof window !== 'undefined' && /Mobi|Android|iPhone/i.test(navigator.userAgent) ? 'mobile' : 'web',
  }

  mount(rootComponent: React.ReactNode, _options: WindowOptions = {}): void {
    if (typeof document === 'undefined') return
    const container = document.getElementById('root') ?? document.body
    try {
      // 动态导入 react-dom/client，避免桌面无 DOM 环境下引入报错
      // eslint-disable-next-line @typescript-eslint/no-require-imports
      const { createRoot } = require('react-dom/client')
      const root = createRoot(container)
      root.render(rootComponent)
    } catch (err) {
      console.error('[WebPlatformAdapter] React DOM 挂载失败:', err)
    }
  }

  async clipboardCopy(text: string): Promise<boolean> {
    if (typeof navigator !== 'undefined' && navigator.clipboard && navigator.clipboard.writeText) {
      try {
        await navigator.clipboard.writeText(text)
        return true
      } catch {
        // 权限或上下文未授权，降级到 textarea
      }
    }

    if (typeof document !== 'undefined') {
      try {
        const textarea = document.createElement('textarea')
        textarea.value = text
        textarea.style.position = 'fixed'
        textarea.style.opacity = '0'
        document.body.appendChild(textarea)
        textarea.select()
        const ok = document.execCommand('copy')
        document.body.removeChild(textarea)
        return ok
      } catch {
        return false
      }
    }
    return false
  }

  notify(title: string, body: string, _options?: { level?: 'info' | 'warn' | 'error' }): void {
    if (typeof window === 'undefined' || !('Notification' in window)) return

    if (Notification.permission === 'granted') {
      new Notification(title, { body })
    } else if (Notification.permission !== 'denied') {
      Notification.requestPermission().then((permission) => {
        if (permission === 'granted') {
          new Notification(title, { body })
        }
      })
    }
  }

  windowControl(action: 'minimize' | 'maximize' | 'close' | 'activate'): void {
    if (action === 'close' && typeof window !== 'undefined') {
      window.close()
    }
    // 浏览器标签页无法直接操作最小化/置顶，静默忽略
  }

  async pickFile(options: { directory?: boolean; multiple?: boolean } = {}): Promise<string[] | null> {
    if (typeof document === 'undefined') return null

    return new Promise((resolve) => {
      const input = document.createElement('input')
      input.type = 'file'
      if (options.multiple) input.multiple = true
      if (options.directory) {
        input.setAttribute('webkitdirectory', '')
        input.setAttribute('directory', '')
      }

      input.onchange = () => {
        if (!input.files || input.files.length === 0) {
          resolve(null)
          return
        }
        const fileNames = Array.from(input.files).map((f) => f.name)
        resolve(fileNames)
      }

      input.oncancel = () => resolve(null)
      input.click()
    })
  }
}
