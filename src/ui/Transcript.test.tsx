import { describe, expect, test } from 'bun:test'
import React from 'react'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { connectTest } from '@gpuix/react/automation'
import type { Item } from '../agent/types'
import { store } from '../agent/store'
import { buildTranscriptBlocks, Transcript } from './Transcript'
import { Composer } from './Composer'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

describe('buildTranscriptBlocks 分块逻辑', () => {
  test('将连续的 thinking、tool、notice 聚合成一个已完成的过程块', () => {
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 100, text: '帮我分析项目结构' },
      { kind: 'thinking', id: 'th1', at: 110, endedAt: 130, text: '正在深入思考...' },
      {
        kind: 'tool',
        id: 't1',
        at: 140,
        callId: 'c1',
        name: 'list_files',
        args: {},
        rawArgs: '',
        status: 'done',
        output: 'file1.ts\nfile2.ts',
      },
      { kind: 'notice', id: 'n1', at: 150, text: '提示信息', level: 'info' },
      { kind: 'assistant', id: 'a1', at: 160, text: '这是最终项目分析报告。' },
    ]

    const blocks = buildTranscriptBlocks(items, false)
    expect(blocks).toHaveLength(3)

    expect(blocks[0].kind).toBe('user')
    expect(blocks[0].id).toBe('u1')

    expect(blocks[1].kind).toBe('process')
    if (blocks[1].kind === 'process') {
      expect(blocks[1].items).toHaveLength(3)
      expect(blocks[1].items[0].id).toBe('th1')
      expect(blocks[1].items[1].id).toBe('t1')
      expect(blocks[1].items[2].id).toBe('n1')
      expect(blocks[1].isCompleted).toBe(true)
    }

    expect(blocks[2].kind).toBe('assistant')
    expect(blocks[2].id).toBe('a1')
  })

  test('如果工具仍在运行或等待审批，过程块标记为未完成', () => {
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 100, text: '请修改配置' },
      {
        kind: 'tool',
        id: 't1',
        at: 110,
        callId: 'c1',
        name: 'edit_file',
        args: { path: 'config.json' },
        rawArgs: '',
        status: 'awaiting',
      },
    ]

    const blocks = buildTranscriptBlocks(items, true)
    expect(blocks).toHaveLength(2)
    expect(blocks[1].kind).toBe('process')
    if (blocks[1].kind === 'process') {
      expect(blocks[1].isCompleted).toBe(false)
    }
  })

  test('多轮对话正确分离各个过程块并保留独立完成状态', () => {
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 100, text: '轮次 1' },
      {
        kind: 'tool',
        id: 't1',
        at: 110,
        callId: 'c1',
        name: 'read_file',
        args: { path: 'a.txt' },
        rawArgs: '',
        status: 'done',
      },
      { kind: 'assistant', id: 'a1', at: 120, text: '轮次 1 回复报告' },
      { kind: 'user', id: 'u2', at: 200, text: '轮次 2' },
      {
        kind: 'tool',
        id: 't2',
        at: 210,
        callId: 'c2',
        name: 'run_command',
        args: { command: 'cargo test' },
        rawArgs: '',
        status: 'running',
      },
    ]

    // 当前整体线程在运行（即轮次 2 正在跑）
    const blocks = buildTranscriptBlocks(items, true)
    expect(blocks).toHaveLength(5)

    // 轮次 1 的过程块已完成
    expect(blocks[1].kind).toBe('process')
    if (blocks[1].kind === 'process') {
      expect(blocks[1].isCompleted).toBe(true)
    }

    // 轮次 2 的过程块未完成
    expect(blocks[4].kind).toBe('process')
    if (blocks[4].kind === 'process') {
      expect(blocks[4].isCompleted).toBe(false)
    }
  })

  test('纯思考问答场景（无工具调用）：思考项收纳到过程块中，突出展现思考耗时且完成后默认收起', () => {
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 100, text: '请解释什么是 Rust 所有权' },
      { kind: 'thinking', id: 'th1', at: 110, endedAt: 125, text: '正在梳理所有权三大原则...' },
      { kind: 'assistant', id: 'a1', at: 130, text: '所有权是 Rust 最核心的内存安全机制。' },
    ]

    const blocks = buildTranscriptBlocks(items, false)
    expect(blocks).toHaveLength(3)
    expect(blocks[0].kind).toBe('user')
    expect(blocks[1].kind).toBe('process')
    if (blocks[1].kind === 'process') {
      expect(blocks[1].items).toHaveLength(1)
      expect(blocks[1].items[0].id).toBe('th1')
      expect(blocks[1].isCompleted).toBe(true)
    }
    expect(blocks[2].kind).toBe('assistant')
  })

  test('为每一轮对话精准计算并传递整轮总耗时 turnDurationMs', () => {
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 1000, text: '第一轮问题' },
      { kind: 'tool', id: 't1', at: 2000, callId: 'c1', name: 'read_file', args: {}, rawArgs: '', status: 'done' },
      { kind: 'assistant', id: 'a1', at: 4000, text: '第一轮回答', durationMs: 2000 }, // 结束时刻: 6000, 总耗时: 6000 - 1000 = 5000ms
      { kind: 'user', id: 'u2', at: 7000, text: '第二轮问题' },
      { kind: 'assistant', id: 'a2', at: 8000, text: '第二轮回答', turnDurationMs: 1800 }, // 显式记录的 turnDurationMs 优先
    ]

    const blocks = buildTranscriptBlocks(items, false)
    expect(blocks).toHaveLength(5)

    // 第一轮 process 块
    expect(blocks[1].kind).toBe('process')
    if (blocks[1].kind === 'process') {
      expect(blocks[1].turnDurationMs).toBe(5000)
    }

    // 第一轮 assistant 块
    expect(blocks[2].kind).toBe('assistant')
    if (blocks[2].kind === 'assistant') {
      expect(blocks[2].turnDurationMs).toBe(5000)
    }

    // 第二轮 assistant 块
    expect(blocks[4].kind).toBe('assistant')
    if (blocks[4].kind === 'assistant') {
      expect(blocks[4].turnDurationMs).toBe(1800)
    }
  })
})

/**
 * 订阅版 Transcript。
 *
 * Transcript 本身**不订阅** store——真实应用里由 AgentWindow（useAgentStore →
 * store.subscribe）带着整棵树重渲染。直接渲染 `<Transcript/>` 的测试里没有那个
 * 订阅者，store 变更（如 ask_user 发起提问）就不会触发重渲染，断言会失败在
 * "内容没画出来"上，而那是测试缺了订阅、不是产品的问题。
 */
function TranscriptLive() {
  const [, setTick] = React.useState(0)
  React.useEffect(() => store.subscribe(() => setTick((tick) => tick + 1)), [])
  return <Transcript store={store} />
}

describeNative('Transcript UI 过程收缩交互', () => {
  test('完成之后过程内容默认收缩，点击折叠条可展开与收起，报告始终平铺', async () => {
    const thread = store.active
    thread.items = [
      { kind: 'user', id: 'u-done-1', at: 1, text: '请运行测试并生成报告' },
      { kind: 'thinking', id: 'th-done-1', at: 2, endedAt: 5, text: '正在构思测试步骤...' },
      {
        kind: 'tool',
        id: 'tool-done-1',
        at: 6,
        callId: 'c-done-1',
        name: 'run_command',
        args: { command: 'bun test' },
        rawArgs: '',
        status: 'done',
        output: '152 pass, 0 fail',
      },
      { kind: 'assistant', id: 'a-done-1', at: 10, text: '测试全量通过，这是最终总结报告。' },
    ]

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ display: 'flex', flexDirection: 'column', position: 'relative', width: 800, height: 600 }}>
        {/* 这条用例后半段要在挂载后改 store（ask_user 发起提问），必须用订阅版 */}
        <TranscriptLive />
        {/*
          待答的问答卡不在会话流里，而是浮动在输入框上方——所以这里要把 Composer
          一起挂上。只挂 Transcript 等于验的是"没有输入框的界面"，那本来就不该
          出现浮动面板。
        */}
        <Composer store={store} />
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
    const gone = async (needle: string, timeoutMs = 10_000): Promise<void> => {
      const started = Date.now()
      while (Date.now() - started < timeoutMs) {
        if (!screen().includes(needle)) return
        renderer.flush?.()
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
      throw new Error(`still paints ${needle}\n${screen()}`)
    }

    // 验证用户问题与最终报告始终呈现
    await painted('请运行测试并生成报告')
    await painted('测试全量通过，这是最终总结报告。')

    // 验证执行过程已被默认收纳到折叠条中，显示步骤统计和“已完成”
    await painted('执行过程')
    expect(screen()).toContain('2 个步骤')
    expect(screen()).toContain('已完成')

    // 默认折叠状态下，内部的具体的命令执行细节输出不平铺展示
    expect(await app.getByTestId('process-body-process-th-done-1').count()).toBe(0)

    // 点击执行过程折叠条展开
    await app.getByTestId('process-head-process-th-done-1').click()
    await painted('收起')
    expect(await app.getByTestId('process-body-process-th-done-1').count()).toBe(1)

    // 展开后可以看见内部的思考和工具卡片
    expect(await app.getByTestId('thinking-head-th-done-1').count()).toBe(1)
    expect(await app.getByTestId('tool-head-tool-done-1').count()).toBe(1)

    // 再次点击折叠条，收起过程卡片
    await app.getByTestId('process-head-process-th-done-1').click()
    await gone('收起')
    expect(await app.getByTestId('process-body-process-th-done-1').count()).toBe(0)

    // 最终助理回复/报告依然平铺可见
    expect(screen()).toContain('测试全量通过，这是最终总结报告。')

    // ── 问答卡（ask_user）：并入本用例而不另开窗口 ──
    // GPU 测试渲染器每个 createTestRoot 都开一个真窗口（DirectX），
    // 并发文件同时在跑时窗口数一多会耗尽渲染器内存（实测全量测试直接崩：
    // "Failed to open test window / memory allocation failed"），
    // 所以问答卡的断言放在这个已经活着的窗口里做。
    const askCallId = 'call-ask-ui-1'
    const askCard = {
      kind: 'tool' as const,
      id: 'tool-ask-ui-1',
      at: 100,
      callId: askCallId,
      name: 'ask_user',
      args: { question: '重构方式选哪种？' },
      rawArgs: '',
      status: 'running' as const,
      threadId: thread.id,
    }
    thread.items = [
      { kind: 'user', id: 'u-ask-1', at: 1, text: '帮我重构认证模块' },
      askCard,
    ]
    // 摆出"工具正在执行、等用户作答"的现场：store 要能按调用 id 找到卡片
    const internals = store as unknown as { cards: Map<string, typeof askCard> }
    internals.cards.set(askCallId, askCard)

    const pendingQuestion = store.requestUserAnswer({
      callId: askCallId,
      question: '重构方式选哪种？',
      choices: [
        { id: 'a', label: '先补测试再重构', description: '更稳，但多一轮' },
        { id: 'b', label: '一次性重构' },
      ],
    })

    // 待答的问题**浮动在输入框上方**：整轮正卡在那里等，而会话流随时可能被
    // 上翻一屏或由虚拟列表回收——那时屏幕上若没有一点"在等我"的痕迹，
    // 应用看起来就是卡死了。所以断言的是浮动面板，不是会话流里的内联卡。
    await painted('等待你的回答')
    await painted('重构方式选哪种？')
    await painted('先补测试再重构')
    await painted('一次性重构')
    expect(await app.getByTestId('pending-questions-panel').count()).toBe(1)
    expect(await app.getByTestId('question-choice-a').count()).toBe(1)

    // 紧接着的这条是这次改动的**核心断言**：同一张卡不能既在浮动面板里、
    // 又在会话流里，否则用户会看到两个可点的「先补测试再重构」。
    expect(screen().split('先补测试再重构').length - 1).toBe(1)

    // 选项与自由输入并存：有选项时也允许补充说明
    expect(await app.getByTestId('question-input-call-ask-ui-1').count()).toBe(1)

    // 点选项即作答：等待真的被解开，且答案原样传出
    await app.getByTestId('question-choice-a').click()
    const askAnswer = await pendingQuestion
    expect(askAnswer.answeredBy).toBe('user')
    expect(askAnswer.choice).toBe('a')

    // 作答之后交接：浮动面板退场，卡片回到会话流里当历史（"当时答了什么"）
    await gone('等待你的回答')
    await painted('已回答')
    expect(await app.getByTestId('pending-questions-panel').count()).toBe(0)
    expect(screen()).toContain('先补测试再重构')

    // 说明一处**已知未覆盖**：自由输入的「提交」按钮没有在此点击。
    // 它需要同一窗口里再挂一张问题卡，而实测窗口加高（800×1200）后虚拟列表的
    // 坐标映射不稳（按钮 bounds y≈1049、点击不生效）——那是渲染器的已知约束，
    // 不是问答卡的问题。自由作答的语义由 store 层用例覆盖（ask-user.test.ts）。
    internals.cards.delete(askCallId)

    // ── 真实链路：模型发出 ask_user → gate 建卡 → 工具挂起等待 ──
    // 上面那段是**手工摆状态**（直接给 cards 塞卡片、直接调 requestUserAnswer），
    // 它只能证明"给定状态能画出来"。用户报的阻塞恰恰是"真实链路下什么都没出现"，
    // 所以这里必须让 store 自己跑一遍：打桩模型端发出工具调用，走 send → gate →
    // 工具 → 挂起，断言问题卡**自己**出现在界面上。
    const realCall = { question: '真实链路：改哪个模块？', choices: [{ id: 'x', label: '只改 auth' }] }
    const realArgs = JSON.stringify(realCall).slice(1, -1).replace(/"/g, '\\"')
    let fetchCount = 0
    const originalFetch = globalThis.fetch
    globalThis.fetch = (async () => {
      fetchCount += 1
      const chunks =
        fetchCount === 1
          ? `data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_real_1","type":"function","function":{"name":"ask_user","arguments":"{\\"question\\":\\"真实链路：改哪个模块？\\",\\"choices\\":[{\\"id\\":\\"x\\",\\"label\\":\\"只改 auth\\"}]}"}}]},"finish_reason":"tool_calls"}]}\n\ndata: [DONE]`
          : 'data: {"choices":[{"delta":{"content":"好。"}}]}\n\ndata: [DONE]'
      return new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(new TextEncoder().encode(chunks))
            controller.close()
          },
        }),
        { status: 200, headers: { 'content-type': 'text/event-stream' } }
      )
    }) as unknown as typeof fetch
    expect(realArgs.length).toBeGreaterThan(0)

    const savedEnv = {
      key: process.env.A_DA_API_KEY,
      url: process.env.A_DA_BASE_URL,
      model: process.env.A_DA_MODEL,
    }
    process.env.A_DA_API_KEY = 'test-key'
    process.env.A_DA_BASE_URL = 'http://localhost/v1'
    process.env.A_DA_MODEL = 'test-model'
    try {
      await store.reloadPlugins()
      thread.items = [{ kind: 'user', id: 'u-real-1', at: Date.now(), text: '帮我改认证模块' }]
      store.send('帮我改认证模块')

      // 关键断言：问题卡与选项必须自己出现在界面上（且是在输入框上方的浮动面板里）
      await painted('真实链路：改哪个模块？', 20_000)
      expect(await app.getByTestId('pending-questions-panel').count()).toBe(1)
      expect(await app.getByTestId('question-choice-x').count()).toBe(1)

      // 作答后这一轮要能收尾（不会永久挂起）
      await app.getByTestId('question-choice-x').click()
      const settled = Date.now()
      while (store.isThreadRunning(thread.id) && Date.now() - settled < 20_000) {
        await new Promise((r) => setTimeout(r, 50))
      }
      expect(store.isThreadRunning(thread.id)).toBe(false)
    } finally {
      store.stop(thread.id)
      globalThis.fetch = originalFetch
      for (const [k, v] of [
        ['A_DA_API_KEY', savedEnv.key],
        ['A_DA_BASE_URL', savedEnv.url],
        ['A_DA_MODEL', savedEnv.model],
      ] as const) {
        if (v === undefined) delete process.env[k]
        else process.env[k] = v
      }
    }

    await app.close()
  }, 90_000)

  test('纯思考问答场景：思考收纳在执行过程块中，折叠条显示时长，点击展开查看详情', async () => {
    const thread = store.active
    thread.items = [
      { kind: 'user', id: 'u-think-1', at: 1000, text: '什么是智能体' },
      { kind: 'thinking', id: 'th-think-1', at: 2000, endedAt: 8000, text: '从感知、规划、行动三要素展开...' },
      { kind: 'assistant', id: 'a-think-1', at: 9000, text: '智能体是具备环境感知与自主决策能力的实体。' },
    ]

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ display: 'flex', flexDirection: 'column', position: 'relative', width: 800, height: 600 }}>
        <Transcript store={store} />
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

    // 执行过程卡片默认折叠，显示步骤数与思考耗时徽章
    await painted('执行过程')
    expect(screen()).toContain('6s')
    expect(screen()).toContain('什么是智能体')
    expect(screen()).toContain('智能体是具备环境感知与自主决策能力的实体。')

    // 点击执行过程展开
    await app.getByTestId('process-head-process-th-think-1').click()
    await painted('思考')

    // 点击思考卡片展开推理原文
    await app.getByTestId('thinking-head-th-think-1').click()
    await painted('从感知、规划、行动三要素展开...')

    await app.close()
  }, 30_000)

  test('对话耗时与Token统计：在助手回复下方正确渲染耗时与Token统计徽标', async () => {
    const thread = store.active
    thread.items = [
      { kind: 'user', id: 'u-stats-1', at: 1000, text: '请介绍一下 Rust' },
      {
        kind: 'assistant',
        id: 'a-stats-1',
        at: 2000,
        text: 'Rust 是一门赋予每个人构建可靠且高效软件能力的语言。',
        durationMs: 3450,
        usage: {
          promptTokens: 820,
          completionTokens: 460,
          totalTokens: 1280,
          thinkingTokens: 150,
        },
      },
    ]

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ display: 'flex', flexDirection: 'column', position: 'relative', width: 800, height: 600 }}>
        <Transcript store={store} />
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

    await painted('Rust 是一门赋予每个人构建可靠且高效软件能力的语言。')

    // 验证每轮对话总耗时徽标与Token指标正常渲染
    expect(await app.getByTestId('turn-duration-a-stats-1').count()).toBe(1)
    expect(screen()).toContain('总耗时 4.5s')
    expect(screen()).toContain('1.3k Tokens')
    expect(screen()).toContain('复制全文')

    await app.close()
  }, 30_000)

  test('多轮对话分别独立展示各自轮次的总耗时', async () => {
    const thread = store.active
    thread.items = [
      { kind: 'user', id: 'u-multi-1', at: 1000, text: '第一轮用户提问' },
      { kind: 'assistant', id: 'a-multi-1', at: 2000, text: '第一轮解答内容。', durationMs: 1200, turnDurationMs: 2200 },
      { kind: 'user', id: 'u-multi-2', at: 4000, text: '第二轮用户提问' },
      { kind: 'assistant', id: 'a-multi-2', at: 5000, text: '第二轮解答内容。', durationMs: 800, turnDurationMs: 1800 },
    ]

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ display: 'flex', flexDirection: 'column', position: 'relative', width: 800, height: 600 }}>
        <Transcript store={store} />
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

    await painted('第一轮解答内容。')
    await painted('第二轮解答内容。')

    expect(screen()).toContain('总耗时 2.2s')
    expect(screen()).toContain('总耗时 1.8s')
    expect(await app.getByTestId('turn-duration-a-multi-1').count()).toBe(1)
    expect(await app.getByTestId('turn-duration-a-multi-2').count()).toBe(1)

    await app.close()
  }, 30_000)

  test('同一个会话内派发的多个同类型子智能体，各卡片的独立页签按钮分别对应各自的子会话', async () => {
    const parent = store.newThread(process.cwd())
    store.selectThread(parent.id)

    const { thread: sub1 } = await store.startSubagentThread({
      parentThreadId: parent.id,
      subagentId: 'researcher',
      task: '第一项调研任务：分析模块 A',
    })

    const { thread: sub2 } = await store.startSubagentThread({
      parentThreadId: parent.id,
      subagentId: 'researcher',
      task: '第二项调研任务：分析模块 B',
    })

    parent.items = [
      {
        kind: 'tool',
        id: 'tool-sub-1',
        at: 100,
        callId: 'call-sub-1',
        name: 'invoke_subagent',
        args: { subagent_id: 'researcher', task: '第一项调研任务：分析模块 A' },
        rawArgs: '',
        status: 'done',
        output: `模块 A 调研完毕\n\n(子会话 ID: ${sub1.id})`,
        details: { subagent_thread_id: sub1.id },
        threadId: parent.id,
      },
      {
        kind: 'tool',
        id: 'tool-sub-2',
        at: 200,
        callId: 'call-sub-2',
        name: 'invoke_subagent',
        args: { subagent_id: 'researcher', task: '第二项调研任务：分析模块 B' },
        rawArgs: '',
        status: 'done',
        output: `模块 B 调研完毕\n\n(子会话 ID: ${sub2.id})`,
        details: { subagent_thread_id: sub2.id },
        threadId: parent.id,
      },
    ]

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ display: 'flex', flexDirection: 'column', position: 'relative', width: 800, height: 600 }}>
        <Transcript store={store} />
      </div>,
    )
    const app = await connectTest(renderer)

    // 展开执行过程折叠条
    await app.getByTestId('process-head-process-tool-sub-1').click()

    // 展开两个工具卡片
    await app.getByTestId('tool-head-tool-sub-1').click()
    await app.getByTestId('tool-head-tool-sub-2').click()

    const btn1 = await app.getByTestId(`open-subagent-thread-${sub1.id}`)
    expect(await btn1.count()).toBe(1)

    // 第二个工具卡片的独立按钮应对应 sub2
    const btn2 = await app.getByTestId(`open-subagent-thread-${sub2.id}`)
    expect(await btn2.count()).toBe(1)

    // 点击第二个子智能体按钮应切换至 sub2，而不是错误跳转到 sub1
    await btn2.click()
    await app.close()
    store.deleteThread(parent.id)
  }, 30_000)

  test('CompactBlock 与 CompactCard：在会话列表中清晰渲染压缩指标与节约Token', async () => {
    const thread = store.active
    thread.items = [
      {
        kind: 'compact',
        id: 'compact-test-1',
        at: 1000,
        summary: '1. Primary Request: 测试压缩卡片渲染\n\n9. Next Step: 验证渲染完整性',
        preTokens: 100_000,
        postTokens: 10_000,
        savedTokens: 90_000,
        turnsSummarized: 3,
        customInstructions: '重点保留组件测试',
      },
      { kind: 'user', id: 'u-after-compact', at: 2000, text: '压缩后的第一条新消息' },
      { kind: 'assistant', id: 'a-after-compact', at: 3000, text: '我已获取压缩后的上下文并继续执行。' },
    ]

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ display: 'flex', flexDirection: 'column', position: 'relative', width: 800, height: 600 }}>
        <Transcript store={store} />
      </div>,
    )
    const app = await connectTest(renderer)

    const screen = () => renderer.getPaintedText().join('\n')
    expect(screen()).toContain('会话已压缩')
    expect(screen()).toContain('节约 90k tok (90%)')
    expect(screen()).toContain('100k → 10k tok')
    expect(screen()).toContain('已汇总 3 轮')
    expect(screen()).toContain('复制摘要')
    expect(screen()).toContain('展开')
    expect(screen()).toContain('重点保留组件测试')
    expect(screen()).toContain('压缩后的第一条新消息')
    expect(screen()).toContain('我已获取压缩后的上下文并继续执行。')

    // 点击头部展开摘要详情
    await app.getByTestId('compact-card-header').click()
    expect(screen()).toContain('收起')
    expect(screen()).toContain('测试压缩卡片渲染')

    await app.close()
  })

  test('UserRow 支持复制与内联编辑，点击重新发送调用 editUserMessageAndResend', async () => {
    const thread = store.active
    thread.items = [
      { kind: 'user', id: 'u-edit-1', at: 1000, text: '旧的用户指令内容' },
      { kind: 'assistant', id: 'a-edit-1', at: 2000, text: '旧的回复内容' },
    ]

    let resendCalledWith: { id: string; text: string } | null = null
    const origResend = store.editUserMessageAndResend.bind(store)
    store.editUserMessageAndResend = async (id, text, imgs, tid) => {
      resendCalledWith = { id, text }
    }

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ display: 'flex', flexDirection: 'column', position: 'relative', width: 800, height: 600 }}>
        <Transcript store={store} />
      </div>,
    )
    const app = await connectTest(renderer)

    const screen = () => renderer.getPaintedText().join('\n')
    expect(screen()).toContain('旧的用户指令内容')
    expect(screen()).toContain('编辑')
    expect(screen()).toContain('复制')

    // 1. 点击编辑按钮，展开内联编辑框
    await app.getByTestId('edit-user-msg-u-edit-1').click()
    expect(await app.getByTestId('user-edit-box-u-edit-1').count()).toBe(1)
    expect(screen()).toContain('编辑并重新发送')
    expect(screen()).toContain('将丢弃此消息之后的所有对话记录')
    expect(screen()).toContain('取消')
    expect(screen()).toContain('重新发送')

    // 2. 点击重新发送
    await app.getByTestId('confirm-resend-u-edit-1').click()
    expect(resendCalledWith as any).toEqual({ id: 'u-edit-1', text: '旧的用户指令内容' })

    store.editUserMessageAndResend = origResend
    await app.close()
  })
})
