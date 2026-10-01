/**
 * Rust 原生后端与 UI 自举连接的真子进程守门测试。
 *
 * 验证 UI 角色通过 spawnHostProcess 直接拉起 agent_core.exe，
 * 读就绪行、连接 WebSocket、鉴权握手并同步快照。
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { spawnHostProcess, type HostProcess } from './host-bootstrap'
import { createWebSocketClient } from './ws'

const hosts: HostProcess[] = []

afterEach(async () => {
  for (const host of hosts.splice(0)) host.stop()
  await new Promise((resolve) => setTimeout(resolve, 200))
})

const repoRoot = join(import.meta.dir, '..', '..', '..')
const rustExe = join(repoRoot, 'agent_core', 'target', 'debug', 'agent_core.exe')

describe('Rust 原生后端自举联调', () => {
  test('spawn agent_core.exe，读到就绪行并完成快照与 RPC 交互', async () => {
    if (!existsSync(rustExe)) {
      console.warn('[跳过] agent_core.exe 未编译，请先运行 cargo build')
      return
    }

    const host = await spawnHostProcess({
      execPath: rustExe,
      compiled: true,
      timeoutMs: 10_000,
    })
    hosts.push(host)

    expect(host.port).toBeGreaterThan(0)
    expect(host.pid).toBeGreaterThan(0)
    expect(host.token.length).toBeGreaterThan(16)

    const client = createWebSocketClient({
      url: host.url,
      token: host.token,
      coalesceMs: 0,
      reconnectDelayMs: 0,
    })

    await client.ready()

    // 验证状态快照
    expect(client.state.threads.length).toBeGreaterThan(0)
    expect(client.state.activeId.length).toBeGreaterThan(0)

    // 验证 RPC 请求分发 (fs.roots)
    const roots = await client.request('fs.roots', {})
    expect(Array.isArray(roots)).toBe(true)
    expect(roots.length).toBeGreaterThan(0)

    client.close()
  }, 20_000)
})
