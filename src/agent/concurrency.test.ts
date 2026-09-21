/**
 * Multi-session concurrency tests.
 *
 * Verifies that multiple threads can run in parallel without blocking each other,
 * queues are isolated per thread, stop targets specific threads, and running threads
 * are protected from deletion.
 */

import { afterAll, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { store } from './store'

const dirs: string[] = []

async function project(name: string): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), `a-da-concurrency-${name}-`))
  dirs.push(dir)
  return dir
}

afterAll(async () => {
  for (const dir of dirs) await rm(dir, { recursive: true, force: true })
})

describe('multi-session concurrency', () => {
  test('isThreadRunning tracks multiple concurrent threads independently', async () => {
    const ws1 = await project('ws1')
    const ws2 = await project('ws2')
    const t1 = store.newThread(ws1)
    const t2 = store.newThread(ws2)
    const t3 = store.newThread(ws1)

    const mutable = store as unknown as {
      runningThreadIds: Set<string>
      notify: () => void
    }

    expect(store.isThreadRunning(t1.id)).toBe(false)
    expect(store.isThreadRunning(t2.id)).toBe(false)
    expect(store.isThreadRunning(t3.id)).toBe(false)

    // Start t1 and t2 concurrently
    mutable.runningThreadIds.add(t1.id)
    mutable.runningThreadIds.add(t2.id)

    expect(store.isThreadRunning(t1.id)).toBe(true)
    expect(store.isThreadRunning(t2.id)).toBe(true)
    expect(store.isThreadRunning(t3.id)).toBe(false)

    // store.running reflects active thread
    store.selectThread(t1.id)
    expect(store.running).toBe(true)

    store.selectThread(t3.id)
    expect(store.running).toBe(false)

    store.selectThread(t2.id)
    expect(store.running).toBe(true)

    // Clean up
    mutable.runningThreadIds.clear()
  })

  test('queue is isolated per thread when threads are running', async () => {
    const ws = await project('ws_queue')
    const t1 = store.newThread(ws)
    const t2 = store.newThread(ws)

    const mutable = store as unknown as {
      runningThreadIds: Set<string>
      notify: () => void
    }

    // Mark t1 as running
    mutable.runningThreadIds.add(t1.id)

    // Select t1 and send a follow-up prompt -> should queue for t1
    store.selectThread(t1.id)
    store.send('queued follow up for t1')

    expect(store.queue.length).toBe(1)
    expect(store.queue[0]?.text).toBe('queued follow up for t1')
    expect(t1.items.some((item) => item.kind === 'user' && item.queued)).toBe(true)

    // Switch to t2 (which is NOT running in this mock, but we don't call offlineTurn directly here)
    store.selectThread(t2.id)
    expect(store.queue.length).toBe(0)

    // Clean up
    mutable.runningThreadIds.clear()
    store.stop(t1.id)
  })

  test('stop(targetId) only stops the targeted thread', async () => {
    const ws = await project('ws_stop')
    const t1 = store.newThread(ws)
    const t2 = store.newThread(ws)

    const mutable = store as unknown as {
      runningThreadIds: Set<string>
      aborts: Map<string, AbortController>
    }

    const abort1 = new AbortController()
    const abort2 = new AbortController()
    mutable.runningThreadIds.add(t1.id)
    mutable.runningThreadIds.add(t2.id)
    mutable.aborts.set(t1.id, abort1)
    mutable.aborts.set(t2.id, abort2)

    expect(store.isThreadRunning(t1.id)).toBe(true)
    expect(store.isThreadRunning(t2.id)).toBe(true)

    // Stop t1 specifically
    store.stop(t1.id)
    expect(abort1.signal.aborted).toBe(true)
    expect(abort2.signal.aborted).toBe(false)

    // Simulate drain cleanup on t1 abort
    mutable.runningThreadIds.delete(t1.id)
    mutable.aborts.delete(t1.id)

    expect(store.isThreadRunning(t1.id)).toBe(false)
    expect(store.isThreadRunning(t2.id)).toBe(true)

    // Now stop t2
    store.stop(t2.id)
    expect(abort2.signal.aborted).toBe(true)

    mutable.runningThreadIds.clear()
    mutable.aborts.clear()
  })

  test('deleteThread and removeProject protect running threads in concurrent environment', async () => {
    const ws1 = await project('ws_del1')
    const ws2 = await project('ws_del2')
    const t1 = store.newThread(ws1)
    const t2 = store.newThread(ws2)

    const mutable = store as unknown as {
      runningThreadIds: Set<string>
    }

    mutable.runningThreadIds.add(t1.id)

    // t1 is running -> cannot delete t1
    expect(store.deleteThread(t1.id)).toBe('这个会话正在运行，先停止再删除')
    // ws1 has running thread -> cannot remove ws1
    expect(store.removeProject(ws1)).toBe('该工作区内有会话正在运行，先停止再移除')

    // ws2 has no running threads -> can remove ws2 (as long as >1 project exists)
    expect(store.projects.length).toBeGreaterThan(1)
    expect(store.removeProject(ws2)).toBe(null)

    // Stop t1
    mutable.runningThreadIds.delete(t1.id)
    expect(store.isThreadRunning(t1.id)).toBe(false)
  })

  test('concurrent execution allows independent background progression', async () => {
    const ws = await project('ws_drain_async')
    const t1 = store.newThread(ws)
    const t2 = store.newThread(ws)

    let t1Resolve: () => void = () => {}
    const t1Promise = new Promise<void>((r) => {
      t1Resolve = r
    })

    const mutable = store as unknown as {
      turn: (thread: unknown, prompt: string) => Promise<void>
    }

    const origTurn = mutable.turn.bind(store)
    mutable.turn = async (thread: unknown, _prompt: string) => {
      const thr = thread as { id: string }
      if (thr.id === t1.id) {
        await t1Promise
      } else {
        await new Promise((r) => setTimeout(r, 20))
      }
    }

    try {
      // Send to t1 -> starts running and waits on t1Promise
      store.selectThread(t1.id)
      store.send('hello t1')
      expect(store.isThreadRunning(t1.id)).toBe(true)

      // Switch to t2 and send -> t2 should run concurrently, NOT blocked by t1!
      store.selectThread(t2.id)
      store.send('hello t2')
      expect(store.isThreadRunning(t2.id)).toBe(true)
      expect(store.isThreadRunning(t1.id)).toBe(true)

      // Wait for t2 to complete while t1 is still suspended
      await new Promise((r) => setTimeout(r, 60))
      expect(store.isThreadRunning(t2.id)).toBe(false)
      expect(store.isThreadRunning(t1.id)).toBe(true)

      // Resume and finish t1
      t1Resolve()
      await new Promise((r) => setTimeout(r, 60))
      expect(store.isThreadRunning(t1.id)).toBe(false)
    } finally {
      mutable.turn = origTurn
      t1Resolve()
    }
  })
})

