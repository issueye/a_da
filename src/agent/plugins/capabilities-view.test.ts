/**
 * 能力开关的展示层与"受限"说明（M3-2）。
 *
 * 这些是纯函数，所以能脱离窗口测——而它们恰恰是设计文档那句硬要求的落点：
 * 关掉某个开关后，**用到它的插件必须显示受限原因，不允许静默失效**。
 */

import { describe, expect, test } from 'bun:test'
import { DEFAULT_PLUGIN_CAPABILITIES, type PluginCapabilities } from '../config'
import type { AgentHooks } from '../core/events'
import type { LoadedPlugin, PluginScope } from './types'
import { CAPABILITY_SWITCHES, describePluginRestrictions, parseHookTimeout } from './capabilities-view'

const caps = (overrides: Partial<PluginCapabilities> = {}): PluginCapabilities => ({
  ...DEFAULT_PLUGIN_CAPABILITIES,
  ...overrides,
})

function plugin(hooks: AgentHooks | undefined, scope: PluginScope = 'builtin'): LoadedPlugin {
  return {
    manifest: { id: `${scope}:probe`, name: '探针', description: '', scope },
    contributions: { tools: [], hooks },
    declarative: scope === 'builtin',
    status: 'ready',
    diagnostics: [],
  }
}

describe('开关表：每一项都要说清"关掉后会发生什么"', () => {
  test('七个布尔开关都有标签、说明与后果，且键名不重复', () => {
    expect(CAPABILITY_SWITCHES).toHaveLength(7)
    const keys = new Set<string>()
    for (const item of CAPABILITY_SWITCHES) {
      expect(item.label.length).toBeGreaterThan(0)
      expect(item.description.length).toBeGreaterThan(0)
      // 用户必须能看见后果——这是开放原则的配套要求
      expect(item.effect.length).toBeGreaterThan(0)
      expect(keys.has(item.key)).toBe(false)
      keys.add(item.key)
    }
  })
})

describe('受限说明：用到被关掉的开关时要说出来', () => {
  test('没注册钩子的插件不受钩子类开关影响', () => {
    expect(describePluginRestrictions(plugin(undefined), caps({ allowThirdPartyHooks: false }))).toEqual([])
    expect(describePluginRestrictions(plugin({}), caps({ allowPlanModeHooks: false }))).toEqual([])
  })

  test('第三方插件的钩子被整体关闭时，一句话说清"全部不生效"', () => {
    const reasons = describePluginRestrictions(
      plugin({ beforeTurn: async () => undefined }, 'workspace'),
      caps({ allowThirdPartyHooks: false })
    )

    expect(reasons).toHaveLength(1)
    expect(reasons[0]).toContain('全部不生效')
    expect(reasons[0]).toContain('allowThirdPartyHooks')
  })

  test('内置插件不受 allowThirdPartyHooks 影响', () => {
    const reasons = describePluginRestrictions(
      plugin({ beforeTurn: async () => undefined }, 'builtin'),
      caps({ allowThirdPartyHooks: false })
    )
    expect(reasons).toEqual([])
  })

  test('逐项说明：只提它注册过的点位，且写成条件句', () => {
    const reasons = describePluginRestrictions(
      plugin({
        beforeAgentStart: async () => undefined,
        afterAgentEnd: async () => undefined,
        beforeCompaction: async () => undefined,
      }),
      caps({
        allowSystemPromptReplace: false,
        allowTextRewrite: false,
        allowCompactionReplace: false,
        allowPlanModeHooks: false,
      })
    )

    expect(reasons.some((line) => line.includes('plan 模式'))).toBe(true)
    expect(reasons.some((line) => line.includes('替换系统提示词'))).toBe(true)
    expect(reasons.some((line) => line.includes('收尾文本'))).toBe(true)
    expect(reasons.some((line) => line.includes('替换压缩方案'))).toBe(true)
  })

  test('没注册 afterAgentEnd 时不提"收尾文本"（它压根不会用这个能力）', () => {
    const reasons = describePluginRestrictions(
      plugin({ beforeTurn: async () => undefined }),
      caps({ allowTextRewrite: false })
    )
    expect(reasons).toEqual([])
  })

  test('全开时任何插件都不受限', () => {
    const reasons = describePluginRestrictions(
      plugin({
        beforeAgentStart: async () => undefined,
        afterAgentEnd: async () => undefined,
        beforeCompaction: async () => undefined,
        beforeTurn: async () => undefined,
      }),
      caps()
    )
    expect(reasons).toEqual([])
  })
})

describe('超时值校验：0 是合法配置，不是填错', () => {
  test('接受非负整数，0 表示不限', () => {
    expect(parseHookTimeout('0')).toEqual({ ok: true, value: 0 })
    expect(parseHookTimeout(' 500 ')).toEqual({ ok: true, value: 500 })
  })

  test('拒绝负数、小数、空值与非数字，并说明原因', () => {
    for (const raw of ['', '-1', '1.5', 'abc', '500ms']) {
      const parsed = parseHookTimeout(raw)
      expect(parsed.ok).toBe(false)
      if (!parsed.ok) expect(parsed.reason.length).toBeGreaterThan(0)
    }
  })
})
