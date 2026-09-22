import { describe, expect, test } from 'bun:test'
import React from 'react'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { connectTest } from '@gpuix/react/automation'
import type { Item, Thread } from '../agent/types'
import { store } from '../agent/store'
import {
  Composer,
  ComposerTelemetryBar,
  computeThreadTelemetry,
  getModelContextWindow,
} from './Composer'
import { convertMessagesToLlm, imageToDataUrl } from '../agent/core/agent-loop'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

describe('Composer 遥测与多模态配置逻辑', () => {
  test('getModelContextWindow 模型上下文窗口大小判定及用户自定义覆盖', () => {
    // 默认与按模型名自动识别
    expect(getModelContextWindow('gemini-2.0-flash')).toBe(1_000_000)
    expect(getModelContextWindow('qwen-long')).toBe(1_000_000)
    expect(getModelContextWindow('claude-3-5-sonnet-20241022')).toBe(200_000)
    expect(getModelContextWindow('moonshot-v1-32k')).toBe(32_000)
    expect(getModelContextWindow('model-64k')).toBe(64_000)
    expect(getModelContextWindow('deepseek-chat')).toBe(128_000)
    expect(getModelContextWindow('gpt-4o')).toBe(128_000)
    expect(getModelContextWindow(undefined)).toBe(128_000)

    // 用户在设置中自定义配置了上下文上限时，优先使用配置值
    expect(getModelContextWindow('deepseek-chat', 1_000_000)).toBe(1_000_000)
    expect(getModelContextWindow('custom-glm-5', 256_000)).toBe(256_000)
  })

  test('computeThreadTelemetry 正确计算轮数、步数、tok/s、缓存命中率和上下文占比', () => {
    const thread: Thread = {
      id: 'th-1',
      title: '测试遥测会话',
      createdAt: Date.now(),
      workspace: 'C:/test',
      messages: [],
      items: [
        { kind: 'user', id: 'u1', at: 1000, text: '你好，帮我写个脚本' },
        {
          kind: 'tool',
          id: 't1',
          at: 1010,
          callId: 'c1',
          name: 'write_file',
          args: {},
          rawArgs: '',
          status: 'done',
        },
        {
          kind: 'tool',
          id: 't2',
          at: 1020,
          callId: 'c2',
          name: 'run_command',
          args: {},
          rawArgs: '',
          status: 'done',
        },
        {
          kind: 'assistant',
          id: 'a1',
          at: 1030,
          text: '已经为您完成脚本编写。',
          durationMs: 2000, // 2 秒
          usage: {
            promptTokens: 81920,
            completionTokens: 362,
            totalTokens: 82282,
            cachedTokens: 77000, // 77000 / 81920 ≈ 94%
          },
        },
      ],
    }

    const telemetry = computeThreadTelemetry(thread, false, 'deepseek-chat')

    expect(telemetry.turns).toBe(1)
    expect(telemetry.steps).toBe(2)
    // 362 tokens / 2s = 181 tok/s
    expect(telemetry.tokPerSec).toBe(181)
    // 82282 tokens
    expect(telemetry.totalTokens).toBe(82282)
    // 缓存命中率: 77000 / 81920 = 94%
    expect(telemetry.cacheHitRatio).toBe(94)
    // 上下文占比: 81920 / 128000 = 64%
    expect(telemetry.contextRatio).toBe(64)
  })

  test('computeThreadTelemetry 支持用户自定义 contextWindow 重新计算占比', () => {
    const thread: Thread = {
      id: 'th-custom',
      title: '自定义上下文会话',
      createdAt: Date.now(),
      workspace: 'C:/test',
      messages: [],
      items: [
        { kind: 'user', id: 'u1', at: 1000, text: '测试大上下文' },
        {
          kind: 'assistant',
          id: 'a1',
          at: 1030,
          text: '完成。',
          durationMs: 1000,
          usage: {
            promptTokens: 256000,
            completionTokens: 100,
            totalTokens: 256100,
          },
        },
      ],
    }

    // 若配置上限为 1,000,000，则 256000 / 1000000 = 26%
    const telemetry = computeThreadTelemetry(thread, false, 'glm-5.3-flash', 1_000_000)
    expect(telemetry.contextRatio).toBe(26)
  })

  test('computeThreadTelemetry 支持流式中实时估算', () => {
    const thread: Thread = {
      id: 'th-2',
      title: '流式遥测会话',
      createdAt: Date.now(),
      workspace: 'C:/test',
      messages: [],
      items: [
        { kind: 'user', id: 'u1', at: 1000, text: '流式测试' },
        {
          kind: 'assistant',
          id: 'a1',
          at: Date.now() - 1000, // 1秒前发起
          text: '正在持续输出模型内容中，文字越来越长...',
          streaming: true,
          usage: {
            promptTokens: 1000,
            completionTokens: 50,
            totalTokens: 1050,
            cachedTokens: 500,
          },
        },
      ],
    }

    const telemetry = computeThreadTelemetry(thread, true, 'gpt-4o')
    expect(telemetry.turns).toBe(1)
    expect(telemetry.steps).toBe(0)
    expect(telemetry.tokPerSec).toBeGreaterThan(0)
    expect(telemetry.cacheHitRatio).toBe(50)
  })

  test('computeThreadTelemetry 严格依据模型返回的 usage，不进行前端估算', () => {
    const thread: Thread = {
      id: 'th-model-usage',
      title: '模型 Token 会话',
      createdAt: Date.now(),
      workspace: 'C:/test',
      messages: [
        { role: 'user', content: '请帮我写一个简单的计数器' },
        { role: 'assistant', content: '这是一个基于 TypeScript 的简易计数器实现。' },
      ],
      items: [
        { kind: 'user', id: 'u1', at: 1000, text: '请帮我写一个简单的计数器' },
        {
          kind: 'assistant',
          id: 'a1',
          at: 1030,
          text: '这是一个基于 TypeScript 的简易计数器实现。',
          durationMs: 1000,
          // 模拟服务端未返回 usage 的情况
        },
      ],
    }

    const telemetry = computeThreadTelemetry(thread, false, 'gpt-4o')
    // 未返回 usage 时不进行前端伪造估算，应全部保持为 0
    expect(telemetry.totalCompletionTokens).toBe(0)
    expect(telemetry.totalTokens).toBe(0)
    expect(telemetry.currentContextTokens).toBe(0)
    expect(telemetry.tokPerSec).toBe(0)
  })

  test('convertMessagesToLlm 正确处理多模态图片输入与降级', () => {
    // 开启支持图片输入
    const llmMessagesVision = convertMessagesToLlm(
      '系统提示词',
      [
        {
          role: 'user',
          content: '请看这张图',
          images: ['data:image/png;base64,abc12345'],
        },
      ],
      { supportsImages: true }
    )
    expect(llmMessagesVision).toHaveLength(2)
    const userMsg = llmMessagesVision[1]
    expect(userMsg.role).toBe('user')
    expect(Array.isArray(userMsg.content)).toBe(true)
    if (Array.isArray(userMsg.content)) {
      expect(userMsg.content).toEqual([
        { type: 'text', text: '请看这张图' },
        { type: 'image_url', image_url: { url: 'data:image/png;base64,abc12345' } },
      ])
    }

    // 关闭支持图片输入（降级附加为文字提示）
    const llmMessagesFallback = convertMessagesToLlm(
      '',
      [
        {
          role: 'user',
          content: '分析图片',
          images: ['path/to/diagram.png'],
        },
      ],
      { supportsImages: false }
    )
    expect(llmMessagesFallback[0].content).toContain('分析图片')
    expect(llmMessagesFallback[0].content).toContain('[附带图片: path/to/diagram.png]')
  })
})

describeNative('ComposerTelemetryBar UI 渲染', () => {
  test('渲染输入框底部的遥测栏并验证不包含费用估计', async () => {
    store.active.items = [
      { kind: 'user', id: 'u1', at: 1000, text: '测试遥测渲染' },
      {
        kind: 'tool',
        id: 't1',
        at: 1010,
        callId: 'c1',
        name: 'test_tool',
        args: {},
        rawArgs: '',
        status: 'done',
      },
      {
        kind: 'assistant',
        id: 'a1',
        at: 1020,
        text: '测试回复',
        durationMs: 1000,
        usage: {
          promptTokens: 10000,
          completionTokens: 150,
          totalTokens: 10150,
          cachedTokens: 8000,
        },
      },
    ]

    const { render, renderer } = createTestRoot({ width: 1000, height: 400 })
    render(<Composer store={store} />)
    const app = await connectTest(renderer)

    const screenText = renderer.getPaintedText().join(' ')

    // 验证遥测栏容器存在
    expect(await app.getByTestId('composer-telemetry').count()).toBe(1)
    expect(await app.getByTestId('telemetry-turns-steps').count()).toBe(1)
    expect(await app.getByTestId('telemetry-tokens-cache').count()).toBe(1)
    expect(await app.getByTestId('telemetry-context-ratio').count()).toBe(1)

    // 验证展示轮数、步数、tok/s
    expect(screenText).toContain('1 轮 1 步')
    expect(screenText).toContain('150 tok/s')

    // 验证 Token 统计与缓存命中率
    expect(screenText).toContain('缓存命中 80%')

    // 严格验证：绝不包含费用估计（如「费用」、「¥」、「$」等）
    expect(screenText).not.toContain('费用')
    expect(screenText).not.toContain('¥')
    expect(screenText).not.toContain('$')

    await app.close()
  })

  test('居中空会话模式下不渲染遥测栏', async () => {
    store.active.items = []

    const { render, renderer } = createTestRoot({ width: 1000, height: 400 })
    render(<Composer store={store} centered={true} />)
    const app = await connectTest(renderer)

    // 空会话居中时不展示
    expect(await app.getByTestId('composer-telemetry').count()).toBe(0)

    await app.close()
  })

  test('开启 supportsImages 时渲染图片附件按键', async () => {
    store.supportsImages = true
    store.entries = ['assets/logo.png', 'readme.md']

    const { render, renderer } = createTestRoot({ width: 1000, height: 400 })
    render(<Composer store={store} />)
    const app = await connectTest(renderer)

    expect(await app.getByTestId('composer-attach-image').count()).toBe(1)

    await app.close()
  })
})
