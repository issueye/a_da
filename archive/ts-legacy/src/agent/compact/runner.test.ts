import { describe, expect, test } from 'bun:test'
import type { AgentMessage } from '../core/types'
import type { Item } from '../types'
import { selectCompactSelection } from './runner'

describe('会话压缩切分与选择引擎', () => {
  test('单轮对话历史无法切分', () => {
    const messages: AgentMessage[] = [
      { role: 'user', content: '第一条提问' },
      { role: 'assistant', content: '第一条回复' },
    ]
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 1, text: '第一条提问' },
      { kind: 'assistant', id: 'a1', at: 2, text: '第一条回复' },
    ]

    const selection = selectCompactSelection(messages, items)
    expect(selection.messagesToSummarize.length).toBe(0)
    expect(selection.preservedMessages.length).toBe(2)
    expect(selection.prunedItems.length).toBe(0)
    expect(selection.preservedItems.length).toBe(2)
    expect(selection.turnsSummarized).toBe(0)
  })

  test('多轮对话准确将前期历史分离为待总结，并原样保留最近 1 轮完整回合', () => {
    const messages: AgentMessage[] = [
      // 第 1 轮
      { role: 'user', content: '帮我设计数据库表结构' },
      {
        role: 'assistant',
        content: '',
        toolCalls: [{ id: 'tc1', name: 'write_file', arguments: {}, rawArguments: '{}' }],
      },
      { role: 'toolResult', toolCallId: 'tc1', toolName: 'write_file', content: 'OK' },
      { role: 'assistant', content: '数据库表已设计完成' },
      // 第 2 轮
      { role: 'user', content: '增加外键约束' },
      { role: 'assistant', content: '外键约束已追加' },
      // 第 3 轮（最近活跃轮次）
      { role: 'user', content: '现在请帮我编写迁移脚本' },
      { role: 'assistant', content: '这是迁移脚本内容：...' },
    ]

    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 100, text: '帮我设计数据库表结构' },
      {
        kind: 'tool',
        id: 't1',
        at: 101,
        callId: 'tc1',
        name: 'write_file',
        args: {},
        rawArgs: '{}',
        status: 'done',
      },
      { kind: 'assistant', id: 'a1', at: 102, text: '数据库表已设计完成' },
      { kind: 'user', id: 'u2', at: 200, text: '增加外键约束' },
      { kind: 'assistant', id: 'a2', at: 201, text: '外键约束已追加' },
      { kind: 'user', id: 'u3', at: 300, text: '现在请帮我编写迁移脚本' },
      { kind: 'assistant', id: 'a3', at: 301, text: '这是迁移脚本内容：...' },
    ]

    const selection = selectCompactSelection(messages, items)

    // 应该总结前 2 轮
    expect(selection.turnsSummarized).toBe(2)
    expect(selection.messagesToSummarize.length).toBe(6) // 4 + 2
    expect(selection.messagesToSummarize[0]?.content).toBe('帮我设计数据库表结构')
    expect(selection.messagesToSummarize[5]?.content).toBe('外键约束已追加')

    // 保留最近第 3 轮
    expect(selection.preservedMessages.length).toBe(2)
    expect(selection.preservedMessages[0]?.content).toBe('现在请帮我编写迁移脚本')
    expect(selection.preservedMessages[1]?.content).toBe('这是迁移脚本内容：...')

    // 界面卡片切分
    expect(selection.prunedItems.length).toBe(5)
    expect(selection.preservedItems.length).toBe(2)
    expect(selection.preservedItems[0]?.kind).toBe('user')
    expect((selection.preservedItems[0] as any).text).toBe('现在请帮我编写迁移脚本')
  })
})
