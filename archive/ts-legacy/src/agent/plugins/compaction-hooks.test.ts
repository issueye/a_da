/**
 * 上下文压缩的插件干预（M3-6，设计文档 §6.6.2 第一优先）。
 *
 * 覆盖开发计划的验收清单：
 * - `beforeCompaction` 追加的保留消息**确实出现在压缩后的历史里**
 * - 插件**可以**替换 `CompactSelection`（`allowCompactionReplace` 默认开），
 *   关掉时替换被忽略、追加保留仍生效
 * - `afterCompaction` 能拿到压缩前后的消息数对比
 *
 * 判定应用逻辑（keep → 从待总结挪到保留、保持时间顺序）单独放在 store.ts 的
 * `applyCompactionVerdict`，这里直接测它，不依赖模型接口。
 */

import { describe, expect, test } from 'bun:test'
import type { AgentMessage } from '../core/types'
import type { CompactSelection } from '../compact/types'
import { composePluginHooks } from './hook-runtime'
import { DEFAULT_PLUGIN_CAPABILITIES, type ResolvedPluginCapabilities } from '../config'
import type { LoadedPlugin } from './types'

const capabilities = (
  overrides: Partial<typeof DEFAULT_PLUGIN_CAPABILITIES> = {}
): ResolvedPluginCapabilities => {
  const caps = { ...DEFAULT_PLUGIN_CAPABILITIES, ...overrides }
  return { capabilities: caps, forPlugin: () => caps, overrides: {}, invalid: [] }
}

function pluginWith(hooks: LoadedPlugin['contributions']['hooks']): LoadedPlugin {
  return {
    manifest: { id: 'builtin:compact-probe', name: '压缩探针', description: '', scope: 'builtin' },
    contributions: { tools: [], hooks },
    declarative: true,
    status: 'ready',
    diagnostics: [],
  }
}

const message = (text: string, timestamp: number): AgentMessage => ({
  role: 'user',
  content: text,
  timestamp,
})

const early = message('早期：请记住这个约束', 1)
const middle = message('中期：随便聊聊', 2)
const recent = message('最近：继续', 3)

const selection: CompactSelection = {
  messagesToSummarize: [early, middle],
  preservedMessages: [recent],
  prunedItems: [],
  preservedItems: [],
  turnsSummarized: 1,
}

const context = {
  kind: 'main' as const,
  workspace: '/tmp/ws',
  threadId: 't1',
  trigger: 'manual' as const,
  selection,
  messageCount: 3,
  itemCount: 3,
}

describe('运行层：追加保留与替换选择方案', () => {
  test('追加保留消息：任何配置下都生效', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ allowCompactionReplace: false }),
      plugins: [pluginWith({ beforeCompaction: async () => ({ keepMessages: [early] }) })],
    })

    const result = await hooks.beforeCompaction!(context)
    expect(result?.keepMessages).toEqual([early])
  })

  test('替换选择方案：allowCompactionReplace 关掉时被忽略并说明', async () => {
    const traces: string[] = []
    const replacement: CompactSelection = { ...selection, messagesToSummarize: [middle] }
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ allowCompactionReplace: false }),
      plugins: [pluginWith({ beforeCompaction: async () => ({ selection: replacement }) })],
      trace: (line) => traces.push(line),
    })

    const result = await hooks.beforeCompaction!(context)
    expect(result?.selection).toBeUndefined()
    expect(traces.some((line) => line.includes('allowCompactionReplace'))).toBe(true)
  })

  test('替换选择方案：默认开启时生效，并记下是谁替换的', async () => {
    const replacement: CompactSelection = { ...selection, messagesToSummarize: [middle] }
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [pluginWith({ beforeCompaction: async () => ({ selection: replacement }) })],
    })

    const result = await hooks.beforeCompaction!(context)
    expect(result?.selection?.messagesToSummarize).toEqual([middle])
    expect(result?.by).toBe('builtin:compact-probe')
  })

  test('afterCompaction 是纯观察：抛错不影响调用方', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        pluginWith({
          afterCompaction: async () => {
            throw new Error('观察钩子炸了')
          },
        }),
      ],
    })

    await expect(
      hooks.afterCompaction!({
        kind: 'main',
        trigger: 'auto',
        before: { messages: 10, items: 10 },
        after: { messages: 3, items: 3 },
        turnsSummarized: 4,
        savedTokens: 1234,
        durationMs: 88,
        success: true,
      })
    ).resolves.toBeUndefined()
  })
})

describe('判定应用：追加保留的消息真的从待总结里挪出来了', () => {
  test('按引用匹配，保持时间顺序（被留下的并到保留区最前面）', async () => {
    const { applyCompactionVerdict } = await import('../compact/verdict')
    const next = applyCompactionVerdict(selection, { keepMessages: [early] })

    expect(next.messagesToSummarize).toEqual([middle])
    // early 本来在 recent 之前，所以它排在保留区最前
    expect(next.preservedMessages).toEqual([early, recent])
  })

  test('要保留的消息不在待总结里时什么也不改', async () => {
    const { applyCompactionVerdict } = await import('../compact/verdict')
    const outsider = message('不在这一批里', 99)
    const next = applyCompactionVerdict(selection, { keepMessages: [outsider] })

    expect(next.messagesToSummarize).toEqual([early, middle])
    expect(next.preservedMessages).toEqual([recent])
  })

  test('插件递回结构不可用的选择方案时，退回原选择（不让压缩读到 undefined）', async () => {
    const { applyCompactionVerdict } = await import('../compact/verdict')
    const broken = {
      ...selection,
      messagesToSummarize: undefined,
    } as unknown as CompactSelection

    const next = applyCompactionVerdict(selection, { selection: broken })
    expect(next).toBe(selection)
  })
})
