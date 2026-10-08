/**
 * 传输选择（M3-6）。
 *
 * 默认 `inprocess`（开发与测试：不起进程、不起端口）；`ws` 时**自己 spawn 自己当主机**，
 * 然后走 WebSocket——打包形态走这条，但用户看不见：交付物仍是一个 exe。
 *
 * 这里验的是"选了 ws 之后，界面拿到的东西与进程内**同形**"，以及**收尾不留主机**。
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { store } from '../../agent/store'
import { resolveAgentClient } from './index'

type Resolved = Awaited<ReturnType<typeof resolveAgentClient>>

const opened: Resolved[] = []

afterEach(() => {
  for (const resolved of opened.splice(0)) resolved.shutdown()
})

describe('传输选择', () => {
  test('默认是进程内：不起子进程、不起端口', async () => {
    const resolved = await resolveAgentClient({ transport: 'inprocess' })
    opened.push(resolved)

    expect(resolved.info.transport).toBe('inprocess')
    expect(resolved.info.port).toBeUndefined()
    expect(resolved.client.state.threads.length).toBe(store.threads.length)
  })

  test(
    '选 ws：自己 spawn 主机、客户端同形可用，shutdown 之后主机不再存活',
    async () => {
      const resolved = await resolveAgentClient({ transport: 'ws' })
      opened.push(resolved)

      // 真的起了主机进程与端口
      expect(resolved.info.transport).toBe('ws')
      expect(resolved.info.port ?? 0).toBeGreaterThan(0)
      expect(resolved.info.pid ?? 0).toBeGreaterThan(0)

      // 界面拿到的东西与进程内同形：同一套读法、同一套命令
      expect(resolved.client.state.threads.length).toBeGreaterThan(0)
      expect(resolved.client.state.activeId.length).toBeGreaterThan(0)
      const { threadId } = await resolved.client.request('thread.create', { workspace: store.project })
      await store.deleteThread(threadId)

      // 收尾：shutdown 之后主机进程真的没了（不留孤儿）
      const pid = resolved.info.pid!
      resolved.shutdown()
      const started = Date.now()
      let alive = true
      while (alive && Date.now() - started < 8000) {
        try {
          process.kill(pid, 0)
          await new Promise((resolve) => setTimeout(resolve, 50))
        } catch {
          alive = false
        }
      }
      expect(alive).toBe(false)
      opened.splice(opened.indexOf(resolved), 1)
    },
    40_000
  )
})
