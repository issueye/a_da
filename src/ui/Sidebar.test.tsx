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

      // 新建会话会写一份流水；写盘是异步的，先等它真的在。
      const sessionFile = join(getSessionsDir(), `${doomed.id}.jsonl`)
      await app.getByTestId(`thread-${doomed.id}`).waitFor({ timeoutMs: 10_000 })
      await until(() => existsSync(sessionFile))

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
    'a picked directory is added without touching the path field',
    async () => {
      const workspace = await project()
      setDirectoryPicker(async () => ({ status: 'picked', path: workspace }))

      const { app, painted } = await mount()
      await app.getByTestId('add-project').click()
      await painted(shortPath(workspace, 2))

      expect(store.project).toBe(workspace)
      // 加成功了就没有理由再把输入框留在那儿。
      await new Promise((resolve) => setTimeout(resolve, 200))
      expect(await app.getByTestId('project-path').count()).toBe(0)

      await app.close()
    },
    30_000,
  )

  test(
    'falls back to typing a path when no dialog can open',
    async () => {
      const workspace = await project()
      const { app, painted } = await mount()

      await app.getByTestId('add-project').click()
      const field = app.getByTestId('project-path')
      await field.waitFor({ timeoutMs: 10_000 })

      await field.fill(workspace)
      await field.press('enter')
      await painted(shortPath(workspace, 2))
      expect(store.project).toBe(workspace)

      await app.close()
    },
    30_000,
  )
})
