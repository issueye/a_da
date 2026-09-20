/**
 * 目录选择弹窗里那些不需要开窗口的部分：脚本长什么样、输出怎么解释。
 *
 * 真弹窗（PowerShell + FolderBrowserDialog）只能靠人眼验，所以这里把能钉住的
 * 都钉住：三态翻译、中文路径的编码约定、owner 窗口与起始目录有没有带上。
 * `A_DA_NO_DIALOG=1` 由 scripts/test-preload.ts 设好，因此这些用例不会真开窗口。
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { parsePickerOutput, pickDirectory, pickerCommand, pickerScript, setDirectoryPicker } from './dialog'

afterEach(() => {
  setDirectoryPicker(null)
})

describe('parsePickerOutput', () => {
  const b64 = (text: string): string => Buffer.from(text, 'utf8').toString('base64')

  test('a path on stdout is a pick', () => {
    expect(parsePickerOutput(0, b64('C:\\work\\demo'), '')).toEqual({
      status: 'picked',
      path: 'C:\\work\\demo',
    })
  })

  test('a non-ASCII path survives the pipe', () => {
    const chinese = 'D:\\项目\\演示'
    expect(parsePickerOutput(0, b64(chinese), '')).toEqual({ status: 'picked', path: chinese })
  })

  test('a UTF-8 BOM or stray newline does not become part of the path', () => {
    expect(parsePickerOutput(0, `\uFEFF${b64('C:\\work\\demo')}\r\n`, '')).toEqual({
      status: 'picked',
      path: 'C:\\work\\demo',
    })
  })

  test('a silent success is a cancel, not an empty path', () => {
    expect(parsePickerOutput(0, '   \n', '')).toEqual({ status: 'cancelled' })
  })

  test('anything that is not base64 is refused instead of turned into a bogus path', () => {
    const result = parsePickerOutput(0, 'Read-Host : ??', '')
    expect(result.status).toBe('unavailable')
  })

  test('a non-zero exit is unavailable, with the first stderr line as the reason', () => {
    const result = parsePickerOutput(1, '', 'Add-Type : 找不到类型\n第二行')
    expect(result.status).toBe('unavailable')
    expect(result).toHaveProperty('reason', 'Add-Type : 找不到类型')
  })
})

describe('pickerScript', () => {
  test('references the assembly the inline C# needs', () => {
    // 少了这一行，内联 C# 编译不过：Add-Type -AssemblyName 只让 PowerShell 认识
    // 这些类型，编译器看不到它们。
    expect(pickerScript(null)).toContain('-ReferencedAssemblies System.Windows.Forms')
  })

  test('hands the path back as base64 so the console codepage cannot mangle it', () => {
    const script = pickerScript(null)
    expect(script).toContain('ToBase64String')
    expect(script).toContain('[System.Text.Encoding]::UTF8.GetBytes($dialog.SelectedPath)')
  })

  test('only writes the path when the user confirmed', () => {
    const script = pickerScript(null)
    expect(script).toContain('DialogResult]::OK')
    expect(script).toContain('$dialog.ShowDialog()')
  })

  test('owns the dialog by our window when there is one', () => {
    expect(pickerScript(4242)).toContain('$dialog.ShowDialog((New-Object AdaDialogOwner(4242)))')
    // 没有窗口就别硬塞一个 owner 进去（类定义本身总在，只有这里能区分）。
    expect(pickerScript(null)).not.toContain('New-Object AdaDialogOwner')
  })

  test('starts in the hint directory and escapes a quote in it', () => {
    expect(pickerScript(null, 'C:\\work')).toContain("$dialog.SelectedPath = 'C:\\work'")
    expect(pickerScript(null, "C:\\it's here")).toContain("$dialog.SelectedPath = 'C:\\it''s here'")
  })
})

describe('pickerCommand', () => {
  test('starts powershell without a console window', () => {
    // 应用自己没有控制台，不带 windowsHide 时 Windows 会给 powershell 新分配一个，
    // 用户就看到一个黑框陪着目录选择弹窗一起出现（这是真的发生过的一次回归）。
    expect(pickerCommand('x').options.windowsHide).toBe(true)
  })

  test('runs powershell single-threaded, which WinForms dialogs require', () => {
    expect(pickerCommand('x').cmd).toEqual(['powershell.exe', '-NoProfile', '-STA', '-Command', 'x'])
  })
})

describe('pickDirectory', () => {
  test('an injected picker replaces the real dialog', async () => {
    setDirectoryPicker(async (hint) => ({ status: 'picked', path: `picked-from-${hint}` }))
    expect(await pickDirectory('C:\\hint')).toEqual({ status: 'picked', path: 'picked-from-C:\\hint' })
  })

  test('reports unavailable rather than opening a window when dialogs are off', async () => {
    const result = await pickDirectory()
    expect(result.status).toBe('unavailable')
  })
})
