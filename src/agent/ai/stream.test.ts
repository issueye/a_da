import { describe, expect, test } from 'bun:test'
import { ThinkTagFilter } from './stream'

describe('ThinkTagFilter 正文思考标签抽取', () => {
  test('完整提取 <think>...</think> 标签中的思考内容与标签外的回答内容', () => {
    const filter = new ThinkTagFilter()
    const chunks = [
      '<think>\n正在分析用户问题...',
      '\n梳理架构逻辑\n</think>\n',
      '这是正式的最终回答内容。',
    ]

    const outputs: Array<{ type: 'thinking' | 'text'; text: string }> = []
    for (const chunk of chunks) {
      outputs.push(...filter.feed(chunk))
    }
    outputs.push(...filter.flush())

    const thinkings = outputs.filter((o) => o.type === 'thinking').map((o) => o.text).join('')
    const texts = outputs.filter((o) => o.type === 'text').map((o) => o.text).join('')

    expect(thinkings).toBe('\n正在分析用户问题...\n梳理架构逻辑\n')
    expect(texts).toBe('\n这是正式的最终回答内容。')
  })

  test('正确处理标签被跨 chunk 切断的极端情况（如 <th 和 ink>）', () => {
    const filter = new ThinkTagFilter()
    const chunks = [
      '前言<th',
      'ink>第一阶段思考',
      '第二阶段思考</th',
      'ink>正式回复',
    ]

    const outputs: Array<{ type: 'thinking' | 'text'; text: string }> = []
    for (const chunk of chunks) {
      outputs.push(...filter.feed(chunk))
    }
    outputs.push(...filter.flush())

    const thinkings = outputs.filter((o) => o.type === 'thinking').map((o) => o.text).join('')
    const texts = outputs.filter((o) => o.type === 'text').map((o) => o.text).join('')

    expect(texts).toBe('前言正式回复')
    expect(thinkings).toBe('第一阶段思考第二阶段思考')
  })

  test('无思考标签时正常透传所有正文', () => {
    const filter = new ThinkTagFilter()
    const outputs = filter.feed('你好，这是一个常规回答。')
    expect(outputs).toHaveLength(1)
    expect(outputs[0].type).toBe('text')
    expect(outputs[0].text).toBe('你好，这是一个常规回答。')
  })
})

describe('streamModelChat Token 统计与流式参数', () => {
  test('发起流式请求时正确设置 stream_options 并解析 usage', async () => {
    const { streamModelChat } = await import('./stream')
    const originalFetch = globalThis.fetch

    let sentBody: any = null
    const sseData = [
      'data: {"choices":[{"delta":{"content":"你好"}}]}\n\n',
      'data: {"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"completion_tokens_details":{"reasoning_tokens":2}}}\n\n',
      'data: [DONE]\n\n',
    ].join('')

    globalThis.fetch = (async (input: any, init: any) => {
      sentBody = JSON.parse(init.body)
      const encoder = new TextEncoder()
      const stream = new ReadableStream({
        start(controller) {
          controller.enqueue(encoder.encode(sseData))
          controller.close()
        },
      })
      return new Response(stream, { status: 200, headers: { 'content-type': 'text/event-stream' } })
    }) as any

    try {
      const deltas = []
      for await (const delta of streamModelChat(
        { baseUrl: 'https://api.example.com/v1', apiKey: 'test-key', model: 'gpt-4o' },
        [{ role: 'user', content: '测试' }]
      )) {
        deltas.push(delta)
      }

      // 验证 stream_options 配置
      expect(sentBody).toBeDefined()
      expect(sentBody.stream).toBe(true)
      expect(sentBody.stream_options).toEqual({ include_usage: true })

      // 验证 usage 事件正确产出
      const usageDelta = deltas.find((d) => d.type === 'usage')
      expect(usageDelta).toBeDefined()
      expect(usageDelta?.usage?.promptTokens).toBe(10)
      expect(usageDelta?.usage?.completionTokens).toBe(5)
      expect(usageDelta?.usage?.totalTokens).toBe(15)
      expect(usageDelta?.usage?.thinkingTokens).toBe(2)
    } finally {
      globalThis.fetch = originalFetch
    }
  })

  test('服务端无 usage 字段时不产生伪造 usage，严格依据模型返回', async () => {
    const { streamModelChat } = await import('./stream')
    const originalFetch = globalThis.fetch

    const sseData = [
      'data: {"choices":[{"delta":{"content":"这是测试回复内容"}}]}\n\n',
      'data: [DONE]\n\n',
    ].join('')

    globalThis.fetch = (async () => {
      const encoder = new TextEncoder()
      const stream = new ReadableStream({
        start(controller) {
          controller.enqueue(encoder.encode(sseData))
          controller.close()
        },
      })
      return new Response(stream, { status: 200, headers: { 'content-type': 'text/event-stream' } })
    }) as any

    try {
      const deltas = []
      for await (const delta of streamModelChat(
        { baseUrl: 'https://api.example.com/v1', apiKey: 'test-key', model: 'gpt-4o' },
        [{ role: 'user', content: '输入字符' }]
      )) {
        deltas.push(delta)
      }

      // 验证未产生伪造的 usage 统计，完全依赖模型自身返回
      const usageDelta = deltas.find((d) => d.type === 'usage')
      expect(usageDelta).toBeUndefined()
    } finally {
      globalThis.fetch = originalFetch
    }
  })
})
