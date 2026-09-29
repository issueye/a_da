import { afterAll, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { AgentStore, store } from '../agent/store'
import { publicWorkspaceOf } from '../agent/home'
import { EmptyConversationView } from './EmptyConversationView'
import { shortPath } from '../theme'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const dirs: string[] = []

async function project(name: string): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), `a-da-empty-${name}-`))
  dirs.push(dir)
  return dir
}

afterAll(async () => {
  for (const dir of dirs) await rm(dir, { recursive: true, force: true })
})

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
  })

  /**
   * 「新建对话时可选公共区」的用户路径。
   *
   * 用独立的 `AgentStore` 并把公共区路径**注入**进去，不碰全局 `store` 单例：把公共区
   * 塞进单例会让 `projects.test.ts` 里「只剩一个工作区」的清理循环卡在一个删不掉的工作区上。
   * 窗口用完关掉，同一时刻只有一个真窗口（AGENTS.md §13 末尾）。
   */
  test('the selector offers 公共区 and binds the conversation to it', async () => {
    const home = await mkdtemp(join(tmpdir(), 'a-da-empty-public-home-'))
    const projectDir = await project('public')
    dirs.push(home)
    const publicPath = publicWorkspaceOf(home)
    const localStore = new AgentStore(projectDir, publicPath)

    const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
    render(<EmptyConversationView store={localStore} />)
    const app = await connectTest(renderer)

    try {
      await app.getByTestId('workspace-selector-trigger').click()
      const option = app.getByTestId('select-workspace-public')
      await option.waitFor({ timeoutMs: 10_000 })

      // 下拉里就是「公共区」，不暴露实现路径。
      expect(renderer.getPaintedText().join('\n')).toContain('公共区')

      await option.click()
      const started = Date.now()
      while (localStore.active.workspace !== publicPath && Date.now() - started < 10_000) {
        await new Promise((resolve) => setTimeout(resolve, 50))
      }

      expect(localStore.active.workspace).toBe(publicPath)
      expect(localStore.labelFor(publicPath)).toBe('公共区')
    } finally {
      await app.close()
    }
  })
})
