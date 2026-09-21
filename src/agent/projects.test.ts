/**
 * Projects.
 *
 * A project exists exactly as long as it has a thread, and a thread is pinned
 * to one workspace for its whole life — that is what keeps a running agent from
 * touching a folder the user switched away from.
 */

import { afterAll, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { store } from './store'

const dirs: string[] = []

async function project(name: string): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), `a-da-${name}-`))
  dirs.push(dir)
  return dir
}

afterAll(async () => {
  for (const dir of dirs) await rm(dir, { recursive: true, force: true })
})

describe('projects', () => {
  test('a new thread pins its own project', async () => {
    const alpha = await project('alpha')
    const beta = await project('beta')

    const first = store.newThread(alpha)
    expect(first.workspace).toBe(alpha)
    expect(store.project).toBe(alpha)
    expect(store.projectThreads.every((thread) => thread.workspace === alpha)).toBe(true)

    store.newThread(beta)
    expect(store.project).toBe(beta)
    expect(store.projects.slice(0, 2)).toEqual([beta, alpha])
  })

  test('selecting a project returns to its newest thread instead of adding one', async () => {
    const alpha = await project('alpha2')
    const beta = await project('beta2')
    const alphaThread = store.newThread(alpha)
    store.newThread(beta)
    const before = store.threads.length

    store.selectProject(alpha)
    expect(store.project).toBe(alpha)
    expect(store.active.id).toBe(alphaThread.id)
    expect(store.threads.length).toBe(before)
  })

  test('a project with no thread yet gets one when it is opened', async () => {
    const fresh = await project('fresh')
    const before = store.threads.length

    store.selectProject(fresh)
    expect(store.project).toBe(fresh)
    expect(store.threads.length).toBe(before + 1)
  })

  test('threads of other projects are hidden from the list', async () => {
    const alpha = await project('alpha3')
    const beta = await project('beta3')
    store.newThread(alpha)
    store.newThread(alpha)
    store.newThread(beta)

    expect(store.projectThreads.length).toBe(1)
    store.selectProject(alpha)
    expect(store.projectThreads.length).toBe(2)
  })

  test('adding a project checks the path first', async () => {
    const good = await project('good')
    await writeFile(join(good, 'file.txt'), 'x\n', 'utf8')
    await mkdir(join(good, 'nested'), { recursive: true })

    expect(await store.addProject(good)).toBe(null)
    expect(store.project).toBe(good)

    expect(await store.addProject(join(good, 'file.txt'))).toContain('不是目录')
    expect(await store.addProject(join(good, 'missing'))).toContain('路径不存在')
    expect(await store.addProject('   ')).toContain('请输入目录路径')
    // A path pasted from Explorer or a shell arrives quoted.
    expect(await store.addProject(`"${join(good, 'nested')}"`)).toBe(null)
    expect(store.project).toBe(join(good, 'nested'))
  })

  test('adding an already existing project switches to it without creating duplicate threads', async () => {
    const existing = await project('dup')
    store.newThread(existing)
    const countBefore = store.threads.filter((t) => t.workspace === existing).length

    // 切换到另一个项目
    const other = await project('other')
    store.newThread(other)
    expect(store.project).toBe(other)

    // 再次添加 existing：应该平滑切换回去，且不会创建多余的空会话
    const err = await store.addProject(existing)
    expect(err).toBe(null)
    expect(store.project).toBe(existing)
    expect(store.threads.filter((t) => t.workspace === existing).length).toBe(countBefore)
  })

  test('removing a project cleans threads, tabs and switches project', async () => {
    const p1 = await project('p1')
    const p2 = await project('p2')

    const t1 = store.newThread(p1)
    const t1Newer = store.newThread(p1)
    const t2 = store.newThread(p2)

    expect(store.projects).toContain(p1)
    expect(store.projects).toContain(p2)
    expect(store.project).toBe(p2)

    // 移除当前激活的 p2
    const err = store.removeProject(p2)
    expect(err).toBe(null)

    expect(store.projects).not.toContain(p2)
    expect(store.projects).toContain(p1)
    expect(store.project).toBe(p1)
    expect(store.activeId).toBe(t1Newer.id)
    expect(store.threads.some((t) => t.workspace === p2)).toBe(false)
    expect(store.openTabs.some((t) => t.workspace === p2)).toBe(false)
  })

  test('removeProject refuses when only one project remains', async () => {
    // 保证只剩一个项目
    while (store.projects.length > 1) {
      const extra = store.projects[store.projects.length - 1]!
      store.removeProject(extra)
    }

    const last = store.projects[0]!
    expect(store.projects.length).toBe(1)
    expect(store.removeProject(last)).toBe('至少保留一个工作区')
    expect(store.projects.length).toBe(1)
  })

  test('removeProject refuses when a thread in that project is running', async () => {
    const runProj = await project('run_proj')
    const otherProj = await project('other_proj')
    const runningThread = store.newThread(runProj)
    store.newThread(otherProj)

    const mutable = store as unknown as { runningThreadId: string | null }
    mutable.runningThreadId = runningThread.id

    expect(store.removeProject(runProj)).toBe('该工作区内有会话正在运行，先停止再移除')
    expect(store.projects).toContain(runProj)

    mutable.runningThreadId = null
    expect(store.removeProject(runProj)).toBe(null)
    expect(store.projects).not.toContain(runProj)
  })

  test('setThreadWorkspace switches thread workspace and updates active project', async () => {
    const wsA = await project('ws_a')
    const wsB = await project('ws_b')
    const thread = store.newThread(wsA)

    expect(thread.workspace).toBe(wsA)
    expect(store.project).toBe(wsA)

    store.setThreadWorkspace(thread.id, wsB)
    expect(thread.workspace).toBe(wsB)
    expect(store.project).toBe(wsB)
  })
})
