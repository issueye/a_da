/**
 * The window through the GPU test renderer.
 *
 * These open no real window: `createTestRoot()` runs the same `GpuixView`,
 * `build_element()` and paint path offscreen, so a click and the frame it
 * produces can be asserted without a desktop.
 *
 *   bun test
 */

import { describe, expect, test } from 'bun:test'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { AgentWindow } from './AgentWindow'
import { store } from './agent/store'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

async function mount() {
  const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
  render(<AgentWindow />)
  const app = await connectTest(renderer)
  // Every painted string of the last frame, joined: the screen as the user sees it.
  const screen = () => renderer.getPaintedText().join('\n')
  return { app, screen }
}

describeNative('agent window', () => {
  test('opens on the empty state the design shows', async () => {
    const { app, screen } = await mount()
    const painted = screen()

    for (const label of [
      '工作区',
      '会话',
      '新建对话',
      '在下方输入任务目标：Agent 会工作区内执行，改动与命令需你批准',
      'Agent 只能访问当前项目内的文件',
      '描述要 Agent 完成的任务',
      '自动批准',
      '调试',
      '最高',
    ]) {
      expect(painted, `missing ${label}`).toContain(label)
    }

    await app.close()
  })

  test('only shows the queue hint while a turn is running', async () => {
    const { app, screen } = await mount()
    expect(screen()).not.toContain('继续输入以排队后续修改')
    await app.close()
  })

  test('opens and closes the event log from the composer', async () => {
    const { app, screen } = await mount()
    expect(screen()).not.toContain('事件日志')

    await app.getByTestId('debug').click()
    await app.getByTestId('debug-on').click()
    expect(screen()).toContain('事件日志')
    // 空状态只有真的没有事件时才该出现：工作区或全局装了扩展的话，日志一开始就
    // 有「已加载扩展工具」那几行（这个仓库自己就带一个 web_search 扩展）。
    if (store.log.length === 0) expect(screen()).toContain('还没有事件')

    await app.getByTestId('debug-close').click()
    expect(screen()).not.toContain('事件日志')

    await app.close()
  })

  test('switches the approval mode', async () => {
    const { app, screen } = await mount()
    await app.getByTestId('approval').click()
    await app.getByTestId('approval-ask').click()
    expect(screen()).toContain('每次询问')

    await app.getByTestId('approval').click()
    await app.getByTestId('approval-auto').click()
    expect(screen()).toContain('自动批准')

    await app.close()
  })

  test('collapses the sidebar', async () => {
    const { app, screen } = await mount()
    // 「新建对话」只出现在侧边栏：欢迎卡片里也有「工作区」的字样，不能拿它当标记。
    expect(screen()).toContain('新建对话')

    await app.getByTestId('toggle-sidebar').click()
    expect(screen()).not.toContain('新建对话')

    await app.getByTestId('toggle-sidebar').click()
    expect(screen()).toContain('新建对话')

    await app.close()
  })

  test('opens a thread and filters the list', async () => {
    const { app, screen } = await mount()
    await app.getByTestId('sidebar-new-chat').click()
    await app.getByTestId('search').click()

    // The search input replaces the second thread's row, and a query that
    // matches nothing leaves the list empty while the sidebar header stays.
    expect(screen()).toContain('会话\n2')
    await app.getByTestId('thread-search').fill('没有这个会话')
    expect(screen()).toContain('会话\n0')
    expect(screen()).toContain('新建对话')

    await app.close()
  })

  test('shows scroll-to-bottom button when scrolled away and scrolls to bottom on click', async () => {
    const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
    render(<AgentWindow />)
    const app = await connectTest(renderer)

    // 初始状态没有置底按钮
    expect(renderer.getPaintedText().join('\n')).not.toContain('回到底部')

    // 添加会话记录以产生虚拟长列表
    for (let i = 0; i < 20; i++) {
      store.active.items.push({
        kind: 'user',
        id: `test_item_${i}`,
        at: Date.now(),
        text: `会话消息 ${i}`,
      })
    }
    // 触发更新
    (store as any).notify()
    for (let attempt = 0; attempt < 30; attempt++) {
      renderer.flush()
      if (renderer.findByType('virtual-list').length) break
      await new Promise((resolve) => setTimeout(resolve, 30))
    }

    const list = renderer.findByType('virtual-list')[0]!
    expect(list).toBeDefined()

    // 模拟向上滚动后触发的 visibleRange 事件（未到末尾）
    const { handleGpuixEvent } = await import('@gpuix/react')
    for (let attempt = 0; attempt < 30; attempt++) {
      const currentList = renderer.findByType('virtual-list')[0]
      if (currentList) {
        handleGpuixEvent(
          {
            elementId: currentList.id,
            eventType: 'visibleRange',
            startIndex: 0,
            endIndex: 5,
          },
          renderer,
        )
      }
      renderer.flush()
      if (renderer.getPaintedText().join('\n').includes('回到底部')) break
      await new Promise((resolve) => setTimeout(resolve, 40))
    }

    // 此时应当浮现“回到底部”置底按钮
    expect(renderer.getPaintedText().join('\n')).toContain('回到底部')

    // 点击“回到底部”按钮
    await app.getByTestId('scroll-to-bottom').click()
    for (let attempt = 0; attempt < 20; attempt++) {
      renderer.flush()
      if (!renderer.getPaintedText().join('\n').includes('回到底部')) break
      await new Promise((resolve) => setTimeout(resolve, 30))
    }

    // 点击后置底按钮消失（恢复底部状态）
    expect(renderer.getPaintedText().join('\n')).not.toContain('回到底部')

    await app.close()
  })
})
