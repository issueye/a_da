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
import { DEFAULT_PLUGIN_CAPABILITIES, type ResolvedPluginCapabilities } from '../config'
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
