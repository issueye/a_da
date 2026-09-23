import { afterEach, describe, expect, test } from 'bun:test'
import { explorerCommand, openInExplorer, setExplorerOpener } from './explorer'

describe('explorer helper', () => {
  afterEach(() => {
    setExplorerOpener(null)
  })

  test('returns false for empty path', () => {
    expect(openInExplorer('')).toBe(false)
  })

  test('explorerCommand returns valid command and options for path', () => {
    const target = 'C:/codes/project'
    const { cmd, options } = explorerCommand(target)
    expect(cmd.length).toBeGreaterThanOrEqual(2)
    if (process.platform === 'win32') {
      expect(cmd[0]).toBe('explorer.exe')
      expect(cmd[1]).toBe('C:\\codes\\project')
      expect(options.windowsHide).toBe(true)
    } else if (process.platform === 'darwin') {
      expect(cmd[0]).toBe('open')
      expect(cmd[1]).toBe(target)
    } else {
      expect(cmd[0]).toBe('xdg-open')
      expect(cmd[1]).toBe(target)
    }
  })

  test('calls injected opener when configured', () => {
    const recorded: string[] = []
    setExplorerOpener((p) => {
      recorded.push(p)
      return true
    })

    const success = openInExplorer('D:/my/workspace')
    expect(success).toBe(true)
    expect(recorded).toEqual(['D:/my/workspace'])
  })

  test('handles injected opener returning false or throwing', () => {
    setExplorerOpener(() => false)
    expect(openInExplorer('test')).toBe(false)

    setExplorerOpener(() => {
      throw new Error('fail')
    })
    expect(openInExplorer('test')).toBe(false)
  })

  test('returns true in test environment when A_DA_NO_DIALOG=1 and no mock is set', () => {
    expect(openInExplorer('E:/codes/rust_projects')).toBe(true)
  })
})
