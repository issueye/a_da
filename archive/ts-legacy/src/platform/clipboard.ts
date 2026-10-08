/**
 * 跨平台剪贴板复制工具
 * 优先使用浏览器标准 navigator.clipboard，桌面环境回退至系统原生命令
 */

import { join } from 'node:path'
import { tmpdir } from 'node:os'

export async function copyToClipboard(text: string): Promise<boolean> {
  if (!text) return false

  // 1. Web / 浏览器标准 API
  if (typeof navigator !== 'undefined' && navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      // 权限受限或非安全上下文时向下回退
    }
  }

  // 2. 原生桌面回退 (Bun.spawn)
  if (typeof Bun !== 'undefined' && typeof Bun.spawn === 'function') {
    try {
      if (process.platform === 'win32') {
        const proc = Bun.spawn(['clip'], { stdin: 'pipe' })
        proc.stdin.write(text)
        proc.stdin.end()
        await proc.exited
        return true
      } else if (process.platform === 'darwin') {
        const proc = Bun.spawn(['pbcopy'], { stdin: 'pipe' })
        proc.stdin.write(text)
        proc.stdin.end()
        await proc.exited
        return true
      } else if (process.platform === 'linux') {
        const proc = Bun.spawn(['xclip', '-selection', 'clipboard'], { stdin: 'pipe' })
        proc.stdin.write(text)
        proc.stdin.end()
        await proc.exited
        return true
      }
    } catch {
      // 容错
    }
  }

  return false
}

export interface ClipboardImageResult {
  /** 是否从剪贴板取到了图片 */
  saved: boolean
  /** 落盘的 PNG 路径（saved 时有效） */
  path?: string
  reason?: string
}

/**
 * 生成「把剪贴板里的图片另存为 PNG」的 PowerShell 脚本。
 * 单独成函数与 dialog.ts 的 pickerScript 同理：能被测试直接钉住。
 * 剪贴板里是文本时 GetImage() 返回 null，脚本输出 NONE——所以 Ctrl+V 粘贴
 * 文字的场景不会误附件。
 */
export function clipboardImageScript(savePath: string): string {
  return [
    'Add-Type -AssemblyName System.Windows.Forms',
    'Add-Type -AssemblyName System.Drawing',
    '$img = [System.Windows.Forms.Clipboard]::GetImage()',
    // PS 单引号字符串里反斜杠是字面量，不需要任何转义
    `if ($img -eq $null) { Write-Output 'NONE' } else { $img.Save('${savePath}', [System.Drawing.Imaging.ImageFormat]::Png); Write-Output 'OK' }`,
  ].join('; ')
}

/**
 * 读 Windows 剪贴板；若其中是图片，落盘为临时 PNG 并返回路径。
 *
 * GPUX 没有暴露剪贴板 API（原生 textarea 只认文本粘贴），Composer 在
 * Ctrl+V 时调这里兜底：剪贴板有图就作为图片附件，纯文本粘贴不受影响。
 * 非 Windows 返回 unavailable。
 */
export async function saveClipboardImageToTemp(): Promise<ClipboardImageResult> {
  if (process.platform !== 'win32') {
    return { saved: false, reason: '剪贴板图片目前只有 Windows 版' }
  }
  if (typeof Bun === 'undefined' || typeof Bun.spawn !== 'function') {
    return { saved: false, reason: '运行时不支持进程调用' }
  }

  const savePath = join(tmpdir(), `a-da-paste-${Date.now()}.png`)
  const cmd = ['powershell.exe', '-NoProfile', '-STA', '-Command', clipboardImageScript(savePath)]
  try {
    const proc = Bun.spawn(cmd, {
      stdin: 'ignore',
      stdout: 'pipe',
      stderr: 'pipe',
      windowsHide: true,
    })
    // PS 偶发挂起（如剪贴板被其他进程占用）：超时强杀，不让粘贴把输入框拖死
    const timer = setTimeout(() => {
      try {
        proc.kill()
      } catch {}
    }, 8000)
    const stdout = await new Response(proc.stdout as ReadableStream).text()
    await proc.exited
    clearTimeout(timer)

    if (stdout.trim() === 'OK' && (await Bun.file(savePath).exists())) {
      return { saved: true, path: savePath }
    }
    if (stdout.includes('NONE')) {
      return { saved: false, reason: '剪贴板里没有图片' }
    }
    return { saved: false, reason: '读取剪贴板失败' }
  } catch (error) {
    return { saved: false, reason: (error as Error).message }
  }
}
