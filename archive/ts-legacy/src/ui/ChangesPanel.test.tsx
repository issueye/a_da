/**
 * 改动审阅面板与回滚链路的 UI 测试。
 *
 * 关键点：回滚不是装饰——测试里真的写文件、真的点「恢复原状」，
 * 然后断言盘上的文件被还原、卡片被标记为已撤销。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import React from 'react'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { connectTest } from '@gpuix/react/automation'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { Item } from '../agent/types'
import { store } from '../agent/store'
import { defaultCheckpointManager } from '../agent/checkpoint'
import { ChangesPanel } from './ChangesPanel'
import { agentClient } from './client'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

let ws = ''
let savedActiveId = ''
let savedItems: Item[] = []
const threadId = 'test-changes-thread-ui'

beforeAll(async () => {
  ws = await mkdtemp(join(tmpdir(), 'a-da-chg-ui-'))
  await mkdir(join(ws, 'src'), { recursive: true })
  savedActiveId = store.activeId
  savedItems = store.active.items

  const thread = {
    id: threadId,
    title: '改动审阅测试',
    createdAt: Date.now(),
    workspace: ws,
    items: [] as Item[],
    messages: [],
    mode: 'code' as const,
  }
  store.threads = [thread, ...store.threads.filter((t) => t.id !== threadId)]
  store.activeId = threadId
})

afterAll(async () => {
  store.threads = store.threads.filter((t) => t.id !== threadId)
  store.activeId = savedActiveId
  const restored = store.threads.find((t) => t.id === savedActiveId)
  if (restored) restored.items = savedItems
  store.setChangesOpen(false)
  await defaultCheckpointManager.discard(threadId)
  if (ws) await rm(ws, { recursive: true, force: true })
})

describe('getThreadFileChanges 按文件聚合', () => {
  test('同一文件的多次编辑合并，reverted 汇总取与', () => {
    const thread = store.threads.find((t) => t.id === threadId)!
    thread.items = [
      {
        kind: 'tool',
        id: 'w1',
        at: 1,
        callId: 'c1',
        name: 'write_file',
        args: { path: 'src\\a.ts' },
        rawArgs: '',
        status: 'done',
        patch: '+one\n+two\n',
        checkpointId: 'ckpt_1',
      },
      {
        kind: 'tool',
        id: 'w2',
        at: 2,
        callId: 'c2',
        name: 'edit_file',
        args: { path: 'src/a.ts' },
        rawArgs: '',
        status: 'done',
        patch: '+three\n-old\n',
        checkpointId: 'ckpt_2',
      },
      {
        kind: 'tool',
        id: 'w3',
        at: 3,
        callId: 'c3',
        name: 'write_file',
        args: { path: 'b.ts' },
        rawArgs: '',
        status: 'done',
        patch: '+file-b\n',
        checkpointId: 'ckpt_3',
        reverted: true,
      },
      // 读文件不该被算进改动
      {
        kind: 'tool',
        id: 'r1',
        at: 4,
        callId: 'c4',
        name: 'read_file',
        args: { path: 'src/a.ts' },
        rawArgs: '',
        status: 'done',
      },
    ]

    const changes = store.getThreadFileChanges(threadId)
    expect(changes).toHaveLength(2)

    const a = changes.find((c) => c.path === 'src/a.ts')!
    expect(a).toBeDefined()
    expect(a.editsCount).toBe(2)
    expect(a.additions).toBe(3)
    expect(a.deletions).toBe(1)
    expect(a.reverted).toBe(false)
    expect(a.cardIds).toEqual(['w1', 'w2'])

    const b = changes.find((c) => c.path === 'b.ts')!
    expect(b.reverted).toBe(true)
    expect(store.getThreadChangeCount(threadId)).toBe(1)
  })
})

describeNative('ChangesPanel 交互', () => {
  test('逐文件恢复原状：文件真的被还原，卡片标记已撤销', async () => {
    const thread = store.threads.find((t) => t.id === threadId)!
    const target = join(ws, 'solo.ts')
    await writeFile(target, 'A0\n')

    // 模拟 Agent 动手前的检查点，然后「改写」文件
    const record = await defaultCheckpointManager.capture(threadId, 'call-solo', [
      { path: 'solo.ts', absolute: target },
    ])
    await writeFile(target, 'A1\n')

    thread.items = [
      {
        kind: 'tool',
        id: 'w-solo',
        at: 1,
        callId: 'call-solo',
        name: 'write_file',
        args: { path: 'solo.ts' },
        rawArgs: '',
        status: 'done',
        patch: '+A1\n',
        checkpointId: record.id,
      },
    ]

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ position: 'relative', width: 800, height: 600 }}>
        <ChangesPanel client={agentClient} />
      </div>,
    )
    const app = await connectTest(renderer)
    const screen = () => renderer.getPaintedText().join('\n')
    const painted = async (needle: string, timeoutMs = 10_000): Promise<void> => {
      const started = Date.now()
      while (Date.now() - started < timeoutMs) {
        if (screen().includes(needle)) return
        renderer.flush?.()
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
      throw new Error(`never painted ${needle}\n${screen()}`)
    }

    await painted('文件改动')
    expect(screen()).toContain('solo.ts')

    await app.getByTestId('change-revert-solo.ts').click()
    await painted('已撤销')

    expect(await readFile(target, 'utf-8')).toBe('A0\n')
    expect((thread.items[0] as Extract<Item, { kind: 'tool' }>).reverted).toBe(true)
    await app.close()
  }, 30_000)

  test('一键全部恢复：新建的文件被删除，改动卡片全部作废', async () => {
    const thread = store.threads.find((t) => t.id === threadId)!
    const existing = join(ws, 'keep.ts')
    await writeFile(existing, 'K0\n')
    const created = join(ws, 'new.ts')

    const ck1 = await defaultCheckpointManager.capture(threadId, 'call-keep', [
      { path: 'keep.ts', absolute: existing },
    ])
    const ck2 = await defaultCheckpointManager.capture(threadId, 'call-new', [
      { path: 'new.ts', absolute: created },
    ])

    // Agent 的两次改动：改写 keep.ts、新建 new.ts
    await writeFile(existing, 'K1\n')
    await writeFile(created, 'N1\n')
    thread.items = [
      {
        kind: 'tool',
        id: 'w-keep',
        at: 1,
        callId: 'call-keep',
        name: 'edit_file',
        args: { path: 'keep.ts' },
        rawArgs: '',
        status: 'done',
        patch: '+K1\n-K0\n',
        checkpointId: ck1.id,
      },
      {
        kind: 'tool',
        id: 'w-new',
        at: 2,
        callId: 'call-new',
        name: 'write_file',
        args: { path: 'new.ts' },
        rawArgs: '',
        status: 'done',
        patch: '+N1\n',
        checkpointId: ck2.id,
      },
    ]

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ position: 'relative', width: 800, height: 600 }}>
        <ChangesPanel client={agentClient} />
      </div>,
    )
    const app = await connectTest(renderer)
    const screen = () => renderer.getPaintedText().join('\n')
    const painted = async (needle: string, timeoutMs = 10_000): Promise<void> => {
      const started = Date.now()
      while (Date.now() - started < timeoutMs) {
        if (screen().includes(needle)) return
        renderer.flush?.()
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
      throw new Error(`never painted ${needle}\n${screen()}`)
    }

    await painted('文件改动')
    await app.getByTestId('revert-all-changes').click()
    // 等回滚落盘、两行都翻成已撤销
    await painted('已撤销')

    expect(await readFile(existing, 'utf-8')).toBe('K0\n')
    expect(existsSync(created)).toBe(false)
    const cards = thread.items as Array<Extract<Item, { kind: 'tool' }>>
    expect(cards[0]!.reverted).toBe(true)
    expect(cards[1]!.reverted).toBe(true)
    await app.close()
  }, 30_000)
})
