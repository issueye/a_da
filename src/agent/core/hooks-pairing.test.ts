/**
 * 成对性：每个 `before*` 都必须有配对的 `after*`（设计文档 §6.0、§10）。
 *
 * 为什么值得单独一个文件：成对是本层的核心承诺——少了 `after`，插件既无法知道自己
 * "是不是真的生效了"，也没有清理时机。而**漏配是最难发现的一类缺陷**：代码照跑，
 * 只是插件在"以为自己生效"的状态下工作。
 *
 * 两道防线：
 * 1. **编译期**：`HOOK_KEYS` 是 `Record<keyof AgentHooks, true>`，往契约里加钩子却
 *    忘了登记，typecheck 直接红；
 * 2. **运行期**：下面的用例断言每个 before 都真能在循环里等到它的 after——包括
 *    `before*` 已经被短路的情况（§6.4.4.1）。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import type { AgentEndReason, AgentMessage, AgentTool, AgentToolResult } from './types'
import type { AgentHooks } from './events'
import { HOOK_PAIRS, UNPAIRED_HOOKS } from './events'
import { runAgentLoop } from './agent-loop'

/**
 * 契约里的全部钩子点位。
 *
 * 这个对象字面量是**编译期的成对守门**：`AgentHooks` 新增点位而这里没跟上，
 * `Record<keyof AgentHooks, true>` 会让 typecheck 报缺字段。
 */
const HOOK_KEYS: Record<keyof AgentHooks, true> = {
  beforeAgentStart: true,
  afterAgentEnd: true,
  beforeTurn: true,
  afterTurn: true,
  beforeToolCall: true,
  afterToolCall: true,
  beforeSubagentStart: true,
  afterSubagentEnd: true,
  beforeApproval: true,
  afterApproval: true,
  beforeCompaction: true,
  afterCompaction: true,
  beforeThreadCreate: true,
  afterThreadCreate: true,
  beforeThreadDelete: true,
  afterThreadDelete: true,
  onThreadSwitch: true,
  beforeLlmRequest: true,
  afterLlmResponse: true,
  beforeSystemPrompt: true,
  beforeSkillLoad: true,
  afterSkillLoad: true,
  beforePersist: true,
  afterCheckpoint: true,
}

let server: ReturnType<typeof Bun.serve>

beforeAll(() => {
  server = Bun.serve({
    port: 0,
    async fetch(req) {
      const body = (await req.json()) as { messages?: Array<{ role: string }> }
      const hasToolResult = (body.messages ?? []).some((message) => message.role === 'tool')
      const chunks = hasToolResult
        ? [{ choices: [{ delta: { content: '好了。' } }] }]
        : [
            { choices: [{ delta: { content: '查一下。' } }] },
            {
              choices: [
                {
                  delta: {
                    tool_calls: [
                      { index: 0, id: 'call_1', function: { name: 'ping', arguments: '{}' } },
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

const pingTool: AgentTool = {
  name: 'ping',
  description: '回声工具',
  parameters: { type: 'object' },
  async execute(): Promise<AgentToolResult> {
    return { output: 'pong', ok: true }
  },
}

/** 跑一轮：`toolTurns` 决定这个 mock 端点给不给工具调用。 */
async function run(hooks: AgentHooks, options: { maxSteps?: number; plain?: boolean } = {}): Promise<{
  events: string[]
  messages: AgentMessage[]
}> {
  const config = {
    model: 'test-model',
    apiKey: 'test-key',
    // plain 场景用一个只回文本的端点：走的是 `turn_end` 的**另一个出口**
    baseUrl: `http://127.0.0.1:${options.plain ? textOnlyServer().port : server.port}`,
    source: 'test' as const,
  }
  const generator = runAgentLoop(
    [{ role: 'user', content: '打个招呼', timestamp: 1 }],
    config,
    {
      tools: [pingTool],
      systemPrompt: '你是助手',
      maxSteps: options.maxSteps ?? 5,
      hooks,
      hookContext: { kind: 'main', threadId: 'thread-1' },
    }
  )
  const events: string[] = []
  for (;;) {
    const next = await generator.next()
    if (next.done) return { events, messages: next.value }
    events.push(next.value.type)
  }
}

/** 一个只回纯文本的端点，用来命中"没有工具调用"的那个 turn_end 出口。 */
let plainServer: ReturnType<typeof Bun.serve> | undefined
function textOnlyServer(): ReturnType<typeof Bun.serve> {
  if (plainServer) return plainServer
  plainServer = Bun.serve({
    port: 0,
    async fetch() {
      const stream = `data: ${JSON.stringify({ choices: [{ delta: { content: '纯文本作答。' } }] })}\n\ndata: [DONE]\n\n`
      return new Response(stream, { headers: { 'content-type': 'text/event-stream' } })
    },
  })
  return plainServer
}

afterAll(() => {
  plainServer?.stop(true)
})

describe('成对性：契约层面', () => {
  test('配对表覆盖契约里的每一个点位（含刻意不成对的），方向正确且没有自配对', () => {
    const registered = Object.keys(HOOK_KEYS)
    const paired = HOOK_PAIRS.flatMap((pair) => [pair.before, pair.after])

    // 契约里的点位 = 成对的 ∪ 刻意不成对的（纯判定 / 纯通知），不允许有漏网的
    expect(registered.sort()).toEqual([...new Set([...paired, ...UNPAIRED_HOOKS])].sort())
    expect(HOOK_PAIRS).toHaveLength((registered.length - UNPAIRED_HOOKS.length) / 2)
    for (const pair of HOOK_PAIRS) {
      expect(String(pair.before).startsWith('before')).toBe(true)
      expect(String(pair.after).startsWith('after')).toBe(true)
      expect(pair.before).not.toBe(pair.after)
    }
  })
})

describe('成对性：循环里真的配得起来', () => {
  test('带工具的轮：beforeTurn/afterTurn 各一次，afterTurn 拿到工具结果', async () => {
    const calls: { before: number; after: number; results: number[] } = {
      before: 0,
      after: 0,
      results: [],
    }
    await run({
      beforeTurn: async () => {
        calls.before += 1
        return undefined
      },
      afterTurn: async (ctx) => {
        calls.after += 1
        calls.results.push(ctx.toolResults.length)
        return undefined
      },
    })

    expect(calls.before).toBe(2) // 工具轮 + 收尾的纯文本轮
    expect(calls.after).toBe(2)
    expect(calls.results).toEqual([1, 0])
  })

  test('纯文本轮同样触发 afterTurn（两个 turn_end 出口都不能漏）', async () => {
    let after = 0
    let toolResultsSeen = -1
    let effective: string[] = []

    await run(
      {
        afterTurn: async (ctx) => {
          after += 1
          toolResultsSeen = ctx.toolResults.length
          effective = ctx.effectiveToolNames
          return undefined
        },
      },
      { plain: true }
    )

    expect(after).toBe(1)
    expect(toolResultsSeen).toBe(0)
    // 回执：实际下发给模型的工具名，而不是任何插件的意图
    expect(effective).toEqual(['ping'])
  })

  test('beforeTurn 要求终止时，afterTurn **照常执行**（事前被短路，事后仍要收尾）', async () => {
    const order: string[] = []
    await run({
      beforeTurn: async () => {
        order.push('before')
        // `terminateBy` 平时由钩子运行层填写（它才知道是哪个插件要求终止），
        // 这里手工注入，验证循环会把这个字段原样带到 afterTurn 的上下文里
        return { terminate: true, terminateReason: '够了', terminateBy: 'plugin-x' }
      },
      afterTurn: async (ctx) => {
        order.push('after')
        // 事前终止的原因要能被事后看到，否则插件无法解释"为什么只有一轮"
        expect(ctx.terminatedByHook).toBe('plugin-x')
        return undefined
      },
    })

    // 终止在**本轮跑完之后**生效：钩子只跑了一次（第一轮），但成对仍然完整
    expect(order).toEqual(['before', 'after'])
  })

  test('afterAgentEnd 与 beforeAgentStart 成对：正常收尾时都会执行', async () => {
    const seen: { start: number; end: number; reason?: AgentEndReason; steps: number } = {
      start: 0,
      end: 0,
      steps: -1,
    }
    await run({
      beforeAgentStart: async () => {
        seen.start += 1
        return undefined
      },
      afterAgentEnd: async (ctx) => {
        seen.end += 1
        seen.reason = ctx.reason
        seen.steps = ctx.stepsExecuted
        return undefined
      },
    })

    expect(seen.start).toBe(1)
    expect(seen.end).toBe(1)
    expect(seen.reason).toBe('completed')
    expect(seen.steps).toBe(2)
  })

  test('afterAgentEnd 在撞上步数上限时也执行（成对不依赖正常收尾）', async () => {
    let end = 0
    let reason: AgentEndReason | undefined
    await run(
      {
        afterAgentEnd: async (ctx) => {
          end += 1
          reason = ctx.reason
          return undefined
        },
      },
      { maxSteps: 1 }
    )

    expect(end).toBe(1)
    expect(reason).toBe('max_steps')
  })
})

describe('成对性：子智能体门禁与结束复核', () => {
  test('门禁只在配了 criteria 时才跑；没有门禁时 afterSubagentEnd 仍可独立工作', async () => {
    // 门禁的成对性由 subagents/access.ts 的判定表 + 下面这两条语义钉住：
    // beforeSubagentStart 是纯判定点位（没有标准就没有要判的事），
    // afterSubagentEnd 是生命周期收尾（每次结束都该有机会被看到）。
    const { runSubagentGate } = await import('../subagents/access')
    const profile = {
      id: 'probe',
      name: '探针',
      description: '',
      systemPrompt: '',
      allowedTools: ['*'],
      mode: 'readwrite' as const,
      enabled: true,
      scope: 'builtin' as const,
    }

    // 没配 gate → 不跑判定，返回 undefined（不是失败）
    expect(await runSubagentGate({ profile, task: 't', authorizedTools: [] })).toBeUndefined()

    // 配了 gate 但没有插件提供判定能力 → 放行 + 提示"门禁未生效"
    const notices: string[] = []
    const outcome = await runSubagentGate({
      profile: { ...profile, gate: { criteria: '必须通过测试' } },
      task: 't',
      authorizedTools: [],
      notice: (message) => notices.push(message),
    })
    expect(outcome?.allowed).toBe(true)
    expect(outcome?.judged).toBe(false)
    expect(notices.some((line) => line.includes('门禁未生效'))).toBe(true)
  })
})
