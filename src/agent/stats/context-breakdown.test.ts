import { describe, expect, test } from 'bun:test'
import { computeContextBreakdown } from './context-breakdown'
import type { Item } from '../types'

describe('上下文分解与统计分析引擎', () => {
  test('根据真实返回的 usage 与消息构成，准确切分各组成部分与缓存命中率', () => {
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 1000, text: '请帮我实现一个高效的二叉树' },
      {
        kind: 'tool',
        id: 't1',
        at: 1010,
        callId: 'c1',
        name: 'read_file',
        args: {},
        rawArgs: '',
        status: 'done',
        output: 'struct Node { value: i32, left: Option<Box<Node>>, right: Option<Box<Node>> }',
      },
      {
        kind: 'assistant',
        id: 'a1',
        at: 1020,
        text: '这是基于 Rust 实现的高效二叉树，已经加上了所有权与生命周期管理。',
      },
    ]

    const result = computeContextBreakdown({
      items,
      systemPrompt: '你是全能代码编程助手，遵守 Rust 最佳安全实践。',
      skillsPrompt: '- **code-review**: 审查代码内存安全',
      toolSpecsChars: 200,
      realPromptTokens: 2485,
      realCompletionTokens: 198,
      realCachedTokens: 2304,
      contextLimit: 128_000,
    })

    // 验证总体统计
    expect(result.usedTokens).toBe(2485 + 198)
    expect(result.maxTokens).toBe(128_000)
    expect(result.percent).toBeCloseTo((2485 + 198) / 128_000, 4)
    expect(result.formattedSummary).toContain('2.7k/128k')

    // 验证缓存命中率 (2304 / 2485 ≈ 92.7%)
    expect(result.cacheHitRate).toBeDefined()
    expect(result.cacheHitRate!).toBeGreaterThan(0.9)
    expect(result.cachedTokens).toBe(2304)

    // 验证包含会话历史、系统提示词、技能、工具与本次回复的拆分项
    const sources = result.breakdown.map((b) => b.source)
    expect(sources).toContain('messages')
    expect(sources).toContain('system_prompt')
    expect(sources).toContain('skills')
    expect(sources).toContain('tools')
    expect(sources).toContain('completion')

    // 验证排序：从大到小
    for (let i = 1; i < result.breakdown.length; i++) {
      expect(result.breakdown[i - 1].chars).toBeGreaterThanOrEqual(result.breakdown[i].chars)
    }
  })

  test('在尚未返回 realPromptTokens 时能够稳健退回字符估算模式', () => {
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 1000, text: '你好' },
    ]

    const result = computeContextBreakdown({
      items,
      contextLimit: 64_000,
    })

    expect(result.usedTokens).toBeGreaterThan(0)
    expect(result.maxTokens).toBe(64_000)
    expect(result.cacheHitRate).toBeNull()
    expect(result.cachedTokens).toBe(0)
  })

  test('准确呈现正常对话消息、系统提示词与工具的 Token 明细并保证无缝汇总', () => {
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 1000, text: '请查看当前项目的 package.json 并执行测试' },
      {
        kind: 'tool',
        id: 't1',
        at: 1010,
        callId: 'c1',
        name: 'read_file',
        args: { path: 'package.json' },
        rawArgs: '{"path":"package.json"}',
        status: 'done',
        output: '{\n  "name": "a_da",\n  "version": "1.0.0"\n}',
      },
      {
        kind: 'assistant',
        id: 'a1',
        at: 1020,
        text: '已成功查看 package.json，项目已准备就绪。',
      },
    ]

    const result = computeContextBreakdown({
      items,
      systemChars: 4000,
      toolSpecsChars: 3000,
      realPromptTokens: 6821,
      realCompletionTokens: 213,
      contextLimit: 1_000_000,
    })

    const msgItem = result.breakdown.find((b) => b.source === 'messages')
    const sysItem = result.breakdown.find((b) => b.source === 'system_prompt')
    const toolItem = result.breakdown.find((b) => b.source === 'tools')
    const compItem = result.breakdown.find((b) => b.source === 'completion')

    expect(msgItem).toBeDefined()
    expect(msgItem!.label).toBe('正常对话消息')
    expect(msgItem!.estimatedTokens).toBeGreaterThan(0)

    expect(sysItem).toBeDefined()
    expect(sysItem!.label).toBe('系统提示词')
    expect(sysItem!.estimatedTokens).toBeGreaterThan(0)

    expect(toolItem).toBeDefined()
    expect(toolItem!.label).toBe('工具')
    expect(toolItem!.estimatedTokens).toBeGreaterThan(0)

    expect(compItem).toBeDefined()
    expect(compItem!.label).toBe('本次回复')
    expect(compItem!.estimatedTokens).toBe(213)

    // 输入 Prompt 的三个细分 Token 之和必须严格等于真实 PromptTokens (6821)
    const promptSum = msgItem!.estimatedTokens + sysItem!.estimatedTokens + toolItem!.estimatedTokens
    expect(promptSum).toBe(6821)
  })
})
