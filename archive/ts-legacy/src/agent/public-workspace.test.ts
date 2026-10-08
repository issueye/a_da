/**
 * 公共区在 store 上的行为：新建对话时可以选它，它的目录由 a-da 建出来。
 *
 * 公共区路径**注入**给 `AgentStore`（构造函数的第二个参数），不去改进程级的
 * `A_DA_HOME`：那是并发跑在同一进程里的各测试文件共享的，动它会让 `restore.test.ts`
 * 之类的用例读到别人的 home（AGENTS.md §13 末尾——实测过，确实会红）。
 *
 * 也不使用模块级的 `store` 单例：把公共区塞进去会让 `projects.test.ts` 里
 * 「只剩一个工作区」的清理循环永远差一个删不掉的工作区。
 */

import { afterAll, describe, expect, test } from 'bun:test'
import { existsSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { AgentStore } from './store'
import { publicWorkspaceOf } from './home'

const dirs: string[] = []

afterAll(async () => {
  for (const dir of dirs) await rm(dir, { recursive: true, force: true })
})

/**
 * 建一个完全隔离的 store：项目目录、应用数据目录、公共区都在自己的临时目录里。
 *
 * 三者都给同一份临时根，这样公共区、会话落盘、项目目录互不干扰别人。
 */
async function isolatedStore(): Promise<{ store: AgentStore; root: string; publicPath: string }> {
  const root = await mkdtemp(join(tmpdir(), 'a-da-public-'))
  dirs.push(root)
  const projectDir = await mkdtemp(join(tmpdir(), 'a-da-public-proj-'))
  dirs.push(projectDir)
  const publicPath = publicWorkspaceOf(root)
  return { store: new AgentStore(projectDir, publicPath), root, publicPath }
}

describe('the public workspace', () => {
  test('opening it creates the directory and pins the new thread', async () => {
    const { store, publicPath } = await isolatedStore()
    const thread = store.newThread()
    expect(store.isPublic(thread.workspace)).toBe(false)

    await store.openPublicWorkspace(thread.id)

    // 目录真的建出来了（此前它不存在），会话绑过去、成了当前工作区。
    expect(existsSync(publicPath)).toBe(true)
    expect(thread.workspace).toBe(publicPath)
    expect(store.project).toBe(publicPath)
  })

  test('opening it without a thread starts a conversation there', async () => {
    const { store, publicPath } = await isolatedStore()
    const before = store.threads.length

    await store.openPublicWorkspace()

    expect(store.threads.length).toBe(before + 1)
    expect(store.active.workspace).toBe(publicPath)
  })

  test('it is a project you cannot remove', async () => {
    const { store, publicPath } = await isolatedStore()
    await store.openPublicWorkspace()

    expect(store.projects).toContain(publicPath)
    expect(store.removeProject(store.project)).toBe('公共区由 a-da 提供，不能移除')
    // 拒绝之后它还在，会话也没被动。
    expect(store.projects).toContain(publicPath)
    expect(store.project).toBe(publicPath)
  })

  test('its row is labelled 公共区, not the implementation path', async () => {
    const { store, publicPath } = await isolatedStore()
    await store.openPublicWorkspace()

    expect(store.labelFor(publicPath)).toBe('公共区')
    expect(store.isPublic(publicPath)).toBe(true)
  })
})
