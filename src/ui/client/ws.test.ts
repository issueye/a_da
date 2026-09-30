/**
 * WebSocket 客户端的端到端测试（M3-3）。
 *
 * 真起主机服务端、真连 WebSocket、真发命令——验的是"UI 换成跨进程传输后行为等价"：
 * 同一个 `AgentClient` 接口，读的是复制视图，写的是协议命令，事件从 socket 来。
 *
 * 五条：
 * 1. **种子**：`ready()` 后 `state` 就是主机当前状态；
 * 2. **命令**：`request` 跨进程打到主机，结果与**事件**都对得上；
 * 3. **事件驱动**：主机侧状态一变，客户端自己会更新（不用轮询）；
 * 4. **UI 外壳动作**：走 `ui.setShell`，本机与跨进程同形；
 * 5. **对齐与失败都要如实**：`refreshState()` 能重新要快照；令牌错了就**连不上**（不假装连上）。
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { store } from '../../agent/store'
import { startHostServer, type HostServer } from '../../agent/host/server'
import { createWebSocketClient, type WebSocketClient } from './ws'

const servers: HostServer[] = []
const clients: WebSocketClient[] = []

afterEach(() => {
  for (const client of clients.splice(0)) client.close()
  for (const server of servers.splice(0)) server.stop()
})

function startHost(token = 'ws-client-token') {
  const server = startHostServer({ store, token, port: 0, coalesceMs: 0 })
  servers.push(server)
  return { server, token }
}

async function connectClient(server: HostServer, token: string, overrides: Record<string, unknown> = {}) {
  const client = createWebSocketClient({
    url: server.url,
    token,
    coalesceMs: 0,
    reconnectDelayMs: 0,
    ...overrides,
  })
  clients.push(client)
  await client.ready()
  return client
}

describe('WebSocket 客户端：读到的是主机的状态', () => {
  test('ready 之后 state 就是主机当前状态（线程、焦点、项目都在）', async () => {
    const { server, token } = startHost()
    const client = await connectClient(server, token)

    expect(client.status).toBe('connected')
    expect(client.state.threads.length).toBe(store.threads.length)
    expect(client.state.activeId).toBe(store.activeId)
    expect(client.state.project).toBe(store.project)
    // 读助手是本地推导的（进程内也一样）
    expect(client.state.isThreadRunning(store.activeId)).toBe(store.isThreadRunning(store.activeId))
  })

  test('命令跨进程打到主机，且结果与事件都对得上', async () => {
    const { server, token } = startHost()
    const client = await connectClient(server, token)

    const { threadId } = await client.request('thread.create', { workspace: store.project })
    // 主机侧真的建了
    expect(store.threads.some((thread) => thread.id === threadId)).toBe(true)

    // 事件到达后，客户端的复制视图里也有它
    const started = Date.now()
    while (!client.state.threads.some((thread) => thread.id === threadId)) {
      if (Date.now() - started > 3000) throw new Error('事件没把新会话带到客户端')
      await new Promise((resolve) => setTimeout(resolve, 20))
    }

    await store.deleteThread(threadId)
  })

  test('主机侧状态一变，客户端自己更新（事件驱动，不用轮询命令）', async () => {
    const { server, token } = startHost()
    const client = await connectClient(server, token)

    expect(client.state.mode).not.toBe('plan')
    store.setMode('plan')

    const started = Date.now()
    while (client.state.mode !== 'plan') {
      if (Date.now() - started > 3000) throw new Error('模式变化没通过事件到达客户端')
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
    expect(client.state.mode).toBe('plan')

    store.setMode('code')
  })

  test('UI 外壳动作走 ui.setShell，跨进程与进程内同形', async () => {
    const { server, token } = startHost()
    const client = await connectClient(server, token)

    const before = client.state.debugOpen
    client.ui.toggleDebug()

    const started = Date.now()
    while (client.state.debugOpen === before) {
      if (Date.now() - started > 3000) throw new Error('外壳开关没生效')
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
    // 主机侧也真的改了（不是客户端本地假装的）
    expect(store.debugOpen).toBe(client.state.debugOpen)

    client.ui.toggleDebug()
    // 确认框仍是**客户端本地**的：它带回调，不上线
    client.ui.showConfirm({ title: '本地确认框', message: '', onConfirm: () => {} })
    expect(client.state.confirmModal?.title).toBe('本地确认框')
    client.ui.closeConfirm()
    expect(client.state.confirmModal).toBeNull()
  })

  test('refreshState 能重新要一份快照；令牌不对就连不上（不假装连上）', async () => {
    const { server, token } = startHost()
    const client = await connectClient(server, token)

    // 主机侧直接改（不通知）：客户端要主动对齐才看得到
    const model = store.currentModel
    store.currentModel = `${model}·对齐测试`
    client.refreshState()

    const started = Date.now()
    while (client.state.currentModel !== `${model}·对齐测试`) {
      if (Date.now() - started > 3000) throw new Error('refreshState 没能重新对齐')
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
    store.currentModel = model

    // 令牌错：服务端 401，客户端如实停在 disconnected，ready() 超时失败
    const bad = createWebSocketClient({
      url: server.url,
      token: 'wrong-token',
      requestTimeoutMs: 800,
      reconnectDelayMs: 0,
    })
    clients.push(bad)
    await expect(bad.ready()).rejects.toThrow()
    expect(bad.status).toBe('disconnected')
  })
})
