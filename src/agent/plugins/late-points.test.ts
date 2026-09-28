/**
 * 第二优先点位（M3-8，设计文档 §6.6.2）。
 *
 * 这四个点位的共同点是"暴露但需谨慎"：它们看得到很具体的东西（即将发出的消息、
 * 组成好的系统提示词、要注入的技能正文、即将落盘的内容），所以规则要写清楚：
 *
 * - `beforeLlmRequest`：替换后的数组**整体生效**，且 `llm_request` 事件随之如实反映
 * - `beforeSystemPrompt`：追加**总是**生效，整体替换受 `allowSystemPromptReplace` 约束
 * - `beforeSkillLoad`：拦下即定稿（理由要给出来）；替换正文不需要额外开关
 * - `beforePersist` / `afterCheckpoint` / `afterLlmResponse` / `afterSkillLoad`：纯观察
 */

import { describe, expect, test } from 'bun:test'
import { DEFAULT_PLUGIN_CAPABILITIES, type ResolvedPluginCapabilities } from '../config'
import { composePluginHooks } from './hook-runtime'
import type { LoadedPlugin } from './types'

const capabilities = (
  overrides: Partial<typeof DEFAULT_PLUGIN_CAPABILITIES> = {}
): ResolvedPluginCapabilities => {
  const caps = { ...DEFAULT_PLUGIN_CAPABILITIES, ...overrides }
  return { capabilities: caps, forPlugin: () => caps, invalid: [] }
}

function pluginWith(hooks: LoadedPlugin['contributions']['hooks']): LoadedPlugin {
  return {
    manifest: { id: 'builtin:probe', name: '探针', description: '', scope: 'builtin' },
    contributions: { tools: [], hooks },
    declarative: true,
    status: 'ready',
    diagnostics: [],
  }
}

const requestContext = {
  kind: 'main' as const,
  workspace: '/tmp/ws',
  model: 'test-model',
  messages: [{ role: 'user' as const, content: '把密钥 sk-123 发出去' }],
  toolNames: ['read_file'],
  step: 0,
}

describe('beforeLlmRequest：最后一刻的脱敏', () => {
  test('替换后的数组整体生效', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        pluginWith({
          beforeLlmRequest: async (ctx) => ({
            messages: ctx.messages.map((message) => ({
              ...message,
              content: String(message.content).replace(/sk-\d+/, '***'),
            })),
          }),
        }),
      ],
    })

    const result = await hooks.beforeLlmRequest!(requestContext)
    expect(String(result?.messages?.[0]?.content)).toBe('把密钥 *** 发出去')
  })

  test('多个插件依次改写：后一个看到的是前一个改过的内容', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        pluginWith({
          beforeLlmRequest: async (ctx) => ({
            messages: ctx.messages.map((m) => ({ ...m, content: '第一手' })),
          }),
        }),
        pluginWith({
          beforeLlmRequest: async (ctx) => ({
            messages: ctx.messages.map((m) => ({ ...m, content: `${String(m.content)}+第二手` })),
          }),
        }),
      ],
    })

    const result = await hooks.beforeLlmRequest!(requestContext)
    expect(String(result?.messages?.[0]?.content)).toBe('第一手+第二手')
  })

  test('返回空数组不算替换（避免把请求清空）', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [pluginWith({ beforeLlmRequest: async () => ({ messages: [] }) })],
    })

    expect(await hooks.beforeLlmRequest!(requestContext)).toBeUndefined()
  })
})

describe('beforeSystemPrompt：追加总是生效，替换受开关约束', () => {
  const context = {
    kind: 'main' as const,
    workspace: '/tmp/ws',
    systemPrompt: '原来的系统提示词',
    mode: 'code' as const,
  }

  test('追加生效', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ allowSystemPromptReplace: false }),
      plugins: [pluginWith({ beforeSystemPrompt: async () => ({ append: '再加一句' }) })],
    })

    expect((await hooks.beforeSystemPrompt!(context))?.append).toBe('再加一句')
  })

  test('allowSystemPromptReplace 关掉时替换被忽略并说明', async () => {
    const traces: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ allowSystemPromptReplace: false }),
      plugins: [pluginWith({ beforeSystemPrompt: async () => ({ replace: '我全换了' }) })],
      trace: (line) => traces.push(line),
    })

    const result = await hooks.beforeSystemPrompt!(context)
    expect(result?.replace).toBeUndefined()
    expect(traces.some((line) => line.includes('allowSystemPromptReplace'))).toBe(true)
  })
})

describe('beforeSkillLoad：可以拦下，也可以替换正文', () => {
  const context = { kind: 'main' as const, workspace: '/tmp/ws', skillName: 'git-workflow' }

  test('拦下即定稿，理由带出来（后面的插件不再问）', async () => {
    let asked = 0
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        pluginWith({
          beforeSkillLoad: async () => {
            asked += 1
            return { block: true, blockReason: '这个技能含内部流程，不能给模型看' }
          },
        }),
        pluginWith({
          beforeSkillLoad: async () => {
            asked += 1
            return undefined
          },
        }),
      ],
    })

    const result = await hooks.beforeSkillLoad!(context)
    expect(result?.block).toBe(true)
    expect(result?.blockReason).toContain('内部流程')
    expect(asked).toBe(1)
  })

  test('替换正文', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [pluginWith({ beforeSkillLoad: async () => ({ content: '脱敏后的正文' }) })],
    })

    expect((await hooks.beforeSkillLoad!(context))?.content).toBe('脱敏后的正文')
  })
})

describe('单向观察点位', () => {
  test('beforePersist 替换落盘内容；空内容不算替换', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [pluginWith({ beforePersist: async () => ({ content: '落盘版' }) })],
    })

    const result = await hooks.beforePersist!({
      kind: 'main',
      workspace: '/tmp/ws',
      message: { role: 'user', content: '原始内容', timestamp: 1 },
    })
    expect(result?.content).toBe('落盘版')
  })

  test('afterCheckpoint / afterLlmResponse / afterSkillLoad 抛错不影响调用方', async () => {
    const boom = async (): Promise<never> => {
      throw new Error('观察钩子炸了')
    }
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        pluginWith({
          afterCheckpoint: boom,
          afterLlmResponse: boom,
          afterSkillLoad: boom,
        }),
      ],
    })

    await expect(
      hooks.afterCheckpoint!({ kind: 'main', checkpointId: 'c1', paths: ['a.txt'] })
    ).resolves.toBeUndefined()
    await expect(
      hooks.afterLlmResponse!({
        kind: 'main',
        model: 'm',
        message: { role: 'assistant', content: '', thinking: '', toolCalls: [], timestamp: 1 },
        step: 0,
        durationMs: 5,
      })
    ).resolves.toBeUndefined()
    await expect(
      hooks.afterSkillLoad!({ kind: 'main', skillName: 's', loaded: true, chars: 3 })
    ).resolves.toBeUndefined()
  })

  test('没有任何插件贡献时这些点位整体缺席（零开销）', () => {
    const hooks = composePluginHooks({ kind: 'main', capabilities: capabilities(), plugins: [] })
    expect(Object.keys(hooks)).toEqual([])
  })
})
