import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { Agent } from './agent'
import { runAgentLoop } from './agent-loop'
import type { AgentEvent, AgentTool, AgentToolResult } from './types'

let server: ReturnType<typeof Bun.serve>

beforeAll(() => {
  server = Bun.serve({
    port: 0,
    async fetch() {
      // 模拟一个带工具调用的 SSE 响应
      const stream = [
        `data: ${JSON.stringify({ choices: [{ delta: { content: '我来计算一下。' } }] })}\n\n`,
        `data: ${JSON.stringify({
          choices: [
            {
              delta: {
                tool_calls: [
                  { index: 0, id: 'call_test_1', function: { name: 'calc', arguments: '{"n":5}' } },
                ],
              },
            },
          ],
        })}\n\n`,
        `data: [DONE]\n\n`,
      ].join('')

      return new Response(stream, { headers: { 'content-type': 'text/event-stream' } })
    },
  })
})

afterAll(() => {
  server.stop(true)
})

describe('Agent Core Runtime', () => {
  test('emits lifecycle events and executes tools', async () => {
    let toolExecuted = false

    const mockTool: AgentTool = {
      name: 'calc',
      description: '计算工具',
      parameters: { type: 'object' },
      async execute(_callId, args): Promise<AgentToolResult> {
        toolExecuted = true
        return {
          output: `结果是: ${(args.n as number) * 2}`,
          ok: true,
        }
      },
    }

    const events: string[] = []
    const agent = new Agent({
      tools: [mockTool],
    })

    agent.subscribe((event) => {
      events.push(event.type)
    })

    const config = {
      model: 'test-model',
      apiKey: 'test-key',
      baseUrl: `http://127.0.0.1:${server.port}`,
      source: 'test' as const,
    }

    await agent.prompt('帮我计算 5', config)

    expect(events).toContain('agent_start')
    expect(events).toContain('turn_start')
    expect(events).toContain('message_start')
    expect(events).toContain('message_update')
    expect(events).toContain('tool_execution_start')
    expect(events).toContain('tool_execution_end')
    expect(events).toContain('turn_end')
    expect(events).toContain('agent_end')

    expect(toolExecuted).toBe(true)
    expect(agent.isStreaming).toBe(false)
  })

  test('beforeToolCall hook can block execution', async () => {
    const mockTool: AgentTool = {
      name: 'calc',
      description: '计算工具',
      parameters: { type: 'object' },
      async execute(): Promise<AgentToolResult> {
        return { output: '10', ok: true }
      },
    }

    const agent = new Agent({
      tools: [mockTool],
      beforeToolCall: async (ctx) => {
        if (ctx.toolCall.name === 'calc') {
          return { block: true, reason: '安全策略禁止计算', terminate: true }
        }
      },
    })

    const config = {
      model: 'test-model',
      apiKey: 'test-key',
      baseUrl: `http://127.0.0.1:${server.port}`,
      source: 'test' as const,
    }

    const messages = await agent.prompt('帮我计算', config)
    const toolRes = messages.find((m) => m.role === 'toolResult')
    expect(toolRes).toBeDefined()
    expect(toolRes?.content).toContain('安全策略禁止计算')
  })
})

/**
 * 工具事件的实时性。
 *
 * 盯的是一个很容易悄悄退化的性质：回调推来的增量必须在工具还在跑的时候就送达。
 * 攒到执行结束再一起补发的写法事件一样齐全，界面却是死的——所以这里比的是
 * 「什么时候看到」，不是「看到了几个」。
 */
describe('tool event streaming', () => {
  async function serveToolCalls(
    calls: { id: string; name: string }[]
  ): Promise<{ baseUrl: string; stop: () => void }> {
    const chunks = [
      ...calls.map((call, index) => ({
        choices: [
          {
            delta: {
              tool_calls: [
                { index, id: call.id, function: { name: call.name, arguments: '{}' } },
              ],
            },
          },
        ],
      })),
      { choices: [{ delta: { content: '好' } }] },
    ]
    const stream = `${chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).join('')}data: [DONE]\n\n`
    const local = Bun.serve({
      port: 0,
      fetch: () => new Response(stream, { headers: { 'content-type': 'text/event-stream' } }),
    })
    return { baseUrl: `http://127.0.0.1:${local.port}`, stop: () => local.stop(true) }
  }

  const config = (baseUrl: string) => ({
    model: 'test-model',
    apiKey: 'test-key',
    baseUrl,
    source: 'test' as const,
  })

  test('partial output reaches the consumer while the tool is still running', async () => {
    const { baseUrl, stop } = await serveToolCalls([{ id: 'call_1', name: 'slow' }])
    let settledAt = 0
    let updateSeenAt = 0

    const slowTool: AgentTool = {
      name: 'slow',
      description: '分两段输出的工具',
      parameters: { type: 'object' },
      async execute(_callId, _args, _signal, onUpdate): Promise<AgentToolResult> {
        onUpdate?.({ output: '第一段', ok: true })
        await new Promise((resolve) => setTimeout(resolve, 60))
        settledAt = Date.now()
        return { output: '第一段第二段', ok: true }
      },
    }

    try {
      for await (const event of runAgentLoop([{ role: 'user', content: '跑一下' }], config(baseUrl), {
        tools: [slowTool],
        maxSteps: 1,
        toolExecution: 'sequential',
      })) {
        if (event.type === 'tool_execution_update') updateSeenAt = Date.now()
      }
    } finally {
      stop()
    }

    expect(updateSeenAt).toBeGreaterThan(0)
    expect(settledAt).toBeGreaterThan(0)
    expect(updateSeenAt).toBeLessThan(settledAt)
  })

  test('parallel mode really overlaps two tools', async () => {
    const { baseUrl, stop } = await serveToolCalls([
      { id: 'call_a', name: 'a' },
      { id: 'call_b', name: 'b' },
    ])
    const order: string[] = []
    const slow = (name: string): AgentTool => ({
      name,
      description: name,
      parameters: { type: 'object' },
      async execute(): Promise<AgentToolResult> {
        order.push(`${name}:start`)
        await new Promise((resolve) => setTimeout(resolve, 80))
        order.push(`${name}:end`)
        return { output: name, ok: true }
      },
    })

    const events: AgentEvent[] = []
    try {
      for await (const event of runAgentLoop([{ role: 'user', content: '两个一起跑' }], config(baseUrl), {
        tools: [slow('a'), slow('b')],
        maxSteps: 1,
        toolExecution: 'parallel',
      })) {
        events.push(event)
      }
    } finally {
      stop()
    }

    // 交错即并发：生成器要各自有驱动任务才会同时推进，光靠 Promise.all 不会。
    expect(order).toEqual(['a:start', 'b:start', 'a:end', 'b:end'])
    expect(events.filter((event) => event.type === 'tool_execution_start')).toHaveLength(2)
    expect(events.filter((event) => event.type === 'tool_execution_end')).toHaveLength(2)
  })
})
