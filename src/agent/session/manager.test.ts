import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { SessionManager } from './manager'

let tempDir = ''
let manager: SessionManager

beforeAll(async () => {
  tempDir = await mkdtemp(join(tmpdir(), 'a-da-session-test-'))
  manager = new SessionManager(tempDir)
})

afterAll(async () => {
  if (tempDir) await rm(tempDir, { recursive: true, force: true })
})

describe('SessionManager', () => {
  test('creates a session and writes header in JSONL', async () => {
    const header = await manager.createSession('s_1', 'E:/test_project', '测试会话1')
    expect(header.id).toBe('s_1')
    expect(header.workspace).toBe('E:/test_project')

    const loaded = await manager.loadSession('s_1')
    expect(loaded).not.toBeNull()
    expect(loaded?.header.id).toBe('s_1')
    expect(loaded?.header.title).toBe('测试会话1')
    expect(loaded?.messages.length).toBe(0)
  })

  test('appends messages incrementally and reconstructs history', async () => {
    await manager.appendMessage('s_1', {
      role: 'user',
      content: '你好，帮我写一段代码',
      timestamp: 1000,
    })

    await manager.appendMessage('s_1', {
      role: 'assistant',
      content: '没问题，这是代码。',
      timestamp: 2000,
    })

    const loaded = await manager.loadSession('s_1')
    expect(loaded?.messages.length).toBe(2)
    expect(loaded?.messages[0]?.role).toBe('user')
    expect(loaded?.messages[0]?.content).toBe('你好，帮我写一段代码')
    expect(loaded?.messages[1]?.role).toBe('assistant')
  })

  test('updates session title', async () => {
    await manager.updateSessionTitle('s_1', '新修改的标题')
    const loaded = await manager.loadSession('s_1')
    expect(loaded?.header.title).toBe('新修改的标题')
  })

  test('lists sessions for workspace sorted by updatedAt', async () => {
    await manager.createSession('s_2', 'E:/test_project', '第二个会话')
    await manager.createSession('s_other', 'E:/other_project', '其他项目会话')

    const list = await manager.listSessionsForWorkspace('E:/test_project')
    expect(list.length).toBe(2)
    expect(list.map((s) => s.id)).toContain('s_1')
    expect(list.map((s) => s.id)).toContain('s_2')
    expect(list.map((s) => s.id)).not.toContain('s_other')
  })
})
