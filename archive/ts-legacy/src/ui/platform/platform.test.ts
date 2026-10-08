import { describe, expect, test } from 'bun:test'
import { platform, DesktopPlatformAdapter, WebPlatformAdapter } from './index'

describe('UI 跨端平台适配层 (Platform Adapter)', () => {
  test('当前测试环境默认识别为 Desktop 平台适配器', () => {
    expect(platform.type).toBe('desktop')
    expect(platform.capabilities.hasNativeWindow).toBe(true)
    expect(platform.capabilities.env).toBe('desktop')
  })

  test('DesktopPlatformAdapter 剪贴板与原生操作安全调用', async () => {
    const desktop = new DesktopPlatformAdapter()
    const result = await desktop.clipboardCopy('test copy')
    expect(typeof result).toBe('boolean')
  })

  test('WebPlatformAdapter 纯 Web 模式下的功能与降级机制', async () => {
    const web = new WebPlatformAdapter()
    expect(web.type).toBe('web')
    expect(web.capabilities.hasNativeWindow).toBe(false)
    expect(web.capabilities.env).toBe('web')

    // 在无 DOM 测试环境中安全处理，不抛出异常
    const copyOk = await web.clipboardCopy('web copy')
    expect(typeof copyOk).toBe('boolean')

    // 窗口控制在无 window 时静默无操作
    expect(() => web.windowControl('close')).not.toThrow()
    expect(() => web.notify('title', 'body')).not.toThrow()
  })
})
