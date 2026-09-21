import { describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { store } from '../agent/store'
import { EmptyConversationView } from './EmptyConversationView'
import { shortPath } from '../theme'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const dirs: string[] = []

async function project(name: string): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), `a-da-empty-${name}-`))
  dirs.push(dir)
  return dir
}

describeNative('EmptyConversationView', () => {
  test('renders workspace selector and centered composer', async () => {
    const ws1 = await project('ws1')
    const ws2 = await project('ws2')
    store.newThread(ws1)
    store.newThread(ws2)

    const label1 = shortPath(ws1, 2)
    const label2 = shortPath(ws2, 2)

    const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
    render(<EmptyConversationView store={store} />)
    const app = await connectTest(renderer)

    const screen = () => renderer.getPaintedText().join('\n')

    // 应该渲染工作区选择器
    expect(await app.getByTestId('workspace-selector-trigger').count()).toBe(1)
    expect(screen()).toContain(label2)

    // 应该渲染居中的 Composer 输入框
    expect(await app.getByTestId('composer').count()).toBe(1)
    expect(screen()).toContain('Ask anything')

    await app.close()
    for (const dir of dirs) await rm(dir, { recursive: true, force: true }).catch(() => {})
  })
})
