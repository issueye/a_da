/**
 * 主机自举的**真子进程**验证（M3-4 / 协议 §1.8 落地前要实测的几件事）。
 *
 * 这里不做任何打桩：真 spawn 一个进程（开发态就是 `bun app.tsx --host ...`）、
 * 真读它的 stdout 就绪行、真连它的 WebSocket、真把它杀掉看有没有残留。
 *
 * 验的是四件事里的三件（第四件"编译后的单文件 exe"要打包后才能验，见 `docs/`）：
 * 1. **argv 与就绪行**：子进程收得到参数、UI 读得到真实端口；
 * 2. **能连能用**：拿到端口后 WebSocket 客户端能完成握手与快照；
 * 3. **不残留**：`stop()` 之后进程真的没了；父进程消失时主机自己也会走（看门狗）。
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { spawnHostProcess, makeHostToken, resolveDefaultHostRunner, type HostProcess } from './host-bootstrap'
import { createWebSocketClient } from './ws'

const hosts: HostProcess[] = []
const extraChildren: Array<ReturnType<typeof spawn>> = []

afterEach(async () => {
  for (const host of hosts.splice(0)) host.stop()
  for (const child of extraChildren.splice(0)) child.kill()
  // 给内核一点时间回收句柄，避免下一个用例端口/进程计数不准
  await new Promise((resolve) => setTimeout(resolve, 200))
})

const repoRoot = join(import.meta.dir, '..', '..', '..')

/** 开发态起主机：用当前 bun 跑 app.tsx（测的就是真实入口的分流）。 */
function spawnDevHost(timeoutMs = 15_000): Promise<HostProcess> {
  return spawnHostProcess({
    execPath: process.execPath,
    compiled: false,
    entryScript: join(repoRoot, 'app.tsx'),
    timeoutMs,
  })
}

describe('主机自举：真子进程', () => {
  test(
    'spawn 自己 --host，读到就绪行拿到真实端口，并能连上要快照',
    async () => {
      const host = await spawnDevHost()
      hosts.push(host)

      expect(host.port).toBeGreaterThan(0)
      expect(host.pid).toBeGreaterThan(0)
      expect(host.token.length).toBeGreaterThan(16)
      expect(existsSync(join(repoRoot, 'app.tsx'))).toBe(true)

      const client = createWebSocketClient({
        url: host.url,
        token: host.token,
        coalesceMs: 0,
        reconnectDelayMs: 0,
      })
      await client.ready()
      // 主机是真的在跑 agent：快照里有会话
      expect(client.state.threads.length).toBeGreaterThan(0)
      expect(client.state.activeId.length).toBeGreaterThan(0)

      // 命令也通（跨进程、跨"自己 spawn 自己"这条路径）
      const catalog = await client.request('plugin.builtinCatalog', {})
      expect(Array.isArray(catalog)).toBe(true)
      expect(catalog.length).toBeGreaterThan(0)

      client.close()
    },
    30_000
  )

  test(
    'stop() 之后主机不再存活（不残留）',
    async () => {
      const host = await spawnDevHost()
      expect(host.alive).toBe(true)

      host.stop()
      const started = Date.now()
      while (host.alive) {
        if (Date.now() - started > 8000) throw new Error('stop() 之后主机还活着')
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
      expect(host.alive).toBe(false)
    },
    30_000
  )

  test(
    '父进程消失时主机自己退出（看门狗，防孤儿）',
    async () => {
      // 直接起主机，把一个**不存在的** pid 当父进程：看门狗第一次检查就该让它自杀
      const token = makeHostToken()
      const child = spawn(
        process.execPath,
        [join(repoRoot, 'app.tsx'), '--host', '--port', '0', '--token', token, '--parent-pid', '999999'],
        { stdio: ['ignore', 'pipe', 'pipe'] }
      )
      extraChildren.push(child)

      // 先确认它真的起来了（读到就绪行）
      const stdout = child.stdout!
      stdout.setEncoding('utf8')
      await new Promise<void>((resolve, reject) => {
        let buffer = ''
        const timer = setTimeout(() => reject(new Error('主机没报就绪')), 15_000)
        stdout.on('data', (chunk: string) => {
          buffer += chunk
          if (buffer.includes('A_DA_HOST_READY')) {
            clearTimeout(timer)
            resolve()
          }
        })
        child.on('exit', () => {
          clearTimeout(timer)
          reject(new Error('主机在报就绪前就退了'))
        })
      })

      // 看门狗默认 2s 一次；给它足够时间自己走
      const exitCode = await new Promise<number | null>((resolve) => {
        child.on('exit', (code) => resolve(code))
        setTimeout(() => resolve(-1), 8000)
      })
      expect(exitCode).not.toBe(-1)
      extraChildren.splice(extraChildren.indexOf(child), 1)
    },
    40_000
  )

  test(
    '默认自举优先切流到原生 Rust 核心 (agent_core.exe)',
    async () => {
      const runner = resolveDefaultHostRunner()
      expect(runner.compiled).toBe(true)
      expect(runner.execPath).toContain('agent_core')
      expect(existsSync(runner.execPath)).toBe(true)

      // 拉起该原生 Rust 进程并验证就绪握手与通信
      const host = await spawnHostProcess({ timeoutMs: 15_000 })
      hosts.push(host)

      expect(host.port).toBeGreaterThan(0)
      expect(host.pid).toBeGreaterThan(0)

      const client = createWebSocketClient({
        url: host.url,
        token: host.token,
        coalesceMs: 0,
        reconnectDelayMs: 0,
      })
      await client.ready()
      expect(client.state.threads.length).toBeGreaterThan(0)
      expect(client.state.activeId.length).toBeGreaterThan(0)

      // 验证 subagentProfile.list 通信
      const subagents = await client.request('subagentProfile.list', {})
      expect(Array.isArray(subagents)).toBe(true)
      expect(subagents.length).toBeGreaterThanOrEqual(4)

      client.close()
    },
    30_000
  )
})
