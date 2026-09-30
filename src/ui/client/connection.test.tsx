/**
 * 连接状态的可理解性（B1 / 协议 §1.5）。
 *
 * 两条：
 * 1. **传输层**：真起主机、真连、真把主机停掉——客户端的 `state.connection` 必须变成
 *    `disconnected` 且带上原因与重连次数；主机回来之后能重连上并恢复 `connected`。
 * 2. **界面层**：断线时 `AgentWindow` 顶部出现一条横幅（说人话），连上时它不占地方。
 */

import { afterEach, describe, expect, test } from 'bun:test'
import React from 'react'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { connectTest } from '@gpuix/react/automation'
import { store } from '../../agent/store'
import { startHostServer, type HostServer } from '../../agent/host/server'
import { ConnectionBanner } from '../ConnectionBanner'
import { createWebSocketClient, type WebSocketClient } from './ws'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const servers: HostServer[] = []
const clients: WebSocketClient[] = []

afterEach(() => {
  for (const client of clients.splice(0)) client.close()
  for (const server of servers.splice(0)) server.stop()
})

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

/** 等到条件成立（有界，绝不无限等）。 */
async function until(check: () => boolean, what: string, timeoutMs = 8000): Promise<void> {
  const started = Date.now()
  while (Date.now() - started < timeoutMs) {
    if (check()) return
    await sleep(30)
  }
  throw new Error(`等不到：${what}`)
}

describe('连接状态：传输层', () => {
  test('主机停掉 → disconnected（带原因与重连次数）；主机回来 → 恢复 connected', async () => {
    const token = 'banner-token'
    const first = startHostServer({ store, token, port: 0, coalesceMs: 0 })
    servers.push(first)

    const client = createWebSocketClient({
      url: first.url,
      token,
      coalesceMs: 0,
      reconnectDelayMs: 50, // 测试里把退避调短
    })
    clients.push(client)
    await client.ready()
    expect(client.state.connection.status).toBe('connected')

    // 主机被杀（就是"进程没了"这件事在本机的等价物）
    first.stop()
    await until(() => client.state.connection.status === 'disconnected', '客户端发现断开')
    expect(client.state.connection.attempts).toBeGreaterThan(0)
    expect(client.state.connection.reason ?? '').not.toBe('')

    // 主机回来（同一端口 + 同一令牌 = 同一次自举的重启）
    const second = startHostServer({ store, token, port: first.port, coalesceMs: 0 })
    servers.push(second)
    await until(() => client.state.connection.status === 'connected', '重连成功', 12_000)
    // 重连成功后次数归零，且快照重新对齐过（会话仍在）
    expect(client.state.connection.attempts).toBe(0)
    expect(client.state.threads.length).toBeGreaterThan(0)
  }, 30_000)
})

describeNative('连接状态：界面', () => {
  test('断线时画出横幅，连上时不占地方', async () => {
    const { render, renderer } = createTestRoot({ width: 900, height: 600 })

    // 用一个最小的假 client：这里验的是"状态怎么被画出来"，不是传输
    const fake = {
      state: {
        connection: { status: 'disconnected', attempts: 3, reason: '主机侧关闭（code=1001）' },
      },
    } as unknown as Parameters<typeof ConnectionBanner>[0]['client']

    render(<ConnectionBanner client={fake} />)
    const app = await connectTest(renderer)
    await app.getByTestId('connection-banner').waitFor({ timeoutMs: 5000 })
    const text = renderer.getPaintedText().join('\n')
    expect(text).toContain('与主机的连接已断开')
    expect(text).toContain('第 3 次')
    await app.close()

    // 连上之后：组件不渲染任何东西
    const connected = createTestRoot({ width: 900, height: 600 })
    const fakeConnected = {
      state: { connection: { status: 'connected', attempts: 0 } },
    } as unknown as Parameters<typeof ConnectionBanner>[0]['client']
    connected.render(<ConnectionBanner client={fakeConnected} />)
    const app2 = await connectTest(connected.renderer)
    expect(await app2.getByTestId('connection-banner').count()).toBe(0)
    await app2.close()
  }, 20_000)
})
