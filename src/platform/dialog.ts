/**
 * 选目录的原生弹窗。
 *
 * GPUIX 没有目录选择的 JS API，Windows 上也没有别的能直接调的接口，所以这一步交给
 * Windows PowerShell 的 FolderBrowserDialog——它是每台 Windows 都有、又能在一两秒内
 * 开起来的原生目录选择器。两个必须的细节：5.1 要带 `-STA` 才开得出 WinForms 对话框；
 * 输出编码要显式设成 UTF-8，否则中文目录名经管道出来会变成乱码。
 *
 * 弹窗以我们的窗口为 owner（HWND 由 win32.ts 找出来），所以它挡在应用前面而不是跑到
 * 后面去。拿不到窗口时就不带 owner，弹窗本身照常可用。
 *
 * `A_DA_NO_DIALOG=1` 直接返回 unavailable：脚本和自动化测试用它来避免真开一个窗口。
 */

import type { NativeRenderer } from '@gpuix/react'
import { findAppWindow } from './win32'

export type PickDirectoryResult =
  | { status: 'picked'; path: string }
  | { status: 'cancelled' }
  | { status: 'unavailable'; reason: string }

export type DirectoryPicker = (hint?: string, renderer?: NativeRenderer | null) => Promise<PickDirectoryResult>

let injected: DirectoryPicker | null = null

/** 测试用：换掉真弹窗；传 null 恢复成真弹窗。 */
export function setDirectoryPicker(picker: DirectoryPicker | null): void {
  injected = picker
}

/**
 * 交给 powershell.exe 的脚本，单独成函数是为了能直接断言它。
 *
 * `hint` 是弹窗打开时停在哪个目录——通常是当前工作区，省得用户自己找回去。
 */
export function pickerScript(hwnd: number | null, hint?: string): string {
  const lines = [
    '$ErrorActionPreference = "Stop"',
    // 只是让报错文字好读；设不上也不影响结果，因为路径走的是 base64。
    'try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }',
    'Add-Type -AssemblyName System.Windows.Forms',
    // 内联 C# 要自己引用程序集：`-AssemblyName` 只让 PowerShell 认识这些类型，
    // 编译器看不到它们，漏掉这一行就是「命名空间 System.Windows 中不存在 Forms」。
    'Add-Type -ReferencedAssemblies System.Windows.Forms -TypeDefinition @"',
    'using System;',
    'using System.Windows.Forms;',
    'public class AdaDialogOwner : IWin32Window {',
    '  public IntPtr Handle { get; set; }',
    '  public AdaDialogOwner(long handle) { Handle = new IntPtr(handle); }',
    '}',
    '"@',
    '$dialog = New-Object System.Windows.Forms.FolderBrowserDialog',
    "$dialog.Description = '选择要作为工作区的目录'",
    '$dialog.ShowNewFolderButton = $true',
  ]

  if (hint) {
    lines.push(`$dialog.SelectedPath = '${hint.replace(/'/g, "''")}'`)
  }

  lines.push(
    hwnd === null
      ? '$chosen = $dialog.ShowDialog()'
      : `$chosen = $dialog.ShowDialog((New-Object AdaDialogOwner(${hwnd})))`,
    'if ($chosen -eq [System.Windows.Forms.DialogResult]::OK) {',
    // base64：中文路径经过管道时不该受控制台代码页摆布。
    '  [Console]::Out.Write([Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($dialog.SelectedPath)))',
    '}'
  )

  return lines.join('\n')
}

/** 把一次 powershell 运行的结果翻译成三态。 */
export function parsePickerOutput(exitCode: number, stdout: string, stderr: string): PickDirectoryResult {
  if (exitCode !== 0) {
    const reason = stderr.trim() || stdout.trim() || `powershell 退出码 ${exitCode}`
    return { status: 'unavailable', reason: reason.split('\n')[0]! }
  }

  const encoded = stdout.replace(/^\uFEFF/, '').trim()
  if (!encoded) return { status: 'cancelled' }
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(encoded)) {
    return { status: 'unavailable', reason: `弹窗返回了无法识别的内容：${encoded.slice(0, 80)}` }
  }

  const path = Buffer.from(encoded, 'base64').toString('utf8')
  return path ? { status: 'picked', path } : { status: 'cancelled' }
}

/**
 * 拉起弹窗用的命令。
 *
 * 单独成函数只为一件事能被测试钉住：`windowsHide`。应用是 GUI 子系统、自己没有
 * 控制台，而 powershell.exe 是控制台程序——漏掉这个标志，Windows 会给它新分配
 * 一个控制台窗口，用户就看到一个黑框陪着目录选择弹窗一起冒出来。
 * `CREATE_NO_WINDOW` 只压掉控制台，不影响 WinForms 自己创建的对话框。
 */
export function pickerCommand(script: string): { cmd: string[]; options: Record<string, unknown> } {
  return {
    cmd: ['powershell.exe', '-NoProfile', '-STA', '-Command', script],
    options: {
      stdin: 'ignore',
      stdout: 'pipe',
      stderr: 'pipe',
      windowsHide: true,
    },
  }
}

export async function pickDirectory(
  hint?: string,
  renderer?: NativeRenderer | null,
): Promise<PickDirectoryResult> {
  if (injected) return injected(hint, renderer)
  if (process.env.A_DA_NO_DIALOG === '1') {
    return { status: 'unavailable', reason: 'A_DA_NO_DIALOG=1' }
  }

  // 1. 优先使用 GPUIX 原生 promptForPaths 接口
  if (renderer && typeof renderer.promptForPaths === 'function') {
    try {
      const paths = await renderer.promptForPaths({
        directories: true,
        multiple: false,
        prompt: '选择工作区目录',
      })
      if (paths && paths.length > 0 && paths[0]) {
        return { status: 'picked', path: paths[0] }
      }
      return { status: 'cancelled' }
    } catch {
      // 若原生选择器抛错或不支持，平滑回退
    }
  }

  // 2. Windows 平台回退至 PowerShell FolderBrowserDialog
  if (process.platform !== 'win32') {
    return { status: 'unavailable', reason: '目录选择弹窗目前只有 Windows 版' }
  }

  const { cmd, options } = pickerCommand(pickerScript(findAppWindow(), hint))
  try {
    const proc = Bun.spawn(cmd, options as Parameters<typeof Bun.spawn>[1])
    const [stdout, stderr] = await Promise.all([
      new Response(proc.stdout as ReadableStream).text(),
      new Response(proc.stderr as ReadableStream).text(),
    ])
    const code = await proc.exited
    return parsePickerOutput(code, stdout, stderr)
  } catch (error) {
    return { status: 'unavailable', reason: (error as Error).message }
  }
}
