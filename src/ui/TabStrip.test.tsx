/**
 * 标签页：切换、新建、关闭。
 *
 * 最要紧的一条是**关标签不删会话**：标签是视图，会话是数据。关掉之后会话还在
 * `store.threads` 里、侧边栏照样列着它，从侧边栏点一下标签就回来了。这也正是
 * 关闭不需要二次确认的原因（对比侧边栏的垃圾桶：那个删数据，要点两下）。
 */

import { afterAll, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { AgentWindow } from '../AgentWindow'
import { store } from '../agent/store'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const dirs: string[] = []

async function project(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), 'a-da-tabs-'))
  dirs.push(dir)
  return dir
}

afterAll(async () => {
  for (const dir of dirs) await rm(dir, { recursive: true, force: true })
})

async function mount() {
  const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
  render(<AgentWindow />)
  const app = await connectTest(renderer)
  const screen = () => renderer.getPaintedText().join('\n')

  const until = async (check: () => boolean | Promise<boolean>, timeoutMs = 15_000): Promise<void> => {
    const started = Date.now()
    while (Date.now() - started < timeoutMs) {
      if (await check()) return
      renderer.flush?.()
      await new Promise((resolve) => setTimeout(resolve, 50))
    }
    throw new Error(`condition never became true\n${screen()}`)
  }

  return { app, screen, until, renderer, render }
}

describeNative('tab strip', () => {
  beforeEach(() => {
    store.openTabIds = []
  })

  test(
    'shows a tab per open session and switches on click',
    async () => {
      const workspace = await project()
      const first = store.newThread(workspace)
      const second = store.newThread(workspace)
      store.selectThread(first.id)

      const { app } = await mount()
      await app.getByTestId(`tab-${first.id}`).waitFor({ timeoutMs: 10_000 })
      expect(await app.getByTestId(`tab-${second.id}`).count()).toBe(1)
      expect(store.activeId).toBe(first.id)

      await app.getByTestId(`tab-${second.id}`).click()
      expect(store.activeId).toBe(second.id)

      await app.close()
    },
    30_000,
  )

  test(
    'closing a tab keeps the session, and the sidebar can open it again',
    async () => {
      const workspace = await project()
      const first = store.newThread(workspace)
      const second = store.newThread(workspace)
      store.selectThread(second.id)

      const { app } = await mount()
      await app.getByTestId(`tab-${first.id}`).waitFor({ timeoutMs: 10_000 })

      // 关掉第一个标签：标签没了……
      await app.getByTestId(`close-tab-${first.id}`).click()
      expect(await app.getByTestId(`tab-${first.id}`).count()).toBe(0)
      // ……但会话还在，盘上那份流水也没动。
      expect(store.threads.some((thread) => thread.id === first.id)).toBe(true)
      expect(store.projectThreads.some((thread) => thread.id === first.id)).toBe(true)

      // 从侧边栏点一下，标签就回来了（侧边栏的行和标签是同一个 testId 前缀不同）。
      await app.getByTestId(`thread-${first.id}`).click()
      await app.getByTestId(`tab-${first.id}`).waitFor({ timeoutMs: 10_000 })
      expect(store.activeId).toBe(first.id)

      await app.close()
    },
    30_000,
  )

  test(
    'closing the active tab hands over to another open tab',
    async () => {
      const workspace = await project()
      const first = store.newThread(workspace)
      const second = store.newThread(workspace)
      store.selectThread(second.id)

      const { app } = await mount()
      await app.getByTestId(`close-tab-${second.id}`).click()

      // 关的是当前会话，指针要落到还开着的那个标签上，而不是指向一个看不见的会话。
      expect(store.activeId).toBe(first.id)

      await app.close()
    },
    30_000,
  )

  test(
    'the last tab of a project has no close button',
    async () => {
      const workspace = await project()
      const only = store.newThread(workspace)
      // 只留这一个标签，别的项目的不算。
      store.openTabIds = [only.id]
      store.selectThread(only.id)
      expect(store.openTabs).toHaveLength(1)

      const { app } = await mount()
      await app.getByTestId(`tab-${only.id}`).waitFor({ timeoutMs: 10_000 })

      // 关掉它会让窗口没内容可显示，所以干脆不给 ×。
      expect(await app.getByTestId(`close-tab-${only.id}`).count()).toBe(0)
      expect(store.threads.some((thread) => thread.id === only.id)).toBe(true)

      await app.close()
    },
    30_000,
  )

  test(
    'the plus button opens a session in the current project',
    async () => {
      const workspace = await project()
      store.newThread(workspace)
      const before = store.threads.length

      const { app } = await mount()
      await app.getByTestId('new-tab').click()

      expect(store.threads.length).toBe(before + 1)
      expect(store.project).toBe(workspace)
      // 新建的成为当前会话，标签条上它就是选中那个。
      expect(store.activeId).toBe(store.threads[0]!.id)
      expect(store.openTabs.some((thread) => thread.id === store.activeId)).toBe(true)

      await app.close()
    },
    30_000,
  )

  test(
    'a tab and its close button are siblings, so closing does not also switch',
    async () => {
      const workspace = await project()
      const keep = store.newThread(workspace)
      const doomed = store.newThread(workspace)
      store.selectThread(keep.id)

      const { app } = await mount()
      await app.getByTestId(`tab-${doomed.id}`).waitFor({ timeoutMs: 10_000 })

      // 点另一个标签的 × 不该把它切过去。
      await app.getByTestId(`close-tab-${doomed.id}`).click()
      expect(store.activeId).toBe(keep.id)

      await app.close()
    },
    30_000,
  )

  test(
    'the running session has no close button, so it stays in view mid-turn',
    async () => {
      const workspace = await project()
      const busy = store.newThread(workspace)
      store.newThread(workspace)

      const { app, renderer, until } = await mount()
      await app.getByTestId(`tab-${busy.id}`).waitFor({ timeoutMs: 10_000 })

      // 把 store 摆成「这个会话正在跑」，标签条据此隐藏 ×。
      const mutable = store as unknown as { runningThreadId: string | null; notify: () => void }
      mutable.runningThreadId = busy.id
      mutable.notify()
      renderer.flush()
      await until(async () => (await app.getByTestId(`close-tab-${busy.id}`).count()) === 0)

      expect(await app.getByTestId(`close-tab-${busy.id}`).count()).toBe(0)
      expect(store.openTabs.some((thread) => thread.id === busy.id)).toBe(true)

      mutable.runningThreadId = null
      await app.close()
    },
    30_000,
  )

  test(
    'deleting a session from the sidebar also drops its tab',
    async () => {
      const workspace = await project()
      const doomed = store.newThread(workspace)
      store.newThread(workspace)
      expect(store.openTabs.some((thread) => thread.id === doomed.id)).toBe(true)

      // 删数据顺带收掉视图：会话都没了，标签留着没有意义。
      store.deleteThread(doomed.id)

      expect(store.threads.some((thread) => thread.id === doomed.id)).toBe(false)
      expect(store.openTabIds.includes(doomed.id)).toBe(false)
    },
    30_000,
  )

  test(
    'tabs show all open sessions across workspaces and switch on select',
    async () => {
      const alpha = await project()
      const beta = await project()
      const inAlpha = store.newThread(alpha)
      const inBeta = store.newThread(beta)

      expect(store.project).toBe(beta)
      // 会话页签不再针对某个工作区过滤，所有点开的会话都在上面显示
      expect(store.openTabs.some((thread) => thread.id === inBeta.id)).toBe(true)
      expect(store.openTabs.some((thread) => thread.id === inAlpha.id)).toBe(true)

      // 切换会话激活状态与工作区，且两者的标签均保留在标签栏
      store.selectThread(inAlpha.id)
      expect(store.project).toBe(alpha)
      expect(store.openTabs.some((thread) => thread.id === inAlpha.id)).toBe(true)
      expect(store.openTabs.some((thread) => thread.id === inBeta.id)).toBe(true)
    },
    30_000,
  )
})
