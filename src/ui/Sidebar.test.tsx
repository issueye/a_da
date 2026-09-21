/**
 * 侧边栏：会话的右键删除，以及「添加项目」开目录选择弹窗。
 *
 * 真弹窗会把自动化卡住，所以目录选择器在这里被换成桩（`setDirectoryPicker`）；
 * 没有桩的时候也走不到开窗口那一步，因为 scripts/test-preload.ts 设了
 * `A_DA_NO_DIALOG=1`。
 */

import { afterAll, describe, expect, test } from 'bun:test'
import { existsSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { AgentWindow } from '../AgentWindow'
import { getSessionsDir } from '../agent/session/manager'
import { defaultSessionManager } from '../agent/session/manager'
import { store } from '../agent/store'
import { setDirectoryPicker } from '../platform/dialog'
import { shortPath } from '../theme'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const dirs: string[] = []

async function project(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), 'a-da-sidebar-'))
  dirs.push(dir)
  return dir
}

afterAll(async () => {
  setDirectoryPicker(null)
  for (const dir of dirs) await rm(dir, { recursive: true, force: true })
})

/**
 * 等一个会话的流水文件真的写出来，返回它的路径。
 *
 * 会话按「工作区/会话」分目录存放，目录名是工作区的散列，所以路径只能问管理器要，
 * 不能自己拼——拼一个扁平路径会等到超时。
 */
async function untilFile(sessionId: string, workspace: string): Promise<string> {
  const started = Date.now()
  while (Date.now() - started < 10_000) {
    const summary = (await defaultSessionManager.listSessionsForWorkspace(workspace)).find(
      (candidate) => candidate.id === sessionId,
    )
    if (summary && existsSync(summary.filePath)) return summary.filePath
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
  throw new Error(`the session file for ${sessionId} never appeared under ${getSessionsDir()}`)
}

async function mount() {
  const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
  render(<AgentWindow />)
  const app = await connectTest(renderer)
  const screen = () => renderer.getPaintedText().join('\n')

  /** Wait for a string to be painted, or for it to disappear when needle is ''. */
  const painted = async (needle: string, timeoutMs = 15_000): Promise<void> => {
    const started = Date.now()
    while (Date.now() - started < timeoutMs) {
      if (screen().includes(needle)) return
      renderer.flush?.()
      await new Promise((resolve) => setTimeout(resolve, 50))
    }
    throw new Error(`the frame never painted ${needle}\n${screen()}`)
  }

  const gone = async (needle: string, timeoutMs = 15_000): Promise<void> => {
    const started = Date.now()
    while (Date.now() - started < timeoutMs) {
      if (!screen().includes(needle)) return
      renderer.flush?.()
      await new Promise((resolve) => setTimeout(resolve, 50))
    }
    throw new Error(`the frame still paints ${needle}\n${screen()}`)
  }

  const until = async (check: () => Promise<boolean> | boolean, timeoutMs = 15_000): Promise<void> => {
    const started = Date.now()
    while (Date.now() - started < timeoutMs) {
      if (await check()) return
      renderer.flush?.()
      await new Promise((resolve) => setTimeout(resolve, 50))
    }
    throw new Error('condition never became true')
  }

  return { app, screen, painted, gone, until }
}

describeNative('sidebar sessions', () => {
  test(
    'the trash icon deletes a session in two clicks, JSONL included',
    async () => {
      const workspace = await project()
      const doomed = store.newThread(workspace)
      const { app, painted, gone, until } = await mount()

      // 新建会话会写一份流水；写盘是异步的，先等它真的在。路径按「工作区/会话」
      // 分目录，所以这里问管理器要，而不是自己拼一个扁平路径。
      const sessionFile = await untilFile(doomed.id, workspace)
      await app.getByTestId(`thread-${doomed.id}`).waitFor({ timeoutMs: 10_000 })

      // 第一下只是把垃圾桶变成确认，会话还在。
      await app.getByTestId(`delete-thread-${doomed.id}`).click()
      await painted('确认删除')
      expect(store.threads.some((thread) => thread.id === doomed.id)).toBe(true)

      await app.getByTestId(`delete-thread-${doomed.id}`).click()
      await gone('确认删除')
      await until(() => !store.threads.some((thread) => thread.id === doomed.id))
      await until(() => !existsSync(sessionFile))

      await app.close()
    },
    30_000,
  )

  test(
    'the trash icon does not also open the session it belongs to',
    async () => {
      const workspace = await project()
      const older = store.newThread(workspace)
      const newer = store.newThread(workspace)
      store.selectThread(older.id)

      const { app, painted } = await mount()
      await app.getByTestId(`thread-${newer.id}`).waitFor({ timeoutMs: 10_000 })
      await app.getByTestId(`delete-thread-${newer.id}`).click()
      await painted('确认删除')

      // 图标和选择区是兄弟，点它不该顺手把会话切过去。
      expect(store.activeId).toBe(older.id)

      await app.close()
    },
    30_000,
  )

  test(
    'right-clicking a session row does nothing any more',
    async () => {
      const workspace = await project()
      const only = store.newThread(workspace)
      const { app } = await mount()
      const row = app.getByTestId(`thread-${only.id}`)
      await row.waitFor({ timeoutMs: 10_000 })

      await row.click({ button: 2 })
      await new Promise((resolve) => setTimeout(resolve, 300))

      expect(store.threads.some((thread) => thread.id === only.id)).toBe(true)
      expect(store.activeId).toBe(only.id)

      await app.close()
    },
    30_000,
  )

  test(
    'deleting the last session of a workspace keeps the workspace open',
    async () => {
      const workspace = await project()
      const only = store.newThread(workspace)
      const { app, painted } = await mount()
      await app.getByTestId(`thread-${only.id}`).waitFor({ timeoutMs: 10_000 })

      store.deleteThread(only.id)
      await painted('新会话')

      // 会话换了新的，但那个工作区还在——用户删的是一个会话。
      expect(store.project).toBe(workspace)
      expect(store.projectThreads).toHaveLength(1)
      expect(store.projectThreads[0]!.id).not.toBe(only.id)

      await app.close()
    },
    30_000,
  )
})

describeNative('sidebar add project', () => {
  test(
    'sidebar no longer renders add project buttons',
    async () => {
      const { app } = await mount()
      expect(await app.getByTestId('add-project').count()).toBe(0)
      expect(await app.getByTestId('header-add-project').count()).toBe(0)
      await app.close()
    },
    30_000,
  )

  test(
    'the trash icon removes a project in two clicks',
    async () => {
      const keep = await project()
      const doomed = await project()
      store.newThread(keep)
      store.newThread(doomed)
      store.selectProject(doomed)

      const doomedLabel = shortPath(doomed, 2)
      const { app, painted, gone, until } = await mount()

      await app.getByTestId(`project-${doomedLabel}`).waitFor({ timeoutMs: 10_000 })
      expect(await app.getByTestId(`remove-project-${doomedLabel}`).count()).toBe(1)

      // 第一下点击变红提示「确认移除」
      await app.getByTestId(`remove-project-${doomedLabel}`).click()
      await painted('确认移除')
      expect(store.projects).toContain(doomed)

      // 第二下点击正式移除
      await app.getByTestId(`remove-project-${doomedLabel}`).click()
      await gone('确认移除')
      await until(() => !store.projects.includes(doomed))
      expect(store.projects).not.toContain(doomed)

      await app.close()
    },
    30_000,
  )

  test(
    'sidebar new chat button creates a new thread',
    async () => {
      const workspace = await project()
      store.newThread(workspace)
      const countBefore = store.threads.length

      const { app, painted } = await mount()
      await app.getByTestId('sidebar-new-chat').click()
      await painted('新会话')

      expect(store.threads.length).toBe(countBefore + 1)
      expect(store.active.title).toBe('新会话')

      await app.close()
    },
    30_000,
  )
})
