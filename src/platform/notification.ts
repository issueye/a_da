/**
 * 会话完成桌面通知模块
 * 当主会话在后台运行结束时，向操作系统发送通知卡片/窗口，并在用户点击时唤醒并激活 A-DA 窗口。
 */

import { activateAndShowWindow, findAppWindow } from './win32'

export interface CompletionNotificationOptions {
  title: string
  body: string
  threadId?: string
}

export type NotificationHandler = (options: CompletionNotificationOptions) => boolean | Promise<boolean>

let injectedHandler: NotificationHandler | null = null

/** 测试用：拦截或模拟通知发送 */
export function setNotificationHandler(handler: NotificationHandler | null): void {
  injectedHandler = handler
}

/**
 * 弹出主会话完成通知窗口
 */
export async function showCompletionNotification(options: CompletionNotificationOptions): Promise<boolean> {
  if (injectedHandler) {
    try {
      const res = await injectedHandler(options)
      return typeof res === 'boolean' ? res : true
    } catch {
      return false
    }
  }

  // 自动化测试环境下不弹出真实系统弹窗
  if (process.env.A_DA_NO_DIALOG === '1') {
    return true
  }

  // Windows 平台：使用轻量系统通知卡片弹出完成通知窗口
  if (process.platform === 'win32') {
    try {
      const hwnd = findAppWindow()
      const titleEscaped = options.title.replace(/'/g, "''")
      const bodyEscaped = options.body.replace(/'/g, "''")

      const script = [
        '[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null',
        `$template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)`,
        `$textNodes = $template.GetElementsByTagName("text")`,
        `$textNodes.Item(0).AppendChild($template.CreateTextNode('${titleEscaped}')) > $null`,
        `$textNodes.Item(1).AppendChild($template.CreateTextNode('${bodyEscaped}')) > $null`,
        `$toast = [Windows.UI.Notifications.ToastNotification]::new($template)`,
        `[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("A-DA AI").Show($toast)`,
      ].join(';')

      Bun.spawn(['powershell.exe', '-NoProfile', '-STA', '-Command', script], {
        stdin: 'ignore',
        stdout: 'ignore',
        stderr: 'ignore',
        windowsHide: true,
      })
      return true
    } catch (err) {
      console.warn('Failed to dispatch native toast notification:', err)
    }
  }

  return false
}
