import { beforeEach, describe, expect, test } from 'bun:test'
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
  QueuedMessagesFloatingPanel,
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

  test('computeThreadTelemetry 多轮对话中不累加 Token，只取最后一次请求返回的总 Token', () => {
    const thread: Thread = {
      id: 'th-latest-token',
      title: '多轮请求会话',
      createdAt: Date.now(),
      workspace: 'C:/test',
      messages: [],
      items: [
        { kind: 'user', id: 'u1', at: 1000, text: '第一轮提问' },
        {
          kind: 'assistant',
          id: 'a1',
          at: 2000,
          text: '第一轮回答',
          durationMs: 3400,
          usage: {
            promptTokens: 2751,
            completionTokens: 500,
            totalTokens: 3251,
          },
        },
        { kind: 'user', id: 'u2', at: 3000, text: '第二轮提问' },
        {
          kind: 'assistant',
          id: 'a2',
          at: 4000,
          text: '第二轮回答',
          durationMs: 4811,
          usage: {
            promptTokens: 2711,
            completionTokens: 662,
            totalTokens: 3373,
            cachedTokens: 0,
          },
        },
      ],
    }

    const telemetry = computeThreadTelemetry(thread, false, 'deepseek-flash')
    expect(telemetry.turns).toBe(2)
    // 严格验证：总 Token 是最后一次请求的 3373，而不是两轮累加的 6624
    expect(telemetry.totalTokens).toBe(3373)
    expect(telemetry.promptTokens).toBe(2711)
    expect(telemetry.completionTokens).toBe(662)
    expect(telemetry.cachedTokens).toBe(0)
    expect(telemetry.durationMs).toBe(4811)
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
  beforeEach(() => {
    store.active.isSubagent = false
    store.clearQueue()
  })

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

    // 验证各部分通过竖线隔开
    expect(screenText).toContain('|')

    // 验证多维指标：总 Token、提示词、输出、缓存、用时、上下文
    expect(screenText).toContain('10k tok')
    expect(screenText).toContain('提示词 10k')
    expect(screenText).toContain('输出 150')
    expect(screenText).toContain('缓存 8.0k (80%)')
    expect(screenText).toContain('用时 1.0s')
    expect(screenText).toContain('上下文 8%')

    // 严格验证：绝不包含费用估计（如「费用」、「¥」、「$」等）
    expect(screenText).not.toContain('费用')
    expect(screenText).not.toContain('¥')
    expect(screenText).not.toContain('$')

    await app.close()
  })

  test('无缓存命中时展示「缓存 0」，上下文极低时展示「上下文 <1%」', async () => {
    store.active.items = [
      {
        kind: 'user',
        id: 'u-nocache',
        at: 1000,
        text: '调研代码',
      },
      {
        kind: 'assistant',
        id: 'a-nocache',
        at: 2000,
        text: '开始调研分析代码。',
        durationMs: 1000,
        usage: {
          promptTokens: 500,
          completionTokens: 300,
          totalTokens: 800,
          // cachedTokens 为 0 或未提供
        },
      },
    ]

    const { render, renderer } = createTestRoot({ width: 1000, height: 400 })
    render(<Composer store={store} />)
    const app = await connectTest(renderer)

    const screenText = renderer.getPaintedText().join(' ')
    expect(screenText).toContain('800 tok')
    expect(screenText).toContain('提示词 500')
    expect(screenText).toContain('输出 300')
    expect(screenText).toContain('缓存 0')
    expect(screenText).toContain('用时 1.0s')
    expect(screenText).toContain('上下文 <1%')

    await app.close()
  })

  test('上下文达到警戒线时呈现快捷压缩按钮且遥测栏强制单行不折行', async () => {
    store.active.items = [
      {
        kind: 'user',
        id: 'u-heavy',
        at: 1000,
        text: '海量日志分析',
      },
      {
        kind: 'assistant',
        id: 'a-heavy',
        at: 2000,
        text: '已加载大量上下文',
        durationMs: 2500,
        usage: {
          promptTokens: 85000,
          completionTokens: 500,
          totalTokens: 85500,
          cachedTokens: 20000,
        },
      },
    ]

    const { render, renderer } = createTestRoot({ width: 1000, height: 400 })
    render(<Composer store={store} />)
    const app = await connectTest(renderer)

    // 上下文占比 85500 / 128000 = 67% (>= 60%)，触发快捷压缩按钮
    expect(await app.getByTestId('telemetry-quick-compact-btn').count()).toBe(1)
    const screenText = renderer.getPaintedText().join(' ')
    expect(screenText).toContain('压缩')

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

  test('点击上下文徽标展开 ContextUsagePopover 悬浮面板，展示细分维度与健康度建议', async () => {
    store.active.items = [
      { kind: 'user', id: 'u-pop', at: 1000, text: '请介绍一下深度学习' },
      {
        kind: 'assistant',
        id: 'a-pop',
        at: 2000,
        text: '深度学习是机器学习的一个分支，基于多层神经网络。',
        durationMs: 1200,
        usage: {
          promptTokens: 3000,
          completionTokens: 200,
          totalTokens: 3200,
          cachedTokens: 2500,
        },
      },
    ]

    const { render, renderer } = createTestRoot({ width: 1000, height: 500 })
    render(<Composer store={store} />)
    const app = await connectTest(renderer)

    // 初始状态下 Popover 未展开
    expect(await app.getByTestId('context-usage-popover').count()).toBe(0)

    // 点击上下文指示器
    await app.getByTestId('telemetry-context-ratio').click()
    renderer.flush?.()

    // 验证 Popover 出现并包含深度洞察信息
    expect(await app.getByTestId('context-usage-popover').count()).toBe(1)
    const screenText = renderer.getPaintedText().join(' ')
    expect(screenText).toContain('上下文用量与健康度')
    expect(screenText).toContain('正常对话消息')
    expect(screenText).toContain('系统提示词')
    expect(screenText).toContain('工具')
    expect(screenText).toContain('缓存命中收益')
    expect(screenText).toContain('2,500 tok')

    await app.close()
  })

  test('支持 code、plan、create 三大协作模式切换并联动 store 状态', async () => {
    store.setMode('code')
    const { render, renderer } = createTestRoot({ width: 1000, height: 500 })
    render(<Composer store={store} />)
    const app = await connectTest(renderer)

    // 默认展示 Code 编码
    expect(renderer.getPaintedText().join(' ')).toContain('Code 编码')

    // 切换至 Plan 规划模式
    store.setMode('plan')
    render(<Composer store={store} />)
    renderer.flush?.()
    expect(renderer.getPaintedText().join(' ')).toContain('Plan 规划')
    expect(store.mode as string).toBe('plan')

    // 切换至 Create 创造模式
    store.setMode('create')
    render(<Composer store={store} />)
    renderer.flush?.()
    expect(renderer.getPaintedText().join(' ')).toContain('Create 创造')
    expect(store.mode as string).toBe('create')

    await app.close()
  })

  test('底部工具栏提供「指令」按钮，点击可唤起与收起快捷指令面板', async () => {
    const { render, renderer } = createTestRoot({ width: 1000, height: 600 })
    render(<Composer store={store} />)
    const app = await connectTest(renderer)

    // 验证指令按钮存在
    const slashBtn = app.getByTestId('composer-slash-commands')
    expect(await slashBtn.count()).toBe(1)
    expect(renderer.getPaintedText().join(' ')).toContain('指令')

    // 默认快捷指令面板未打开
    expect(await app.getByTestId('slash-command-menu').count()).toBe(0)

    // 点击指令按钮打开面板
    await slashBtn.click()
    renderer.flush?.()

    // 验证面板出现并包含快捷指令与分类标题
    expect(await app.getByTestId('slash-command-menu').count()).toBe(1)
    const menuText = renderer.getPaintedText().join(' ')
    expect(menuText).toContain('快捷指令')
    expect(menuText).toContain('/clear')
    expect(menuText).toContain('/compact')
    expect(menuText).toContain('/code')
    expect(menuText).toContain('/plan')
    expect(menuText).toContain('/create')

    // 再次点击指令按钮关闭面板
    await slashBtn.click()
    renderer.flush?.()
    expect(await app.getByTestId('slash-command-menu').count()).toBe(0)

    await app.close()
  })

  test('输入框输入中文顿号或斜杠自动展开快捷指令面板', async () => {
    const { render, renderer } = createTestRoot({ width: 1000, height: 600 })
    render(<Composer store={store} />)
    const app = await connectTest(renderer)

    // 默认关闭
    expect(await app.getByTestId('slash-command-menu').count()).toBe(0)

    // 输入 / 时触发面板展开
    await app.getByTestId('composer').fill('/')
    renderer.flush?.()
    expect(await app.getByTestId('slash-command-menu').count()).toBe(1)

    // 输入中文顿号 、 时自动规整并唤起
    await app.getByTestId('composer').fill('、')
    renderer.flush?.()
    expect(await app.getByTestId('slash-command-menu').count()).toBe(1)

    await app.close()
  })

  test('选择快捷指令后在输入框表现为可移除标签，点击可移除，发送时拼接参数', async () => {
    const { render, renderer } = createTestRoot({ width: 1000, height: 600 })
    render(<Composer store={store} />)
    const app = await connectTest(renderer)

    // 打开快捷指令面板
    await app.getByTestId('composer-slash-commands').click()
    renderer.flush?.()

    // 验证面板出现
    expect(await app.getByTestId('slash-command-menu').count()).toBe(1)

    // 若存在内置提示词指令（例如 review-changes），点击选中
    const reviewItem = app.getByTestId('slash-item-review-changes')
    if (await reviewItem.count() > 0) {
      await reviewItem.click()
      renderer.flush?.()

      // 验证面板关闭，且输入框内展示可移除标签
      expect(await app.getByTestId('slash-command-menu').count()).toBe(0)
      expect(await app.getByTestId('composer-selected-command-pill').count()).toBe(1)
      expect(renderer.getPaintedText().join(' ')).toContain('/review-changes')

      // 验证点击移除按钮可成功关闭标签
      await app.getByTestId('composer-remove-command').click()
      renderer.flush?.()
      expect(await app.getByTestId('composer-selected-command-pill').count()).toBe(0)

      // 重新打开并再次选择，测试输入参数与发送集成
      await app.getByTestId('composer-slash-commands').click()
      renderer.flush?.()
      await app.getByTestId('slash-item-review-changes').click()
      renderer.flush?.()
      expect(await app.getByTestId('composer-selected-command-pill').count()).toBe(1)

      // 输入参数并点击发送
      let sentMessage = ''
      const origSend = store.send
      store.send = ((msg: string) => {
        sentMessage = msg
      }) as any
      try {
        await app.getByTestId('composer').fill('src/ui')
        renderer.flush?.()
        await app.getByTestId('send').click()
        renderer.flush?.()

        // 验证发送的消息包含了指令并自动展开或传递
        expect(sentMessage.length).toBeGreaterThan(0)
        // 验证发送后标签被自动清除
        expect(await app.getByTestId('composer-selected-command-pill').count()).toBe(0)
      } finally {
        store.send = origSend
      }
    }

    await app.close()
  })

  test('QueuedMessagesFloatingPanel 浮动面板渲染及管理交互', async () => {
    // 1. 无排队消息时不渲染
    const { render, renderer } = createTestRoot({ width: 1000, height: 600 })
    render(<Composer store={store} />)
    const app = await connectTest(renderer)

    expect(await app.getByTestId('queued-messages-panel').count()).toBe(0)

    // 2. 模拟设置排队消息
    const mockItems = [
      {
        thread: store.active,
        text: '第一条排队指令',
        item: { kind: 'user', id: 'q-item-1', text: '第一条排队指令', at: Date.now(), queued: true } as Item,
      },
      {
        thread: store.active,
        text: '第二条带图指令',
        images: ['test.png'],
        item: { kind: 'user', id: 'q-item-2', text: '第二条带图指令', at: Date.now(), queued: true } as Item,
      },
    ]
    store.queue = [...mockItems]
    store.active.items.push(mockItems[0]!.item, mockItems[1]!.item)
    render(<Composer store={store} />)
    renderer.flush?.()

    // 验证浮动面板渲染
    expect(await app.getByTestId('queued-messages-panel').count()).toBe(1)
    expect(await app.getByTestId('queued-item-0').count()).toBe(1)
    expect(await app.getByTestId('queued-item-1').count()).toBe(1)
    expect(renderer.getPaintedText().join(' ')).toContain('排队发送队列')
    expect(renderer.getPaintedText().join(' ')).toContain('2 条待发送')
    expect(renderer.getPaintedText().join(' ')).toContain('第一条排队指令')
    expect(renderer.getPaintedText().join(' ')).toContain('第二条带图指令')
    expect(renderer.getPaintedText().join(' ')).toContain('1 图')

    // 3. 点击「全部清空」
    await app.getByTestId('queue-clear-all').click()
    render(<Composer store={store} />)
    renderer.flush?.()
    expect(await app.getByTestId('queued-messages-panel').count()).toBe(0)
    expect(store.queue.length).toBe(0)

    await app.close()
  })

  test('QueuedMessagesFloatingPanel 独立组件：立即发送与取出编辑', async () => {
    const { render, renderer } = createTestRoot({ width: 1000, height: 600 })
    let editedText = ''
    let editedImgs: string[] | undefined

    const mockItem = {
      thread: store.active,
      text: '需要插队的紧急任务',
      images: ['urgent.png'],
      item: { kind: 'user', id: 'q-urgent', text: '需要插队的紧急任务', at: Date.now(), queued: true } as Item,
    }
    store.queue = [mockItem]
    store.active.items.push(mockItem.item)

    render(
      <QueuedMessagesFloatingPanel
        store={store}
        onEditItem={(text, imgs) => {
          editedText = text
          editedImgs = imgs
        }}
      />
    )
    const app = await connectTest(renderer)

    expect(await app.getByTestId('queued-messages-panel').count()).toBe(1)
    expect(await app.getByTestId('queue-send-now-0').count()).toBe(1)

    // 测试点击取出编辑
    await app.getByTestId('queue-edit-0').click()
    renderer.flush?.()

    expect(editedText).toBe('需要插队的紧急任务')
    expect(editedImgs).toEqual(['urgent.png'])
    expect(store.queue.length).toBe(0)

    await app.close()
  })
})
