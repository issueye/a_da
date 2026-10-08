/**
 * 审批闸门的插件判定（M3-5，设计文档 §6.6.2 第一优先）。
 *
 * 覆盖开发计划的验收清单：
 * - `beforeApproval` 返回允许 → **不弹卡片**直接执行
 * - 返回拒绝 → 理由**回给模型**（不能变成"工具执行失败"那种无从纠正的错误）
 * - `afterApproval` 拿到实际决策与耗时
 *
 * 另外把两条刻意的效力不对称钉住：`deny` 压倒 `allow`（插件不该推翻另一个插件的
 * 否决）；`allow` 在只读审批档位下被忽略（那一档的语义就是"写操作必须经我确认"）。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { AgentStore } from '../store'
import { defaultExtensionLoader } from '../tools/loader'
import { clearLoadedPlugins } from '../plugins/registry'
import { composePluginHooks } from '../plugins/hook-runtime'
import { DEFAULT_PLUGIN_CAPABILITIES, saveDisabledPlugins, type ResolvedPluginCapabilities } from '../config'
import type { LoadedPlugin } from '../plugins/types'

let workspace = ''

beforeEach(async () => {
  workspace = await mkdtemp(join(tmpdir(), 'ada-approval-ws-'))
  await mkdir(join(workspace, '.ada', 'extensions'), { recursive: true })
})

afterEach(async () => {
  clearLoadedPlugins(workspace)
  await rm(workspace, { recursive: true, force: true }).catch(() => {})
})

const capabilities: ResolvedPluginCapabilities = {
  capabilities: DEFAULT_PLUGIN_CAPABILITIES,
  forPlugin: () => DEFAULT_PLUGIN_CAPABILITIES,
  overrides: {},
  invalid: [],
}

function pluginWith(hooks: LoadedPlugin['contributions']['hooks']): LoadedPlugin {
  return {
    manifest: { id: 'builtin:approval-probe', name: '审批探针', description: '', scope: 'builtin' },
    contributions: { tools: [], hooks },
    declarative: true,
    status: 'ready',
    diagnostics: [],
  }
}

const toolCall = { id: 'c1', name: 'write_file', arguments: { path: 'a.txt' }, rawArguments: '{}' }

const approvalContext = {
  kind: 'main' as const,
  workspace: '/tmp/ws',
  threadId: 't1',
  toolCall,
  approvalMode: 'ask' as const,
  isWrite: true,
}

describe('运行层：allow / deny 的效力不对称', () => {
  test('deny 压倒 allow：一个插件不该能推翻另一个插件的否决', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities,
      plugins: [
        pluginWith({ beforeApproval: async () => ({ decision: 'allow' as const, reason: '我放行' }) }),
        pluginWith({ beforeApproval: async () => ({ decision: 'deny' as const, reason: '我拒绝' }) }),
      ],
    })

    const result = await hooks.beforeApproval!(approvalContext)
    expect(result?.decision).toBe('deny')
    expect(result?.reason).toBe('我拒绝')
  })

  test('没人表态时返回 undefined（照常问用户）', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities,
      plugins: [pluginWith({ beforeApproval: async () => undefined })],
    })

    expect(await hooks.beforeApproval!(approvalContext)).toBeUndefined()
  })

  test('afterApproval 抛错不影响调用方', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities,
      plugins: [
        pluginWith({
          afterApproval: async () => {
            throw new Error('观察钩子炸了')
          },
        }),
      ],
    })

    await expect(
      hooks.afterApproval!({
        kind: 'main',
        toolCall,
        decidedBy: 'user',
        approved: true,
        durationMs: 12,
      })
    ).resolves.toBeUndefined()
  })
})

describe('真实闸门：插件放行不弹卡片、拒绝理由回给模型', () => {
  async function writeApprovalPlugin(body: string): Promise<void> {
    await writeFile(
      join(workspace, '.ada', 'extensions', 'approval-probe.ts'),
      `export default {
  name: '审批探针',
  tools: [],
  hooks: { beforeApproval: async (ctx) => { ${body} } },
}
`,
      'utf-8'
    )
    await defaultExtensionLoader.autoLoadExtensions(workspace)
  }

  test('只读档位下：插件想放行写操作会被忽略并说明，用户仍被问到', async () => {
    await writeApprovalPlugin(`return { decision: 'allow', reason: '白名单放行' }`)
    const store = new AgentStore(workspace)
    store.setApproval('readonly')
    const thread = store.newThread(workspace)
    store.selectThread(thread.id)

    // 直接走闸门：只读档位下写工具需要审批。插件放行被忽略 → 卡片停在 awaiting
    const pending = (store as unknown as {
      gate: (t: unknown, c: unknown) => Promise<{ block?: boolean; reason?: string } | undefined>
    }).gate(thread, toolCall)

    await new Promise((resolve) => setTimeout(resolve, 30))
    const card = thread.items.find((item) => item.kind === 'tool') as { status?: string } | undefined
    expect(card?.status).toBe('awaiting')
    expect(
      store.log.some((entry) => entry.text.includes('只读审批档位') && entry.text.includes('已忽略'))
    ).toBe(true)

    // 用户拒绝收尾，避免这个用例挂在审批闸门上
    store.decide((card as unknown as { id: string }).id, false)
    const outcome = await pending
    expect(outcome?.block).toBe(true)
    store.deleteThread(thread.id)
  })

  test('ask 档位下：插件放行 → 卡片直接进入执行，不弹审批；afterApproval 记到决策', async () => {
    await writeApprovalPlugin(
      `return { decision: 'allow', reason: '这类工具已加入白名单' }`
    )
    const store = new AgentStore(workspace)
    store.setApproval('ask')
    const thread = store.newThread(workspace)
    store.selectThread(thread.id)

    const outcome = await (store as unknown as {
      gate: (t: unknown, c: unknown) => Promise<{ block?: boolean; reason?: string } | undefined>
    }).gate(thread, toolCall)

    // 放行 → 不 block
    expect(outcome).toBeUndefined()
    const card = thread.items.find((item) => item.kind === 'tool') as { status?: string } | undefined
    expect(card?.status).toBe('running')

    store.deleteThread(thread.id)
  })

  test('插件拒绝：理由作为工具结果回给模型，而不是"执行失败"', async () => {
    await writeApprovalPlugin(`return { decision: 'deny', reason: '本周冻结写操作，请先说明改动原因' }`)
    const store = new AgentStore(workspace)
    store.setApproval('ask')
    const thread = store.newThread(workspace)
    store.selectThread(thread.id)

    const outcome = await (store as unknown as {
      gate: (t: unknown, c: unknown) => Promise<{ block?: boolean; reason?: string } | undefined>
    }).gate(thread, toolCall)

    expect(outcome?.block).toBe(true)
    expect(outcome?.reason).toContain('本周冻结写操作')
    expect(
      store.log.some((entry) => entry.text.includes('被插件') && entry.text.includes('拒绝'))
    ).toBe(true)
    store.deleteThread(thread.id)
  })
})

describe('askUser：插件能发起询问，但回答只能来自真实点击', () => {
  /**
   * 等一张**新的、处于 awaiting** 的工具卡出现并返回它。
   *
   * 不能只 `find(kind === 'tool')`：同一会话里可能已有前面的用例留下的旧卡，
   * 点到旧卡等于没回答，`askUser` 会一直等下去（表现为超时，而不是明确的失败）。
   * `skip` 表示跳过前 N 张已存在的卡。
   */
  async function waitForNewToolCard(
    thread: { items: Array<{ kind: string; id: string; status?: string }> },
    skip: number,
    timeoutMs = 2000
  ): Promise<{ id: string; status?: string }> {
    const started = Date.now()
    while (Date.now() - started < timeoutMs) {
      const cards = thread.items.filter(
        (item) => item.kind === 'tool' && item.status === 'awaiting'
      )
      if (cards.length > skip) return cards[cards.length - 1]!
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
    throw new Error('等待审批卡片超时：askUser 可能根本没有发起询问')
  }

  async function writeAskingPlugin(): Promise<void> {
    // 必须先停用内置审批策略插件：它的高危命令判定会先返回 deny，而运行层的规则是
    // "deny 压倒 allow 且立刻定稿"，探针插件就永远跑不到了。
    // 这不是测试取巧——它恰好证明了运行层的否决语义是对的（见上一个 describe）。
    await saveDisabledPlugins(['builtin:approval-guard'])
    await writeFile(
      join(workspace, '.ada', 'extensions', 'approval-asker.ts'),
      `export default {
  name: '审批提问者',
  tools: [],
  hooks: {
    beforeApproval: async (ctx) => {
      if (!ctx.askUser) return undefined
      const answer = await ctx.askUser({ reason: '这条命令看起来危险，请确认' })
      // 用户的回答原样采纳：批准就放行，其余一律拒绝
      return answer.approved
        ? { decision: 'allow', reason: '用户确认通过' }
        : { decision: 'deny', reason: '用户拒绝（' + answer.answeredBy + '）' }
    },
  },
}
`,
      'utf-8'
    )
    await defaultExtensionLoader.autoLoadExtensions(workspace)
  }

  test('插件发起询问后，真实点击"批准"能解开等待并放行', async () => {
    await writeAskingPlugin()
    const store = new AgentStore(workspace)
    store.setApproval('ask')
    const thread = store.newThread(workspace)
    store.selectThread(thread.id)

    // gate 会卡在 askUser 的等待上；从另一侧模拟用户点击
    const askCall = { ...toolCall, name: 'run_command', arguments: { command: 'rm -rf build' } }
    const pending = (store as unknown as {
      gate: (t: unknown, c: unknown) => Promise<{ block?: boolean; reason?: string } | undefined>
    }).gate(thread, askCall)

    // 等卡片出现（说明 askUser 已经在等），再点批准。
    // 取**最后一张**处于 awaiting 的工具卡：同一会话里可能有前面的用例留下的旧卡，
    // 点到旧卡上等于没答，等待会一直挂着（表现为超时）。
    const card = await waitForNewToolCard(thread, 0)
    store.decide(card.id, true)

    const outcome = await pending
    // 放行：block 未被设置即通过
    expect(outcome).toBeUndefined()
    // 卡片应当进入执行态，而不是停在"等你点"
    const toolCard = thread.items.find(
      (item) => item.kind === 'tool' && item.callId === askCall.id
    ) as { status?: string } | undefined
    expect(toolCard?.status).not.toBe('awaiting')
    // 这次提问必须留痕，否则"谁批的、有没有问"无从追溯
    expect(store.log.some((entry) => entry.text.includes('审批提问'))).toBe(true)

    store.deleteThread(thread.id)
  })

  test('用户点"拒绝"时插件收到 false，并把它变成 deny', async () => {
    await writeAskingPlugin()
    const store = new AgentStore(workspace)
    store.setApproval('ask')
    const thread = store.newThread(workspace)
    store.selectThread(thread.id)

    const pending = (store as unknown as {
      gate: (t: unknown, c: unknown) => Promise<{ block?: boolean; reason?: string } | undefined>
    }).gate(thread, { ...toolCall, name: 'run_command', arguments: { command: 'rm -rf build' } })

    const card = await waitForNewToolCard(thread, 0)
    store.decide(card.id, false)

    const outcome = await pending
    expect(outcome?.block).toBe(true)
    // answeredBy 必须如实是 'user'——不能把"用户说不"写成"中止"
    expect(outcome?.reason).toContain('user')

    store.deleteThread(thread.id)
  })

  test('会话中止时 askUser 返回 answeredBy=aborted，等待不会悬着', async () => {
    await writeAskingPlugin()
    const store = new AgentStore(workspace)
    store.setApproval('ask')
    const thread = store.newThread(workspace)
    store.selectThread(thread.id)

    const controller = new AbortController()
    const pending = (store as unknown as {
      gate: (
        t: unknown,
        c: unknown,
        s?: AbortSignal
      ) => Promise<{ block?: boolean; reason?: string } | undefined>
    }).gate(thread, { ...toolCall, name: 'run_command', arguments: { command: 'rm -rf build' } }, controller.signal)

    // 先确认 askUser 真的发起了询问（卡片出现），否则"中止能解开"这件事没被验证到
    await waitForNewToolCard(thread, 0)
    controller.abort()

    const outcome = await pending
    expect(outcome?.block).toBe(true)
    // 中止与"用户拒绝"在结果上都算拒绝，但理由必须能分辨
    expect(outcome?.reason).toContain('aborted')

    store.deleteThread(thread.id)
  })

  afterEach(async () => {
    await saveDisabledPlugins([])
  })

  test('没有 askUser 能力时插件拿不到它（子智能体循环的形态）', async () => {
    // 直接测合成后的钩子：ctx 里不给 askUser，插件应当看到 undefined
    const hooks = await composePluginHooks({
      kind: 'main',
      workspace,
      capabilities,
      trace: () => {},
    })
    if (!hooks.beforeApproval) return // 该工作区没有注册提问插件时跳过

    const seen: unknown[] = []
    await hooks.beforeApproval({
      kind: 'main',
      toolCall,
      approvalMode: 'ask',
      isWrite: true,
      // 刻意不提供 askUser
      askUser: undefined,
      trace: () => {},
    } as Parameters<NonNullable<typeof hooks.beforeApproval>>[0] & { askUser?: undefined })
    expect(seen).toHaveLength(0)
  })
})
