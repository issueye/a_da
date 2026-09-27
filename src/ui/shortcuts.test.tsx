/**
 * 全局快捷键分发的纯逻辑测试 + 命令面板的渲染交互测试。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import React from 'react'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { connectTest } from '@gpuix/react/automation'
import { useEffect, useState } from 'react'
import { store } from '../agent/store'
import { GLOBAL_SHORTCUTS, handleGlobalShortcut, type ShortcutEvent } from './shortcuts'
import { CommandPalette } from './CommandPalette'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const key = (overrides: Partial<ShortcutEvent>): ShortcutEvent => ({
  key: 'k',
  modifiers: { ctrl: true, shift: false, alt: false, cmd: false },
  ...overrides,
})

describe('handleGlobalShortcut 分发', () => {
  test('Ctrl+K 开关命令面板', () => {
    store.setPaletteOpen(false)
    handleGlobalShortcut(key({ key: 'k' }), store)
    expect(store.paletteOpen).toBe(true)
    handleGlobalShortcut(key({ key: 'K' }), store)
    expect(store.paletteOpen).toBe(false)
  })

  test('Ctrl+B 切换侧边栏，Ctrl+D 切换调试', () => {
    const sidebarBefore = store.sidebarOpen
    handleGlobalShortcut(key({ key: 'b' }), store)
    expect(store.sidebarOpen).toBe(!sidebarBefore)
    handleGlobalShortcut(key({ key: 'b' }), store)
    expect(store.sidebarOpen).toBe(sidebarBefore)

    const debugBefore = store.debugOpen
    handleGlobalShortcut(key({ key: 'd' }), store)
    expect(store.debugOpen).toBe(!debugBefore)
    store.toggleDebug()
  })

  test('面板打开时其它快捷键被屏蔽，Esc 只关浮层', () => {
    store.setPaletteOpen(true)
    // Ctrl+T 在面板打开时不该新建会话
    const threadsBefore = store.threads.length
    handleGlobalShortcut(key({ key: 't' }), store)
    expect(store.threads.length).toBe(threadsBefore)

    handleGlobalShortcut(key({ key: 'Escape' }), store)
    expect(store.paletteOpen).toBe(false)
  })

  test('裸键与按住重复不触发', () => {
    const before = store.paletteOpen
    expect(handleGlobalShortcut(key({ key: 'k', modifiers: { ctrl: false, shift: false, alt: false, cmd: false } }), store)).toBe(false)
    expect(store.paletteOpen).toBe(before)
    // 按住不放的自动重复直接忽略（否则开关会疯狂抖动）
    expect(handleGlobalShortcut(key({ key: 'k', isHeld: true }), store)).toBe(false)
    expect(store.paletteOpen).toBe(before)
  })

  test('Ctrl+R 开关改动审阅', () => {
    store.setChangesOpen(false)
    handleGlobalShortcut(key({ key: 'r' }), store)
    expect(store.changesOpen).toBe(true)
    handleGlobalShortcut(key({ key: 'r' }), store)
    expect(store.changesOpen).toBe(false)
  })

  test('每个快捷键都有唯一的 id 与展示键位', () => {
    const ids = new Set(GLOBAL_SHORTCUTS.map((s) => s.id))
    expect(ids.size).toBe(GLOBAL_SHORTCUTS.length)
    for (const shortcut of GLOBAL_SHORTCUTS) {
      expect(shortcut.keys.length).toBeGreaterThan(0)
      expect(shortcut.label.length).toBeGreaterThan(0)
    }
  })
})

describeNative('CommandPalette 渲染与执行', () => {
  test('列出命令、点击执行并自动关闭', async () => {
    function Host(): React.ReactElement {
      const [, setTick] = useState(0)
      useEffect(() => store.subscribe(() => setTick((t) => t + 1)), [])
      // 与 AgentWindow 一致：只在 paletteOpen 时挂载，回滚状态后才会真的消失
      return store.paletteOpen ? <CommandPalette store={store} /> : <div />
    }

    store.setPaletteOpen(true)
    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(<Host />)
    const app = await connectTest(renderer)

    const screen = () => renderer.getPaintedText().join('\n')
    const painted = async (needle: string, timeoutMs = 10_000): Promise<void> => {
      const started = Date.now()
      while (Date.now() - started < timeoutMs) {
        if (screen().includes(needle)) return
        renderer.flush?.()
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
      throw new Error(`never painted ${needle}\n${screen()}`)
    }
    const gone = async (needle: string, timeoutMs = 10_000): Promise<void> => {
      const started = Date.now()
      while (Date.now() - started < timeoutMs) {
        if (!screen().includes(needle)) return
        renderer.flush?.()
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
      throw new Error(`still paints ${needle}\n${screen()}`)
    }

    await painted('命令或搜索')
    expect(screen()).toContain('新建对话')
    expect(screen()).toContain('Ctrl+T')
    expect(screen()).toContain('切换到 Plan 模式')

    // 点击执行模式切换：执行后面板自动关闭
    await app.getByTestId('palette-item-mode-plan').click()
    await gone('命令或搜索')
    expect(store.mode).toBe('plan')
    expect(store.paletteOpen).toBe(false)

    store.setMode('code')
    store.setPaletteOpen(false)
    await app.close()
  }, 30_000)
})
