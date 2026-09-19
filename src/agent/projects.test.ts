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
})
