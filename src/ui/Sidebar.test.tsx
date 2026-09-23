/**
 * 侧边栏：会话的右键删除，以及「添加项目」开目录选择弹窗。
 *
 * 真弹窗会把自动化卡住，所以目录选择器在这里被换成桩（`setDirectoryPicker`）；
 * 没有桩的时候也走不到开窗口那一步，因为 scripts/test-preload.ts 设了
 * `A_DA_NO_DIALOG=1`。
 */

import { afterAll, beforeEach, describe, expect, test } from 'bun:test'
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
import { setExplorerOpener } from '../platform/explorer'
import { shortPath } from '../theme'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const dirs: string[] = []

beforeEach(() => {
  store.closeConfirm()
})

async function project(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), 'a-da-sidebar-'))
  dirs.push(dir)
  return dir
}

afterAll(async () => {
  setDirectoryPicker(null)
  setExplorerOpener(null)
  store.closeConfirm()
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

      // 第一下打开二次确认弹窗，会话还在。
      await app.getByTestId(`delete-thread-${doomed.id}`).click()
      await painted('确认删除')
      expect(store.threads.some((thread) => thread.id === doomed.id)).toBe(true)

      // 在确认弹窗中点击确认删除
      await app.getByTestId('confirm-dialog-confirm').click()
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

      store.closeConfirm()
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

  test(
    'subagent session renders as child item under parent in sidebar and composer is read-only',
    async () => {
      const workspace = await project()
      const parent = store.newThread(workspace)
      store.selectThread(parent.id)
      const { thread: subagent } = await store.startSubagentThread({
        subagentId: 'code_reviewer',
        task: '审核UI代码',
      })
      const { app, painted, until } = await mount()
      await app.getByTestId(`thread-${parent.id}`).waitFor({ timeoutMs: 10_000 })
      await app.getByTestId(`thread-${subagent.id}`).waitFor({ timeoutMs: 10_000 })

      // 验证子智能体已作为子项渲染并显示标题
      await painted('审核UI代码')

      // 验证子智能体会话选中时 Composer 处于只读状态并显示「子智能体专属执行会话」和「返回主会话」
      store.selectThread(subagent.id)
      await painted('子智能体专属执行会话')
      await painted('返回主会话')

      // 点击返回主会话可切回父级会话
      await app.getByTestId('return-parent-thread').click()
      await until(() => store.activeId === parent.id)
      expect(store.activeId).toBe(parent.id)

      await app.close()
    },
    30_000,
  )

  test(
    'supports collapsing and expanding subagent session list under parent thread',
    async () => {
      const workspace = await project()
      const parent = store.newThread(workspace)
      store.selectThread(parent.id)
      const { thread: subagent } = await store.startSubagentThread({
        subagentId: 'code_reviewer',
        task: '折叠测试任务',
      })
      const { app, painted, gone, until } = await mount()
      await app.getByTestId(`thread-${parent.id}`).waitFor({ timeoutMs: 10_000 })
      await app.getByTestId(`thread-${subagent.id}`).waitFor({ timeoutMs: 10_000 })

      // 初始默认展开，展示子会话
      expect(await app.getByTestId(`thread-${subagent.id}`).count()).toBe(1)
      expect(await app.getByTestId(`toggle-subagents-${parent.id}`).count()).toBe(1)

      // 点击收起箭头
      await app.getByTestId(`toggle-subagents-${parent.id}`).click()
      await until(async () => (await app.getByTestId(`thread-${subagent.id}`).count()) === 0)
      expect(await app.getByTestId(`thread-${subagent.id}`).count()).toBe(0)

      // 再次点击展开箭头
      await app.getByTestId(`toggle-subagents-${parent.id}`).click()
      await until(async () => (await app.getByTestId(`thread-${subagent.id}`).count()) === 1)
      expect(await app.getByTestId(`thread-${subagent.id}`).count()).toBe(1)

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

      // 第一下点击弹出确认弹窗
      await app.getByTestId(`remove-project-${doomedLabel}`).click()
      await painted('确认移除')
      expect(store.projects).toContain(doomed)

      // 在确认弹窗中点击确认移除
      await app.getByTestId('confirm-dialog-confirm').click()
      await gone('确认移除')
      await until(() => !store.projects.includes(doomed))
      expect(store.projects).not.toContain(doomed)

      await app.close()
    },
    30_000,
  )

  test(
    'the open in explorer button opens the workspace directory',
    async () => {
      const workspace = await project()
      store.newThread(workspace)
      store.selectProject(workspace)

      const label = shortPath(workspace, 2)
      const recorded: string[] = []
      setExplorerOpener((p) => {
        recorded.push(p)
        return true
      })

      const { app } = await mount()
      await app.getByTestId(`project-${label}`).waitFor({ timeoutMs: 10_000 })
      expect(await app.getByTestId(`open-explorer-${label}`).count()).toBe(1)

      await app.getByTestId(`open-explorer-${label}`).click()
      expect(recorded).toContain(workspace)

      setExplorerOpener(null)
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

  test(
    'long thread and subagent title renders in sidebar with ellipsis truncation',
    async () => {
      const workspace = await project()
      const longTitle = '这是一个非常非常长的会话标题用于测试侧边栏标题超长截断并显示省略号的效果'
      const thread = store.newThread(workspace)
      thread.title = longTitle

      const { app, painted } = await mount()
      await app.getByTestId(`thread-${thread.id}`).waitFor({ timeoutMs: 10_000 })
      await painted(longTitle)

      await app.close()
    },
    30_000,
  )
})
