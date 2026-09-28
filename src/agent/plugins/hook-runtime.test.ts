/**
 * 钩子运行层（`plugins/hook-runtime.ts`）：能力开关、顺序与短路、超时、受限可见性。
 *
 * 这里全程注入插件清单（`LoadedPlugin` 字面量）与能力值，不碰文件系统——运行层本身
 * 是纯的，加载器怎么产出 `LoadedPlugin` 由最后一段的集成用例覆盖。
 */

import { describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DEFAULT_PLUGIN_CAPABILITIES, savePluginConfig, type PluginCapabilities, type ResolvedPluginCapabilities } from '../config'
import type { AgentHooks } from '../core/events'
import type { AgentTool } from '../core/types'
import { ExtensionLoader } from '../tools/loader'
import { composePluginHooks } from './hook-runtime'
import { clearLoadedPlugins, getLoadedPlugins } from './registry'
import type { LoadedPlugin, PluginScope } from './types'

function plugin(id: string, hooks: AgentHooks, scope: PluginScope = 'builtin'): LoadedPlugin {
  return {
    manifest: { id, name: id, description: '', scope },
    contributions: { tools: [], hooks },
    declarative: scope === 'builtin',
    status: 'ready',
    diagnostics: [],
  }
}

function capabilities(
  overrides: Partial<PluginCapabilities> = {},
  perPlugin: Record<string, Partial<PluginCapabilities>> = {}
): ResolvedPluginCapabilities {
  const global: PluginCapabilities = { ...DEFAULT_PLUGIN_CAPABILITIES, ...overrides }
  return {
    capabilities: global,
    forPlugin: (pluginId) => ({ ...global, ...(perPlugin[pluginId] ?? {}) }),
    invalid: [],
  }
}

const tool = (name: string): AgentTool => ({
  name,
  description: name,
  parameters: { type: 'object' },
  async execute() {
    return { output: name, ok: true }
  },
})

const turnContext = {
  step: 0,
  messages: [],
  tools: [tool('read_file'), tool('run_command')],
  kind: 'main' as const,
  threadId: 't1',
}

describe('零开销：没人注册的点位整体缺席', () => {
  test('没有插件时返回空对象，循环据此完全跳过', () => {
    const hooks = composePluginHooks({ kind: 'main', capabilities: capabilities(), plugins: [] })
    expect(Object.keys(hooks)).toEqual([])
  })

  test('只注册了 beforeTurn 的插件不会让其它点位出现', () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [plugin('builtin:only-turn', { beforeTurn: async () => undefined })],
    })
    expect(Object.keys(hooks)).toEqual(['beforeTurn'])
  })
})

describe('顺序、折叠与短路', () => {
  test('按加载顺序串行，后一个插件看到前一个收窄后的工具集', () => {
    const seen: string[][] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        plugin('builtin:a', {
          beforeTurn: async (ctx) => {
            seen.push(ctx.tools.map((entry) => entry.name))
            return { tools: ctx.tools.filter((entry) => entry.name === 'read_file') }
          },
        }),
        plugin('builtin:b', {
          beforeTurn: async (ctx) => {
            seen.push(ctx.tools.map((entry) => entry.name))
            return undefined
          },
        }),
      ],
    })

    return hooks.beforeTurn!(turnContext).then((result) => {
      expect(seen).toEqual([['read_file', 'run_command'], ['read_file']])
      const names = Array.isArray(result?.tools)
        ? result.tools.map((entry: AgentTool) => entry.name)
        : undefined
      expect(names).toEqual(['read_file'])
    })
  })

  test('任一 beforeTurn 要求终止即短路后续 before，但留下是谁要求的', async () => {
    const called: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        plugin('builtin:a', {
          beforeTurn: async () => {
            called.push('a')
            return { terminate: true, terminateReason: '够了' }
          },
        }),
        plugin('builtin:b', {
          beforeTurn: async () => {
            called.push('b')
            return undefined
          },
        }),
      ],
    })

    const result = await hooks.beforeTurn!(turnContext)
    expect(called).toEqual(['a'])
    expect(result?.terminate).toBe(true)
    expect(result?.terminateBy).toBe('builtin:a')
  })

  test("'casual' 档位未实现时明确说出来，而不是让插件以为降级成功", async () => {
    const traces: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [plugin('builtin:a', { beforeTurn: async () => ({ tools: 'casual' }) })],
      trace: (message) => traces.push(message),
    })

    const result = await hooks.beforeTurn!(turnContext)
    expect(result?.tools).toBeUndefined()
    expect(traces.some((line) => line.includes('casual'))).toBe(true)
  })

  test('afterTurn 的 appendNote 累加，terminate 只认第一个', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        plugin('builtin:a', {
          afterTurn: async () => ({ appendNote: '第一条', terminate: true }),
        }),
        plugin('builtin:b', {
          afterTurn: async () => ({ appendNote: '第二条', terminate: true }),
        }),
      ],
    })

    const result = await hooks.afterTurn!({
      step: 0,
      message: { role: 'assistant', content: '', thinking: '', toolCalls: [], timestamp: 1 },
      toolResults: [],
      effectiveToolNames: ['read_file'],
      llmDurationMs: 1,
      toolsDurationMs: 1,
      kind: 'main',
    })
    expect(result?.appendNote).toBe('第一条\n\n第二条')
    expect(result?.terminateBy).toBe('builtin:a')
  })
})

describe('能力开关', () => {
  test('allowThirdPartyHooks 关掉后：第三方钩子不生效，内置照常', async () => {
    const calls: string[] = []
    const traces: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ allowThirdPartyHooks: false }),
      plugins: [
        plugin('workspace:ext.ts', {
          beforeTurn: async () => {
            calls.push('third-party')
            return undefined
          },
        }, 'workspace'),
        plugin('builtin:a', {
          beforeTurn: async () => {
            calls.push('builtin')
            return undefined
          },
        }),
      ],
      trace: (message) => traces.push(message),
    })

    await hooks.beforeTurn!(turnContext)
    expect(calls).toEqual(['builtin'])
    // 不允许静默失效：要说出来是因为哪个开关
    expect(traces.some((line) => line.includes('allowThirdPartyHooks'))).toBe(true)
  })

  test('allowPlanModeHooks 关掉后：plan 模式下所有钩子不生效；code 模式不受影响', async () => {
    const beforeTurn = async (): Promise<undefined> => undefined
    const plugins = [plugin('builtin:a', { beforeTurn })]

    const inPlan = composePluginHooks({
      kind: 'main',
      mode: 'plan',
      capabilities: capabilities({ allowPlanModeHooks: false }),
      plugins,
    })
    expect(inPlan.beforeTurn).toBeUndefined()

    const inCode = composePluginHooks({
      kind: 'main',
      mode: 'code',
      capabilities: capabilities({ allowPlanModeHooks: false }),
      plugins,
    })
    expect(inCode.beforeTurn).toBeDefined()
  })

  test('按插件覆盖优先于全局开关', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      mode: 'plan',
      // 全局关掉，但只给这个插件打开
      capabilities: capabilities({ allowPlanModeHooks: false }, { 'builtin:a': { allowPlanModeHooks: true } }),
      plugins: [plugin('builtin:a', { beforeTurn: async () => undefined })],
    })
    expect(hooks.beforeTurn).toBeDefined()
  })

  test('allowSystemPromptReplace 关掉后：替换被丢弃并说明，追加仍生效', async () => {
    const traces: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ allowSystemPromptReplace: false }),
      plugins: [
        plugin('builtin:a', {
          beforeAgentStart: async () => ({
            systemPrompt: '我换掉了整个提示词',
            appendSystemPrompt: '补充一句',
          }),
        }),
      ],
      trace: (message) => traces.push(message),
    })

    const result = await hooks.beforeAgentStart!({
      messages: [],
      tools: [],
      systemPrompt: '原提示词',
      kind: 'main',
    })
    expect(result?.systemPrompt).toBeUndefined()
    expect(result?.appendSystemPrompt).toBe('补充一句')
    expect(traces.some((line) => line.includes('allowSystemPromptReplace'))).toBe(true)
  })

  test('allowTextRewrite 关掉后：收尾追加文本被丢弃并说明', async () => {
    const traces: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ allowTextRewrite: false }),
      plugins: [plugin('builtin:a', { afterAgentEnd: async () => ({ appendText: '收尾一句' }) })],
      trace: (message) => traces.push(message),
    })

    const result = await hooks.afterAgentEnd!({
      reason: 'completed',
      messages: [],
      stepsExecuted: 1,
      durationMs: 1,
      kind: 'main',
    })
    expect(result?.appendText).toBeUndefined()
    expect(traces.some((line) => line.includes('allowTextRewrite'))).toBe(true)
  })
})

describe('超时与异常：绝不打崩主循环', () => {
  test('超时放行（不视为拒绝）并记 trace', async () => {
    const traces: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ hookTimeoutMs: 20 }),
      plugins: [
        plugin('builtin:slow', {
          beforeTurn: async () => {
            await new Promise((resolve) => setTimeout(resolve, 60))
            return { terminate: true }
          },
        }),
      ],
      trace: (message) => traces.push(message),
    })

    const result = await hooks.beforeTurn!(turnContext)
    expect(result).toBeUndefined()
    expect(traces.some((line) => line.includes('超过 20ms') && line.includes('放行'))).toBe(true)
  })

  test('hookTimeoutMs: 0 表示不限，慢钩子照常返回', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ hookTimeoutMs: 0 }),
      plugins: [
        plugin('builtin:slow', {
          beforeTurn: async () => {
            await new Promise((resolve) => setTimeout(resolve, 60))
            return { terminate: true }
          },
        }),
      ],
    })

    const result = await hooks.beforeTurn!(turnContext)
    expect(result?.terminate).toBe(true)
  })

  test('钩子抛错 → 当作"没有意见"，其它插件的结果不受影响', async () => {
    const traces: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        plugin('builtin:boom', {
          beforeTurn: async () => {
            throw new Error('插件内部炸了')
          },
        }),
        plugin('builtin:ok', {
          beforeTurn: async () => ({ terminate: true, terminateReason: 'ok 插件说了算' }),
        }),
      ],
      trace: (message) => traces.push(message),
    })

    const result = await hooks.beforeTurn!(turnContext)
    expect(result?.terminate).toBe(true)
    expect(result?.terminateBy).toBe('builtin:ok')
    expect(traces.some((line) => line.includes('插件内部炸了') && line.includes('无意见'))).toBe(true)
  })

  test('工具调用钩子只能拦截：第一个 block 生效，后面的不再问', async () => {
    const calls: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        plugin('builtin:a', {
          beforeToolCall: async () => {
            calls.push('a')
            return { block: true, reason: '不许执行' }
          },
        }),
        plugin('builtin:b', {
          beforeToolCall: async () => {
            calls.push('b')
            return undefined
          },
        }),
      ],
    })

    const result = await hooks.beforeToolCall!({
      assistantMessage: { role: 'assistant', content: '', thinking: '', toolCalls: [], timestamp: 1 },
      toolCall: { id: 'c1', name: 'run_command', arguments: {}, rawArguments: '{}' },
      args: {},
    })
    expect(result?.block).toBe(true)
    expect(result?.reason).toBe('不许执行')
    expect(calls).toEqual(['a'])
  })

  test('耗时超过阈值时记一条 trace（hookTimeoutMs 不限也要能被测出来）', async () => {
    const traces: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ hookTimeoutMs: 0 }),
      plugins: [
        plugin('builtin:slow', {
          beforeTurn: async () => {
            await new Promise((resolve) => setTimeout(resolve, 520))
            return undefined
          },
        }),
      ],
      trace: (message) => traces.push(message),
    })

    await hooks.beforeTurn!(turnContext)
    expect(traces.some((line) => line.includes('耗时'))).toBe(true)
  })
})

describe('从加载器到运行层：钩子的来源与失效', () => {
  /**
   * 这一段走真实加载路径（临时工作区 + jiti），验证钩子从"插件怎么声明"到
   * "运行层拿到什么"整条链。前面那些用例注入的是 `LoadedPlugin` 字面量，
   * 覆盖不到导出形态、停用与 not-ready 这些**加载期**的门。
   *
   * 注意：这里**不读全局插件索引**（`getLoadedPlugins()`）——`bun test` 把各测试
   * 文件放在同一进程里并发跑，别的文件调一次 `autoLoadExtensions` 就会把索引换掉。
   * 改用 `scanPlugins`，它算的是同一套状态判定，但结果是本地的。
   */
  let workspace = ''
  let homeDir = ''
  let oldHome: string | undefined

  const setup = async (): Promise<void> => {
    workspace = await mkdtemp(join(tmpdir(), 'ada-hook-ws-'))
    homeDir = await mkdtemp(join(tmpdir(), 'ada-hook-home-'))
    oldHome = process.env.A_DA_HOME
    process.env.A_DA_HOME = homeDir
  }
  const teardown = async (): Promise<void> => {
    if (oldHome !== undefined) process.env.A_DA_HOME = oldHome
    else delete process.env.A_DA_HOME
    clearLoadedPlugins(workspace)
    await rm(workspace, { recursive: true, force: true }).catch(() => {})
    await rm(homeDir, { recursive: true, force: true }).catch(() => {})
  }

  const writeExtension = async (fileName: string, code: string): Promise<void> => {
    const dir = join(workspace, '.ada', 'extensions')
    await mkdir(dir, { recursive: true })
    await writeFile(join(dir, fileName), code, 'utf-8')
  }

  /** 只取本工作区里的第三方插件，避开内置插件（decision 自己也带钩子）。 */
  const scanMine = async (loader: ExtensionLoader): Promise<LoadedPlugin[]> => {
    const items = await loader.scanPlugins(workspace)
    return items.filter((item) => item.id.startsWith('workspace:')).map((item) => item.plugin)
  }

  test('描述符里的 hooks 与 ctx.registerHooks 都会进 contributions.hooks', async () => {
    await setup()
    try {
      await writeExtension(
        'declared.ts',
        `export default {
  name: '声明式钩子插件',
  tools: [],
  hooks: { beforeTurn: async () => undefined },
}
`
      )
      await writeExtension(
        'imperative.ts',
        `export default function (context) {
  context.registerHooks({ afterTurn: async () => undefined })
}
`
      )

      const loader = new ExtensionLoader()
      const mine = await scanMine(loader)
      const declared = mine.find((entry) => entry.manifest.id === 'workspace:declared.ts')!
      const imperative = mine.find((entry) => entry.manifest.id === 'workspace:imperative.ts')!

      expect(Object.keys(declared.contributions.hooks ?? {})).toEqual(['beforeTurn'])
      expect(Object.keys(imperative.contributions.hooks ?? {})).toEqual(['afterTurn'])

      // 运行层能直接消费它们
      const hooks = composePluginHooks({ kind: 'main', capabilities: capabilities(), plugins: mine })
      expect(Object.keys(hooks).sort()).toEqual(['afterTurn', 'beforeTurn'])
    } finally {
      await teardown()
    }
  })

  test('被停用的插件根本不参与加载，因此它的钩子也无从生效', async () => {
    await setup()
    try {
      await writeExtension(
        'toggle-me.ts',
        `export default {
  name: '可停用',
  tools: [{
    name: 'toggle_probe',
    description: '探针',
    parameters: { type: 'object' },
    async execute() {
      return { output: 'ok', ok: true }
    },
  }],
  hooks: { beforeTurn: async () => undefined },
}
`
      )

      const loader = new ExtensionLoader()
      expect(await loader.autoLoadExtensions(workspace)).toContain('toggle_probe')

      const { setPluginDisabled } = await import('../config')
      await setPluginDisabled('workspace:toggle-me.ts', true, workspace)

      // 加载层面它整个不在了（钩子自然也一起没了——运行层只会看到加载过的插件）
      expect(await loader.autoLoadExtensions(workspace)).not.toContain('toggle_probe')
      const item = (await loader.scanPlugins(workspace)).find(
        (entry) => entry.id === 'workspace:toggle-me.ts'
      )!
      expect(item.enabled).toBe(false)
    } finally {
      await teardown()
    }
  })

  test('缺必填配置（not-ready）的插件不会接管决策点', async () => {
    await setup()
    try {
      await writeExtension(
        'gated-hooks.ts',
        `export default {
  name: '缺配置的钩子插件',
  configSchema: { properties: { token: { type: 'string', title: '令牌', required: true } } },
  tools: [],
  hooks: { beforeTurn: async () => undefined },
}
`
      )
      const loader = new ExtensionLoader()

      let mine = await scanMine(loader)
      let plugin = mine.find((entry) => entry.manifest.id === 'workspace:gated-hooks.ts')!
      expect(plugin.status).toBe('not-ready')
      // 一个缺配置的插件每轮都来干预工具表，比它干脆不出现更难查
      expect(plugin.contributions.hooks).toBeUndefined()
      expect(composePluginHooks({ kind: 'main', capabilities: capabilities(), plugins: mine })).toEqual({})

      // 补上必填项后钩子上线
      await savePluginConfig('workspace:gated-hooks.ts', { token: 'abc' })
      mine = await scanMine(loader)
      plugin = mine.find((entry) => entry.manifest.id === 'workspace:gated-hooks.ts')!
      expect(plugin.status).toBe('ready')
      expect(
        Object.keys(
          composePluginHooks({ kind: 'main', capabilities: capabilities(), plugins: mine })
        )
      ).toEqual(['beforeTurn'])
    } finally {
      await teardown()
    }
  })
})
