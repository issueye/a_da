import { describe, expect, test } from 'bun:test'
import React from 'react'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { connectTest } from '@gpuix/react/automation'
import { ConfirmDialog } from './ConfirmDialog'
import { AgentWindow } from '../AgentWindow'
import { store } from '../agent/store'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

describeNative('ConfirmDialog', () => {
  test('renders dialog content and actions', async () => {
    let confirmed = false
    let closed = false

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <ConfirmDialog
        options={{
          title: '测试标题',
          message: '这是确认消息内容',
          confirmText: '确定执行',
          cancelText: '放弃操作',
          onConfirm: () => {
            confirmed = true
          },
        }}
        onClose={() => {
          closed = true
        }}
      />
    )

    const app = await connectTest(renderer)
    expect(await app.getByTestId('confirm-dialog-title').count()).toBe(1)
    expect(await app.getByTestId('confirm-dialog-message').count()).toBe(1)

    const cancelBtn = app.getByTestId('confirm-dialog-cancel')
    expect(await cancelBtn.count()).toBe(1)
    await cancelBtn.click()
    expect(closed).toBe(true)

    const confirmBtn = app.getByTestId('confirm-dialog-confirm')
    expect(await confirmBtn.count()).toBe(1)
    await confirmBtn.click()
    expect(confirmed).toBe(true)

    await app.close()
  })

  test('integrates with AgentWindow and store modal state', async () => {
    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(<AgentWindow />)

    const app = await connectTest(renderer)
    expect(await app.getByTestId('confirm-dialog').count()).toBe(0)

    let executed = false
    store.showConfirm({
      title: '删除会话',
      message: '确定要删除会话吗？',
      onConfirm: () => {
        executed = true
      },
    })

    for (let attempt = 0; attempt < 20; attempt++) {
      renderer.flush()
      if ((await app.getByTestId('confirm-dialog').count()) > 0) break
      await new Promise((resolve) => setTimeout(resolve, 30))
    }

    expect(await app.getByTestId('confirm-dialog').count()).toBe(1)

    await app.getByTestId('confirm-dialog-confirm').click()
    expect(executed).toBe(true)

    for (let attempt = 0; attempt < 20; attempt++) {
      renderer.flush()
      if ((await app.getByTestId('confirm-dialog').count()) === 0) break
      await new Promise((resolve) => setTimeout(resolve, 30))
    }

    expect(await app.getByTestId('confirm-dialog').count()).toBe(0)
    expect(store.confirmModal).toBeNull()

    await app.close()
  })
})
