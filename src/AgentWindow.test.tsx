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
      'Projects',
      'Threads',
      '新建会话',
      '在下方输入任务目标：Agent 会工作区内执行，改动与命令需你批准',
      'Agent 只能访问当前项目内的文件',
      '描述要 Agent 完成的任务',
      '自动批准',
      '刷新',
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
    expect(screen()).toContain('还没有事件')

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
    expect(screen()).toContain('Projects')

    await app.getByTestId('toggle-sidebar').click()
    expect(screen()).not.toContain('Projects')

    await app.getByTestId('toggle-sidebar').click()
    expect(screen()).toContain('Projects')

    await app.close()
  })

  test('opens a thread and filters the list', async () => {
    const { app, screen } = await mount()
    await app.getByTestId('new-thread').click()
    await app.getByTestId('search').click()

    // The search input replaces the second thread's row, and a query that
    // matches nothing leaves the list empty while the + action row stays.
    expect(screen()).toContain('Threads\n2')
    await app.getByTestId('thread-search').fill('没有这个会话')
    expect(screen()).toContain('Threads\n0')
    expect(screen()).toContain('新建会话')

    await app.close()
  })
})
