/**
 * 在系统文件资源管理器中打开指定目录或文件。
 *
 * Windows: explorer.exe <path>
 * macOS: open <path>
 * Linux: xdg-open <path>
 *
 * 自动化测试可通过 setExplorerOpener 拦截，或通过 A_DA_NO_DIALOG=1 避免真开窗口。
 */

import { resolve } from 'node:path'
import { dlopen, FFIType } from 'bun:ffi'

export type ExplorerOpener = (targetPath: string) => boolean | Promise<boolean>

let injected: ExplorerOpener | null = null

/** 测试用：换掉真正调起系统资源管理器的动作；传 null 恢复。 */
export function setExplorerOpener(opener: ExplorerOpener | null): void {
  injected = opener
}

let shell32Lib: {
  symbols: {
    ShellExecuteW: (
      hwnd: bigint | number,
      lpOperation: any,
      lpFile: any,
      lpParameters: any,
      lpDirectory: any,
      nShowCmd: number,
    ) => bigint | number
  }
} | null = null
let shell32Loaded = false

function getShell32() {
  if (shell32Loaded) return shell32Lib
  shell32Loaded = true
  if (process.platform !== 'win32') return null
  try {
    shell32Lib = dlopen('shell32.dll', {
      ShellExecuteW: {
        args: [FFIType.i64, FFIType.ptr, FFIType.ptr, FFIType.ptr, FFIType.ptr, FFIType.i32],
        returns: FFIType.i64,
      },
    }) as any
  } catch {
    shell32Lib = null
  }
  return shell32Lib
}

/** 生成拉起系统资源管理器所用的命令行参数与选项。 */
export function explorerCommand(targetPath: string): { cmd: string[]; options: Record<string, unknown> } {
  if (process.platform === 'win32') {
    const winPath = resolve(targetPath).replace(/\//g, '\\')
    return {
      cmd: ['explorer.exe', winPath],
      options: {
        stdin: 'ignore',
        stdout: 'ignore',
        stderr: 'ignore',
      },
    }
  } else if (process.platform === 'darwin') {
    return {
      cmd: ['open', targetPath],
      options: {
        stdin: 'ignore',
        stdout: 'ignore',
        stderr: 'ignore',
      },
    }
  } else {
    return {
      cmd: ['xdg-open', targetPath],
      options: {
        stdin: 'ignore',
        stdout: 'ignore',
        stderr: 'ignore',
      },
    }
  }
}

/**
 * 打开操作系统文件资源管理器并定位到目标路径。
 * 返回布尔值指示是否成功发出打开指令。
 */
export function openInExplorer(targetPath: string): boolean {
  if (!targetPath) return false

  if (injected) {
    try {
      const res = injected(targetPath)
      return typeof res === 'boolean' ? res : true
    } catch {
      return false
    }
  }

  if (process.env.A_DA_NO_DIALOG === '1') {
    return true
  }

  // Windows 平台：优先调用 Win32 原生 ShellExecuteW，直接唤醒系统资源管理器置顶呈现，
  // 彻底杜绝 CreateProcess / Bun.spawn 伴随 windowsHide 导致资源管理器窗口被隐匿的问题
  if (process.platform === 'win32') {
    try {
      const s32 = getShell32()
      if (s32) {
        const winPath = resolve(targetPath).replace(/\//g, '\\')
        const op = Buffer.from('open\0', 'utf-16le')
        const path = Buffer.from(winPath + '\0', 'utf-16le')
        const res = s32.symbols.ShellExecuteW(0, op, path, null, null, 1 /* SW_SHOWNORMAL */)
        if (Number(res) > 32) {
          return true
        }
      }
    } catch (err) {
      console.warn('ShellExecuteW failed to open directory, falling back to spawn:', err)
    }
  }

  if (typeof Bun !== 'undefined' && typeof Bun.spawn === 'function') {
    try {
      const { cmd, options } = explorerCommand(targetPath)
      Bun.spawn(cmd, options as Parameters<typeof Bun.spawn>[1])
      return true
    } catch (err) {
      console.error('Failed to open directory in explorer:', err)
      return false
    }
  }

  return false
}
