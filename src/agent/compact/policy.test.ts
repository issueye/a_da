import { describe, expect, test } from 'bun:test'
import type { AgentMessage } from '../core/types'
import {
  DEFAULT_COMPACT_CONTEXT_WINDOW,
  estimateMessageTokens,
  getAutoCompactThreshold,
  getEffectiveContextWindowSize,
  getModelContextWindow,
  shouldAutoCompact,
} from './policy'

describe('上下文预算与压缩策略判定引擎', () => {
  test('getModelContextWindow 正确识别主流模型上下文上限并支持用户自定义覆盖', () => {
    expect(getModelContextWindow('gemini-2.0-flash')).toBe(1_000_000)
    expect(getModelContextWindow('qwen-long')).toBe(1_000_000)
    expect(getModelContextWindow('claude-3-5-sonnet-20241022')).toBe(200_000)
    expect(getModelContextWindow('deepseek-chat')).toBe(128_000)
    expect(getModelContextWindow('moonshot-v1-32k')).toBe(32_000)
    expect(getModelContextWindow(undefined)).toBe(128_000)

    // 用户显式配置覆盖
    expect(getModelContextWindow('deepseek-chat', 64_000)).toBe(64_000)
  })

  test('getEffectiveContextWindowSize 与 getAutoCompactThreshold 计算预留与安全阈值', () => {
    // 默认 128,000 上限，预留 20,000 输出，有效窗口 108,000
    const effective = getEffectiveContextWindowSize({ contextWindow: 128_000, outputReserveTokens: 20_000 })
    expect(effective).toBe(108_000)

    // 自动压缩阈值：有效窗口减去安全缓冲区 15,000 或 80%
    const threshold = getAutoCompactThreshold({
      contextWindow: 128_000,
      outputReserveTokens: 20_000,
      bufferTokens: 15_000,
      thresholdPercent: 80,
    })
    // bufferThreshold = 108000 - 15000 = 93000
    // percentThreshold = 128000 * 0.8 = 102400
    // 取最小值 Math.min(93000, 102400) = 93000
    expect(threshold).toBe(93_000)
  })

  test('estimateMessageTokens 准确估算多轮消息中的字符与 Token 对应关系', () => {
    const messages: AgentMessage[] = [
      { role: 'user', content: '请帮我写一个快速排序算法' },
      {
        role: 'assistant',
        content: '好的，这是快速排序的代码实现...',
        thinking: '用户需要排序算法，我将提供标准 Rust 实现',
        toolCalls: [{ id: 'call_1', name: 'write_file', arguments: {}, rawArguments: '{"path":"quick.rs"}' }],
      },
      {
        role: 'toolResult',
        toolCallId: 'call_1',
        toolName: 'write_file',
        content: '文件已保存',
      },
    ]

    const tokens = estimateMessageTokens(messages)
    expect(tokens).toBeGreaterThan(20)
    expect(tokens).toBeLessThan(100)
  })

  test('shouldAutoCompact 策略判定逻辑', () => {
    const shortMessages: AgentMessage[] = [
      { role: 'user', content: '第一条消息' },
      { role: 'assistant', content: '回复' },
    ]

    // 历史过短不触发
    const decisionShort = shouldAutoCompact({
      messages: shortMessages,
      currentTokens: 999_999,
      config: { contextWindow: 128_000 },
    })
    expect(decisionShort.shouldCompact).toBe(false)
    expect(decisionShort.reason).toBe('not_enough_messages')

    // 构造足够轮次的消息
    const longMessages: AgentMessage[] = [
      { role: 'user', content: '第一轮' },
      { role: 'assistant', content: '第一轮回复' },
      { role: 'user', content: '第二轮' },
      { role: 'assistant', content: '第二轮回复' },
    ]

    // Token 较低不触发
    const decisionBelow = shouldAutoCompact({
      messages: longMessages,
      currentTokens: 10_000,
      config: { contextWindow: 128_000 },
    })
    expect(decisionBelow.shouldCompact).toBe(false)
    expect(decisionBelow.reason).toBe('below_threshold')

    // Token 超过阈值触发自动压缩
    const decisionAbove = shouldAutoCompact({
      messages: longMessages,
      currentTokens: 95_000,
      config: { contextWindow: 128_000 },
    })
    expect(decisionAbove.shouldCompact).toBe(true)
    expect(decisionAbove.reason).toBe('above_threshold')

    // 策略显式关闭时不触发
    const decisionDisabled = shouldAutoCompact({
      messages: longMessages,
      currentTokens: 99_000,
      config: { enabled: false },
    })
    expect(decisionDisabled.shouldCompact).toBe(false)
    expect(decisionDisabled.reason).toBe('disabled')
  })
})
