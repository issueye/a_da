import { describe, expect, test } from 'bun:test'
import { clipboardImageScript, copyToClipboard, saveClipboardImageToTemp } from './clipboard'

describe('clipboard helper', () => {
  test('returns false for empty text', async () => {
    const ok = await copyToClipboard('')
    expect(ok).toBe(false)
  })

  test('copies valid text or handles environment without throwing', async () => {
    const ok = await copyToClipboard('test clipboard content')
    expect(typeof ok).toBe('boolean')
  })
})

describe('clipboardImageScript 粘贴图片脚本', () => {
  test('脚本加载 WinForms/Drawing、路径转义与空剪贴板分支', () => {
    const script = clipboardImageScript('C:\Users\tmp\a-da-paste-1.png')
    expect(script).toContain('System.Windows.Forms')
    expect(script).toContain('System.Drawing')
    expect(script).toContain('GetImage()')
    expect(script).toContain('ImageFormat]::Png')
    // 反斜杠路径在单引号 PS 字符串里按字面量保留
    expect(script).toContain("'C:\Users\tmp\a-da-paste-1.png'")
    // 剪贴板没有图片时输出 NONE，纯文本粘贴不受影响
    expect(script).toContain('NONE')
  })
})

describe('saveClipboardImageToTemp 粘贴兜底', () => {
  test('返回结果形态正确；真取到图片时落盘文件必须存在', async () => {
    if (process.platform !== 'win32') {
      const result = await saveClipboardImageToTemp()
      expect(result.saved).toBe(false)
      expect(result.reason).toBeTruthy()
      return
    }
    const result = await saveClipboardImageToTemp()
    expect(typeof result.saved).toBe('boolean')
    if (result.saved) {
      expect(result.path).toBeTruthy()
      expect(await Bun.file(result.path!).exists()).toBe(true)
    }
  }, 20000)
})
