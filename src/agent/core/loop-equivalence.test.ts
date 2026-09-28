/**
 * 主循环的**逐事件行为基线**（设计文档 §6.2 改造点 1）。
 *
 * `runAgentLoop` 里 `toolMap`/`toolSpecs` 原本在循环外算一次，M2 要挪进循环内每轮算
 * （插件需要按轮次干预工具表）。这是这个文件里唯一的破坏性重构，所以先在这里钉住
 * "改动前每一条事件的顺序、载荷与模型请求次数"：
 *
 * 1. **完整事件序列**逐条对照——顺序变了就是行为变了；
 * 2. 每轮 `llm_request.tools` 的工具名集合与**引用复用**（同一份 specs 对象）；
 * 3. 模型请求次数、每轮的 messages 条数（历史增长的节奏）；
 * 4. 工具结果消息的内容与顺序。
 *
 * 这些断言是在**重构前**跑通并把实际值抄下来的，不是照着实现反推的期望值。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import type { AgentEvent, AgentMessage, AgentTool, AgentToolResult } from './types'
import { runAgentLoop } from './agent-loop'

let server: ReturnType<typeof Bun.serve>
/** 每次模型请求的记录：工具名 + 请求体里的消息角色序列。 */
let requests: { toolNames: string[]; roles: string[]; toolsRef: unknown }[] = []

beforeAll(() => {
  requests = []
  server = Bun.serve({
    port: 0,
    async fetch(req) {
      const body = (await req.json()) as {
        messages?: Array<{ role: string }>
        tools?: Array<{ function: { name: string } }>
      }
      requests.push({
        toolNames: (body.tools ?? []).map((tool) => tool.function.name),
        roles: (body.messages ?? []).map((message) => message.role),
        toolsRef: body.tools,
      })

      const hasToolResult = (body.messages ?? []).some((message) => message.role === 'tool')
      const chunks = hasToolResult
        ? [
            { choices: [{ delta: { content: '标记完成。' } }] },
            { usage: { prompt_tokens: 20, completion_tokens: 4, total_tokens: 24 } },
          ]
        : [
            { choices: [{ delta: { content: '我来加一个标记。' } }] },
            {
              choices: [
                {
                  delta: {
                    tool_calls: [
                      {
                        index: 0,
                        id: 'call_1',
                        function: { name: 'mark', arguments: '{"label":"alpha"}' },
                      },
                    ],
                  },
                },
              ],
            },
            { usage: { prompt_tokens: 10, completion_tokens: 6, total_tokens: 16 } },
          ]

      const stream = `${chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).join('')}data: [DONE]\n\n`
      return new Response(stream, { headers: { 'content-type': 'text/event-stream' } })
    },
  })
})

afterAll(() => {
  server.stop(true)
})

const markTool: AgentTool = {
  name: 'mark',
  description: '打一个标记',
  parameters: { type: 'object' },
  async execute(_callId, args): Promise<AgentToolResult> {
    return { output: `已标记 ${args.label as string}`, ok: true }
  },
}

/** 跑完一整轮，收齐全部事件与最终消息。 */
async function runOnce(): Promise<{ events: AgentEvent[]; messages: AgentMessage[] }> {
  requests = []
  const config = {
    model: 'test-model',
    apiKey: 'test-key',
    baseUrl: `http://127.0.0.1:${server.port}`,
    source: 'test' as const,
  }
  const generator = runAgentLoop(
    [{ role: 'user', content: '帮我加一个标记', timestamp: 1 }],
    config,
    { tools: [markTool], systemPrompt: '你是助手', maxSteps: 5 }
  )

  const events: AgentEvent[] = []
  for (;;) {
    const next = await generator.next()
    if (next.done) return { events, messages: next.value }
    events.push(next.value)
  }
}

describe('主循环逐事件基线（M2-2 重构不得改变它）', () => {
  test('事件序列与轮次结构逐条一致', async () => {
    const { events } = await runOnce()

    expect(events.map((event) => event.type)).toEqual([
      'agent_start',
      'turn_start',
      'message_start',
      'llm_request',
      'message_update', // 文本增量
      'message_update', // 工具调用增量
      'message_update', // usage
      'message_end',
      'llm_response',
      'tool_execution_start',
      'tool_execution_end',
      'message_start', // 工具结果消息
      'message_end',
      'turn_end',
      'turn_start',
      'message_start',
      'llm_request',
      'message_update', // 文本增量
      'message_update', // usage
      'message_end',
      'llm_response',
      'turn_end',
      'agent_end',
    ])

    // 只有第一轮带工具调用（第二轮是纯文本轮，据此收尾）
    const turnEnds = events.filter((event) => event.type === 'turn_end')
    expect(turnEnds).toHaveLength(2)
    expect(turnEnds[0]!.toolResults).toHaveLength(1)
    expect(turnEnds[1]!.toolResults).toHaveLength(0)
  })

  test('模型请求的次数、工具表与历史长度逐轮一致', async () => {
    await runOnce()

    expect(requests).toHaveLength(2)
    // 两轮都下发同一张工具表
    expect(requests[0]!.toolNames).toEqual(['mark'])
    expect(requests[1]!.toolNames).toEqual(['mark'])
    // 历史增长的节奏：第二轮多了 assistant(带工具调用) 与 tool 两条
    expect(requests[0]!.roles).toEqual(['system', 'user'])
    expect(requests[1]!.roles).toEqual(['system', 'user', 'assistant', 'tool'])
  })

  test('llm_request 事件里的工具表与实际下发的完全相同', async () => {
    const { events } = await runOnce()

    const llmRequests = events.filter((event) => event.type === 'llm_request')
    expect(llmRequests).toHaveLength(2)
    for (const [index, event] of llmRequests.entries()) {
      const names = (event.tools as { function: { name: string } }[]).map(
        (tool) => tool.function.name
      )
      expect(names).toEqual(requests[index]!.toolNames)
    }
  })

  test('工具集未变时复用同一份 specs 引用（提示缓存友好，也是 applyTools 缓存的回归位）', async () => {
    const { events } = await runOnce()

    const llmRequests = events.filter((event) => event.type === 'llm_request')
    expect(llmRequests[0]!.tools).toBe(llmRequests[1]!.tools)
  })

  test('工具结果消息的内容与顺序不变', async () => {
    const { events, messages } = await runOnce()

    const toolEnd = events.find((event) => event.type === 'tool_execution_end')
    expect(toolEnd?.result).toEqual({ output: '已标记 alpha', ok: true })

    const toolMessages = messages.filter((message) => message.role === 'toolResult')
    expect(toolMessages).toHaveLength(1)
    expect(toolMessages[0]!.content).toBe('已标记 alpha')
    // 最后一条是第二轮定稿的助手回复
    expect(messages.at(-1)!.role).toBe('assistant')
    expect(messages.at(-1)!.content).toBe('标记完成。')
  })

  test('agent_end 汇总结束原因与完整消息', async () => {
    const { events, messages } = await runOnce()

    const end = events.find((event) => event.type === 'agent_end')
    expect(end?.reason).toBe('completed')
    expect(end?.messages).toHaveLength(messages.length)
  })
})
