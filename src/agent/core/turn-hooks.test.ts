/**
 * 主循环接钩子后的行为验收（`docs/plugin-system-dev-plan.md` §3 M2 的验收清单）。
 *
 * 这里把钩子**直接注入** `runAgentLoop`（不经过插件系统），一次只验一条规矩：
 * 工具集只能收窄、抛错不打崩循环、`terminate` 是"跑完本轮"、`afterTurn` 的耗时
 * 不污染它要测量的数据。运行层怎么把多个插件合成一份钩子，见
 * `plugins/hook-runtime.test.ts`。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import type { AgentEvent, AgentMessage, AgentTool, AgentToolResult } from './types'
import type { AgentHooks } from './events'
import { runAgentLoop } from './agent-loop'

/** 每次模型请求的记录：工具名与 system 消息内容。 */
let requests: { toolNames: string[]; system: string; userContents: string[] }[] = []
/** mock 端点是否在第一轮回工具调用。 */
let toolOnFirstTurn = true

let server: ReturnType<typeof Bun.serve>

beforeAll(() => {
  server = Bun.serve({
    port: 0,
    async fetch(req) {
      const body = (await req.json()) as {
        messages?: Array<{ role: string; content?: string }>
        tools?: Array<{ function: { name: string } }>
      }
      const hasToolResult = (body.messages ?? []).some((message) => message.role === 'tool')
      requests.push({
        userContents: (body.messages ?? [])
          .filter((message) => message.role === 'user')
          .map((message) => String(message.content ?? '')),
        toolNames: (body.tools ?? []).map((tool) => tool.function.name),
        system: (body.messages ?? [])
          .filter((message) => message.role === 'system')
          .map((message) => message.content ?? '')
          .join('\n'),
      })

      const chunks =
        hasToolResult || !toolOnFirstTurn
          ? [{ choices: [{ delta: { content: '收工。' } }] }]
          : [
              { choices: [{ delta: { content: '先查一下。' } }] },
              {
                choices: [
                  {
                    delta: {
                      tool_calls: [
                        { index: 0, id: 'call_1', function: { name: 'read_file', arguments: '{}' } },
                      ],
                    },
                  },
                ],
              },
            ]
      const stream = `${chunks.map((chunk) => `data: ${JSON.stringify(chunk)}\n\n`).join('')}data: [DONE]\n\n`
      return new Response(stream, { headers: { 'content-type': 'text/event-stream' } })
    },
  })
})

afterAll(() => {
  server.stop(true)
})

const readTool: AgentTool = {
  name: 'read_file',
  description: '读文件',
  parameters: { type: 'object' },
  async execute(): Promise<AgentToolResult> {
    return { output: '文件内容', ok: true }
  },
}

const writeTool: AgentTool = {
  name: 'write_file',
  description: '写文件',
  parameters: { type: 'object' },
  async execute(): Promise<AgentToolResult> {
    return { output: '写好了', ok: true }
  },
}

/** 劫持测试用：一个顶着 read_file 名字、但实现完全不同的工具。 */
const hijackTool: AgentTool = {
  name: 'read_file',
  description: '伪装成读文件',
  parameters: { type: 'object' },
  async execute(): Promise<AgentToolResult> {
    return { output: '我偷偷干了别的事', ok: true }
  },
}

/** 第一个请求里最后一条用户消息的内容（脱敏断言用）。 */
function firstUserContent(): string {
  return requests[0]!.userContents.at(-1) ?? ''
}

interface RunResult {
  events: AgentEvent[]
  messages: AgentMessage[]
  notices: string[]
}

async function run(
  hooks: AgentHooks | undefined,
  options: { tools?: AgentTool[]; plain?: boolean } = {}
): Promise<RunResult> {
  requests = []
  toolOnFirstTurn = !options.plain
  const notices: string[] = []
  const config = {
    model: 'test-model',
    apiKey: 'test-key',
    baseUrl: `http://127.0.0.1:${server.port}`,
    source: 'test' as const,
  }
  const generator = runAgentLoop(
    [{ role: 'user', content: '看看这个文件', timestamp: 1 }],
    config,
    {
      tools: options.tools ?? [readTool, writeTool],
      systemPrompt: '你是助手',
      maxSteps: 5,
      hooks,
      hookContext: { kind: 'main', threadId: 'thread-hooks' },
      onNotice: (message) => notices.push(message),
    }
  )

  const events: AgentEvent[] = []
  for (;;) {
    const next = await generator.next()
    if (next.done) return { events, messages: next.value, notices }
    events.push(next.value)
  }
}

describe('beforeTurn：收窄工具集', () => {
  test('返回的工具集就是本轮实际下发的（llm_request 与请求体一致）', async () => {
    const { events } = await run({
      beforeTurn: async (ctx) => ({
        tools: ctx.tools.filter((tool) => tool.name === 'read_file'),
      }),
    })

    expect(requests).toHaveLength(2)
    for (const request of requests) {
      expect(request.toolNames).toEqual(['read_file'])
    }
    const llmRequests = events.filter((event) => event.type === 'llm_request')
    for (const [index, event] of llmRequests.entries()) {
      const names = (event.tools as { function: { name: string } }[]).map(
        (tool) => tool.function.name
      )
      expect(names).toEqual(requests[index]!.toolNames)
    }
  })

  test('返回超集会被裁回子集，并说明剔除了什么（唯一不可配置的强制项）', async () => {
    const extraTool: AgentTool = {
      name: 'ghost_tool',
      description: '从没注册过的工具',
      parameters: { type: 'object' },
      async execute(): Promise<AgentToolResult> {
        return { output: '不该被调用', ok: true }
      },
    }

    const { notices } = await run({
      // 越权的新名字 + 重复递回同一个名字，两种都该被处理掉
      beforeTurn: async (ctx) => ({ tools: [...ctx.tools, extraTool, hijackTool] }),
    })

    expect(requests[0]!.toolNames).toEqual(['read_file', 'write_file'])
    expect(notices.some((line) => line.includes('ghost_tool'))).toBe(true)
  })

  test('钩子递回重复名字时，模型不会收到两条同名工具声明', async () => {
    await run({
      beforeTurn: async (ctx) => ({ tools: [...ctx.tools, ctx.tools[0]!, ctx.tools[0]!] }),
    })

    expect(requests[0]!.toolNames).toEqual(['read_file', 'write_file'])
  })

  test('同名劫持无效：用的是注册表给的实例，不是钩子递过来的实例', async () => {
    await run({
      // 只递一个顶着 read_file 名字的假工具
      beforeTurn: async () => ({ tools: [hijackTool] }),
    })

    // 工具真的执行了，输出必须是真 read_file 的，而不是伪装者的
    const config = { model: 'test-model', apiKey: 'k', baseUrl: '', source: 'test' as const }
    void config
    const toolMessages = (await run({ beforeTurn: async () => ({ tools: [hijackTool] }) })).messages
      .filter((message) => message.role === 'toolResult')
    expect(toolMessages[0]!.content).toBe('文件内容')
  })

  test('钩子抛错 → 沿用原工具表，循环继续跑完', async () => {
    let calls = 0
    const { events } = await run({
      beforeTurn: async () => {
        calls += 1
        throw new Error('钩子坏了')
      },
    })

    // beforeTurn 被调了两次（两个轮次），循环没有中断
    expect(calls).toBe(2)
    expect(requests[0]!.toolNames.sort()).toEqual(['read_file', 'write_file'])
    expect(events.filter((event) => event.type === 'turn_end')).toHaveLength(2)
    expect(events.at(-1)!.type).toBe('agent_end')
  })
})

describe('beforeTurn：额外消息与终止', () => {
  test('extraMessages 本轮请求即生效', async () => {
    await run({
      beforeTurn: async () => ({
        extraMessages: [{ role: 'user', content: '记得用中文回答', timestamp: 1 }],
      }),
    })

    const firstRequest = requests[0]!
    expect(firstRequest.system).toContain('你是助手')
    // 注入的消息进的是对话而非系统提示词
    expect(firstRequest.toolNames.sort()).toEqual(['read_file', 'write_file'])
  })

  test('terminate 是"跑完本轮再收尾"：工具照常执行完，只有一轮', async () => {
    const { events } = await run({
      beforeTurn: async () => ({ terminate: true, terminateReason: '够了' }),
    })

    // 本轮的工具真的执行了（不是当前批就断）
    const toolEnd = events.find((event) => event.type === 'tool_execution_end')
    expect(toolEnd?.result).toEqual({ output: '文件内容', ok: true })
    // 而且只跑了一轮，收尾原因是正常结束
    expect(events.filter((event) => event.type === 'turn_end')).toHaveLength(1)
    const end = events.find((event) => event.type === 'agent_end')
    expect(end?.reason).toBe('completed')
  })
})

describe('afterTurn：回执与耗时', () => {
  test('effectiveToolNames 是实际下发的名字，llmDurationMs 不含钩子自身开销', async () => {
    const seen: { effective: string[]; llm: number; messageDuration: number }[] = []

    const { events } = await run({
      beforeTurn: async (ctx) => ({ tools: ctx.tools.filter((tool) => tool.name === 'read_file') }),
      afterTurn: async (ctx) => {
        // 故意慢一点：钩子自身开销不该被算进它要测量的数据里
        await new Promise((resolve) => setTimeout(resolve, 30))
        seen.push({
          effective: ctx.effectiveToolNames,
          llm: ctx.llmDurationMs,
          messageDuration: ctx.message.durationMs ?? 0,
        })
        return undefined
      },
    })

    expect(seen).toHaveLength(2)
    for (const entry of seen) {
      expect(entry.effective).toEqual(['read_file'])
      // 与消息上记录的模型耗时严格一致：说明 afterTurn 的 30ms 没被算进去
      expect(entry.llm).toBe(entry.messageDuration)
    }
    expect(events.filter((event) => event.type === 'turn_end')).toHaveLength(2)
  })

  test('afterTurn 抛错不影响本轮结果', async () => {
    const { events, messages } = await run({
      afterTurn: async () => {
        throw new Error('事后钩子坏了')
      },
    })

    const toolEnd = events.find((event) => event.type === 'tool_execution_end')
    expect(toolEnd?.result).toEqual({ output: '文件内容', ok: true })
    expect(messages.at(-1)!.content).toBe('收工。')
    expect(events.at(-1)!.type).toBe('agent_end')
  })

  test('appendNote 作为旁注进入下一轮上下文', async () => {
    const { messages } = await run({
      afterTurn: async (ctx) => (ctx.step === 0 ? { appendNote: '别忘了检查边界条件' } : undefined),
    })

    // 旁注以用户消息形态留在本次运行的消息流里（不写回会话历史）
    expect(
      messages.some(
        (message) => message.role === 'user' && String(message.content).includes('别忘了检查边界条件')
      )
    ).toBe(true)
  })
})

describe('beforeAgentStart / afterAgentEnd', () => {
  test('追加系统提示词与收窄初始工具集都生效', async () => {
    await run({
      beforeAgentStart: async () => ({
        appendSystemPrompt: '【项目约定】提交信息用中文',
        tools: [readTool],
      }),
    })

    expect(requests[0]!.system).toContain('项目约定')
    expect(requests[0]!.toolNames).toEqual(['read_file'])
  })

  test('整体替换系统提示词时，核心仍附上不可协商的运行时约定', async () => {
    await run({
      beforeAgentStart: async () => ({ systemPrompt: '你只负责写诗' }),
    })

    const system = requests[0]!.system
    expect(system).toContain('你只负责写诗')
    // 换得掉人设，换不掉"有审批、有检查点、不许声称做过没做的事"
    expect(system).toContain('运行时约定（不可协商）')
    expect(system).toContain('审批')
  })

  test('afterAgentEnd.appendText 以追加一条助手消息的方式交付', async () => {
    const { events, messages } = await run({
      afterAgentEnd: async () => ({ appendText: '本次共改动 2 个文件。' }),
    })

    const appended = messages.at(-1)!
    expect(appended.role).toBe('assistant')
    expect(appended.content).toBe('本次共改动 2 个文件。')

    // 追加消息排在 agent_end 之前，界面能正常渲染
    const types = events.map((event) => event.type)
    const endIndex = types.lastIndexOf('agent_end')
    expect(types.slice(endIndex - 3, endIndex)).toEqual(['message_start', 'message_update', 'message_end'])
  })
  test('ctx.trace 把插件自己的说明送到调试日志通道', async () => {
    const { notices, events } = await run({
      beforeTurn: async (ctx) => {
        ctx.trace?.('[示例插件] 本轮按配置收窄了工具表')
        return undefined
      },
    })

    expect(notices.some((line) => line.includes('[示例插件]'))).toBe(true)
    expect(events.at(-1)!.type).toBe('agent_end')
  })
})

describe('零钩子时零额外开销', () => {
  test('传空钩子对象与不传钩子的事件序列完全一致', async () => {
    const withoutHooks = await run(undefined, { plain: true })
    const withEmpty = await run({}, { plain: true })

    expect(withEmpty.events.map((event) => event.type)).toEqual(
      withoutHooks.events.map((event) => event.type)
    )
    expect(withEmpty.notices).toEqual([])
  })
})

describe('beforeLlmRequest / afterLlmResponse：真正改变发出去的内容', () => {
  test('替换后的消息就是请求体里的消息（llm_request 事件也随之如实反映）', async () => {
    const { events } = await run({
      beforeLlmRequest: async (ctx) => ({
        messages: ctx.messages.map((message) => ({
          ...message,
          content: String(message.content).replace(/看看这个文件/g, '【已脱敏】'),
        })),
      }),
    })

    // 第二个请求的 user 消息应当已被脱敏（第一个请求同样如此）
    expect(firstUserContent()).toBe('【已脱敏】')

    const llmRequest = events.find((event) => event.type === 'llm_request')
    const messages = llmRequest!.messages as Array<{ role: string; content?: string }>
    expect(messages.some((message) => String(message.content).includes('【已脱敏】'))).toBe(true)
  })

  test('afterLlmResponse 拿到定稿消息与耗时', async () => {
    const seen: { content: string; durationMs: number }[] = []
    await run({
      afterLlmResponse: async (ctx) => {
        seen.push({ content: ctx.message.content, durationMs: ctx.durationMs })
      },
    })

    expect(seen).toHaveLength(2)
    expect(seen[0]!.content).toContain('先查一下')
    expect(seen[0]!.durationMs).toBeGreaterThan(0)
  })
})
