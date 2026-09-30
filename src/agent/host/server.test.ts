/**
 * 主机 WebSocket 服务端的端到端测试（M3-1/M3-2）。
 *
 * **真起服务端、真连 WebSocket**（本机回环），不是打桩：这是"拆分成为运行期事实"的第一处证据。
 * 用真 store 单例（`A_DA_HOME` 由 `scripts/test-preload.ts` 指到临时目录）。
 *
 * 验四件事：
 * 1. **令牌**：没带令牌的连接拿不到 401；带上才升级成 WebSocket；
 * 2. **连上就有快照**（协议 §1.3）：`evt.state.snapshot` 先到，`seq` 从 0 起；
 * 3. **命令跨进程边界可用**：`session.hello` / `session.snapshot` / `thread.create` 都通；
 * 4. **事件会推送**：store 一变，已连接的客户端收到新快照（`seq` 递增）。
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { store } from '../store'
import { PROTOCOL_VERSION } from '../../shared/protocol'
import { startHostServer, type HostServer } from './server'

const servers: HostServer[] = []
const sockets: WebSocket[] = []

afterEach(() => {
  for (const socket of sockets.splice(0)) {
    try {
      socket.close()
    } catch {
      // 已经关了
    }
  }
  for (const server of servers.splice(0)) server.stop()
})

/** 起一个服务端并连一个客户端；返回收发用的小工具。 */
async function connect(options: { token?: string; connectToken?: string } = {}) {
  const token = options.token ?? 'test-token'
  const server = startHostServer({ store, token, port: 0, coalesceMs: 0 })
  servers.push(server)

  const socket = new WebSocket(`${server.url}/?token=${options.connectToken ?? token}`)
  sockets.push(socket)

  const inbox: Array<Record<string, unknown>> = []
  const waiters: Array<() => void> = []
  socket.addEventListener('message', (event) => {
    inbox.push(JSON.parse(String((event as MessageEvent).data)) as Record<string, unknown>)
    for (const wake of waiters.splice(0)) wake()
  })

  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve())
    socket.addEventListener('error', () => reject(new Error('WebSocket 连接失败')))
  })

  /** 等到 inbox 里出现满足条件的帧。 */
  const waitFor = async (
    predicate: (frame: Record<string, unknown>) => boolean,
    timeoutMs = 5000
  ): Promise<Record<string, unknown>> => {
    const started = Date.now()
    for (;;) {
      const hit = inbox.find(predicate)
      if (hit) return hit
      if (Date.now() - started > timeoutMs) {
        throw new Error(`等不到符合条件的帧；已收到 ${JSON.stringify(inbox)}`)
      }
      await new Promise<void>((resolve) => {
        waiters.push(resolve)
        setTimeout(resolve, 50)
      })
    }
  }

  /** 发一条命令并等它的响应。 */
  const call = async (id: number, method: string, params: unknown = {}): Promise<Record<string, unknown>> => {
    socket.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }))
    return waitFor((frame) => frame.id === id)
  }

  return { server, socket, inbox, waitFor, call }
}

describe('主机 WebSocket 服务端', () => {
  test('没带令牌的连接被挡在 401，带令牌才升级', async () => {
    const server = startHostServer({ store, token: 'right-token', port: 0 })
    servers.push(server)

    // 令牌校验发生在升级之前，用普通 HTTP 请求就能验到（`server.url` 是 ws://，探测要用 http://）
    const httpUrl = server.url.replace(/^ws/, 'http')
    const denied = await fetch(`${httpUrl}/?token=wrong-token`)
    expect(denied.status).toBe(401)
    await denied.text()

    const noToken = await fetch(httpUrl)
    expect(noToken.status).toBe(401)
    await noToken.text()
  })

  test('连上先收到一份快照，seq 从 0 起', async () => {
    const { waitFor } = await connect()

    const seed = await waitFor((frame) => frame.method === 'evt.state.snapshot')
    const params = seed.params as { seq: number; payload: { threads: unknown[] } }
    expect(params.seq).toBe(0)
    expect(Array.isArray(params.payload.threads)).toBe(true)
    expect(params.payload.threads.length).toBeGreaterThan(0)
  })

  test('握手与快照命令跨连接可用；未知方法如实报错', async () => {
    const { call } = await connect()

    const hello = await call(1, 'session.hello', {
      token: 'test-token',
      protocolVersion: PROTOCOL_VERSION,
    })
    const helloResult = hello.result as { protocolVersion: string; host: { pid: number } }
    expect(helloResult.protocolVersion).toBe(PROTOCOL_VERSION)
    expect(helloResult.host.pid).toBe(process.pid)

    const snapshot = await call(2, 'session.snapshot')
    expect((snapshot.result as { threads: unknown[] }).threads.length).toBeGreaterThan(0)

    const unknown = await call(3, 'nope.not-a-method')
    const error = unknown.error as { code: number; message: string }
    expect(error.code).toBe(-32003) // AppErrorCode.NotReady：明确"没实现"，不是静默 no-op
    expect(error.message).toContain('nope.not-a-method')

    const badVersion = await call(4, 'session.hello', {
      token: 'test-token',
      protocolVersion: '9.9',
    })
    expect((badVersion.error as { code: number }).code).toBe(-32000) // ProtocolVersionMismatch
  })

  test('命令真的落到主机：新建会话后两个客户端都能看到', async () => {
    const first = await connect()
    const second = await connect()

    const created = await first.call(10, 'thread.create', { workspace: store.project })
    const threadId = (created.result as { threadId: string }).threadId

    // 主机侧真的建了
    expect(store.threads.some((thread) => thread.id === threadId)).toBe(true)

    // 另一个客户端从**事件**里也看到了（不是各自去问主机）
    const seen = await second.waitFor((frame) => {
      if (frame.method !== 'evt.state.snapshot') return false
      const payload = (frame.params as { payload: { threads: Array<{ id: string }> } }).payload
      return payload.threads.some((thread) => thread.id === threadId)
    })
    expect(seen).toBeTruthy()

    await store.deleteThread(threadId)
  })

  test('store 一变就广播，seq 单调递增', async () => {
    const { waitFor } = await connect()

    await waitFor((frame) => frame.method === 'evt.state.snapshot')
    store.trace('服务端广播测试')

    const next = await waitFor((frame) => {
      if (frame.method !== 'evt.state.snapshot') return false
      return (frame.params as { seq: number }).seq > 0
    })
    expect((next.params as { seq: number }).seq).toBeGreaterThan(0)
  })
})
