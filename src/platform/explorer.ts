/**
 * 在系统文件资源管理器中打开指定目录或文件。
 *
 * Windows: explorer.exe <path>
 * macOS: open <path>
 * Linux: xdg-open <path>
 *
 * 自动化测试可通过 setExplorerOpener 拦截，或通过 A_DA_NO_DIALOG=1 避免真开窗口。
 */

export type ExplorerOpener = (targetPath: string) => boolean | Promise<boolean>

let injected: ExplorerOpener | null = null

/** 测试用：换掉真正调起系统资源管理器的动作；传 null 恢复。 */
export function setExplorerOpener(opener: ExplorerOpener | null): void {
  injected = opener
}

/** 生成拉起系统资源管理器所用的命令行参数与选项。 */
export function explorerCommand(targetPath: string): { cmd: string[]; options: Record<string, unknown> } {
  if (process.platform === 'win32') {
    const winPath = targetPath.replace(/\//g, '\\')
    return {
      cmd: ['explorer.exe', winPath],
      options: {
        stdin: 'ignore',
        stdout: 'ignore',
        stderr: 'ignore',
        windowsHide: true,
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
