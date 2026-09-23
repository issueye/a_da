import { describe, expect, test, beforeEach, afterEach } from 'bun:test'
import { mkdtempSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { SessionManager } from '../session/manager'
import { AgentStore } from '../store'
import type { AgentMessage } from '../core/types'

describe('会话压缩与持久化端到端集成测试', () => {
  let tempDir: string
  let sessionManager: SessionManager

  beforeEach(() => {
    tempDir = mkdtempSync(join(tmpdir(), 'ada_compact_test_'))
    sessionManager = new SessionManager(tempDir)
  })

  afterEach(() => {
    rmSync(tempDir, { recursive: true, force: true })
  })

  test('SessionManager 追加 compact 记录后，能够正确重构缩减后的 messages 与完整流水', async () => {
    const sessionId = 'thread_test_compact_1'
    const workspace = 'E:/projects/test_proj'

    await sessionManager.createSession(sessionId, workspace, '测试会话')

    // 追加前 2 轮消息
    await sessionManager.appendMessage(sessionId, { role: 'user', content: '第一轮提问' }, workspace)
    await sessionManager.appendMessage(sessionId, { role: 'assistant', content: '第一轮回答' }, workspace)
    await sessionManager.appendMessage(sessionId, { role: 'user', content: '第二轮提问' }, workspace)
    await sessionManager.appendMessage(sessionId, { role: 'assistant', content: '第二轮回答' }, workspace)

    // 追加压缩节点
    const summaryText = '1. Primary Request:\n测试项目完成压缩\n\n9. Next Step:\n接续执行新任务'
    await sessionManager.appendCompactEntry(
      sessionId,
      {
        id: 'compact_node_1',
        timestamp: Date.now(),
        summary: summaryText,
        preTokens: 120_000,
        postTokens: 8_000,
        savedTokens: 112_000,
        turnsSummarized: 2,
        customInstructions: '重点保留关键架构变更',
      },
      workspace,
    )

    // 追加保留的第 3 轮消息
    await sessionManager.appendMessage(sessionId, { role: 'user', content: '第 3 轮提问（保留轮次）' }, workspace)
    await sessionManager.appendMessage(sessionId, { role: 'assistant', content: '第 3 轮回答' }, workspace)

    const summaries = await sessionManager.listSessionsForWorkspace(workspace)
    expect(summaries.length).toBe(1)

    // 验证 loadSummaryMessagesSync 仅返回 continuation 消息和后续保留的消息
    const messages = sessionManager.loadSummaryMessagesSync(summaries[0]!)
    expect(messages.length).toBe(3) // 1 continuation + 2 preserved
    expect(messages[0]?.role).toBe('user')
    expect(messages[0]?.content).toContain('This session is being continued from a previous conversation')
    expect(messages[0]?.content).toContain('测试项目完成压缩')
    expect(messages[1]?.content).toBe('第 3 轮提问（保留轮次）')
    expect(messages[2]?.content).toBe('第 3 轮回答')

    // 验证 loadSummaryEntriesSync 保留了完整流水（包括 compact 节点）
    const entries = sessionManager.loadSummaryEntriesSync(summaries[0]!)
    expect(entries.length).toBe(8) // 1 header + 4 old msgs + 1 compact + 2 new msgs
    expect(entries.some((e) => e.type === 'compact')).toBe(true)
  })

  test('AgentStore 从磁盘恢复带有 compact 节点的会话时，正确生成 CompactCard 并收纳历史卡片', async () => {
    const sessionId = 'thread_test_compact_restore'
    const workspace = 'E:/projects/restore_test'

    await sessionManager.createSession(sessionId, workspace, '恢复测试会话')
    await sessionManager.appendMessage(sessionId, { role: 'user', content: '设计架构' }, workspace)
    await sessionManager.appendMessage(sessionId, { role: 'assistant', content: '架构已设计' }, workspace)

    // 记录压缩点
    await sessionManager.appendCompactEntry(
      sessionId,
      {
        id: 'compact_item_restore',
        timestamp: Date.now(),
        summary: '1. Primary Request: 设计架构',
        preTokens: 50_000,
        postTokens: 4_000,
        savedTokens: 46_000,
        turnsSummarized: 1,
      },
      workspace,
    )

    // 保留消息与后续消息
    await sessionManager.appendMessage(
      sessionId,
      {
        role: 'user',
        content: 'This session is being continued from a previous conversation that ran out of context.\n\n1. Primary Request: 设计架构',
      },
      workspace,
    )
    await sessionManager.appendMessage(sessionId, { role: 'user', content: '实现代码' }, workspace)
    await sessionManager.appendMessage(sessionId, { role: 'assistant', content: '代码已完成' }, workspace)

    // 在 store 中执行 threadFrom
    const store = new AgentStore()
    const summary = (await sessionManager.listSessionsForWorkspace(workspace))[0]!

    // @ts-expect-error 调用私有方法测试恢复映射
    const thread = store['threadFrom'](summary)

    expect(thread.items.length).toBe(3) // 1 compact item + 1 user item + 1 assistant item
    expect(thread.items[0]?.kind).toBe('compact')

    const compactItem = thread.items[0] as any
    expect(compactItem.summary).toBe('1. Primary Request: 设计架构')
    expect(compactItem.savedTokens).toBe(46_000)
    // 压缩前的 2 个 item（设计架构提问与架构已设计回答）被归档在 prunedItems 中
    expect(compactItem.prunedItems?.length).toBe(2)
    expect(compactItem.prunedItems[0].text).toBe('设计架构')

    // 紧随 compact 之后的 continuation 消息不以普通 user 气泡重复显示
    expect(thread.items[1]?.kind).toBe('user')
    expect((thread.items[1] as any).text).toBe('实现代码')
    expect(thread.items[2]?.kind).toBe('assistant')
    expect((thread.items[2] as any).text).toBe('代码已完成')
  })

  test('AgentStore compactThread 短历史（少于2轮）时给出友好提示并安全退出', async () => {
    const store = new AgentStore()
    const activeThread = store.active
    activeThread.messages.push({ role: 'user', content: '第一轮' })
    activeThread.items.push({ kind: 'user', id: 'u1', at: 1, text: '第一轮' })
    activeThread.messages.push({ role: 'assistant', content: '回复' })
    activeThread.items.push({ kind: 'assistant', id: 'a1', at: 2, text: '回复' })

    const result = await store.compactThread(activeThread.id)
    expect(result.success).toBe(false)
    expect(result.reason).toContain('较短')
    expect(activeThread.items.some((it) => it.kind === 'notice' && it.text.includes('较短'))).toBe(true)
  })
})
