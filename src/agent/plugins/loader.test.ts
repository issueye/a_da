/**
 * M1 验收：契约与加载层（`docs/plugin-system-dev-plan.md` §3 M1）。
 *
 * 覆盖的验收项：
 * - 两种导出形态（描述符 / 函数）产出**字段结构等价**的 `LoadedPlugin`
 * - 内置插件与第三方插件加载后字段结构一致
 * - `getToolOrigin` 对内置/工作区插件都返回正确 pluginId
 * - 同名工具冲突可见，工具表**不出现同名两份**
 * - 缺依赖 → `broken` 且工具不注册；缺必填配置 → `not-ready` 且工具不注册
 * - `engines` 不匹配 → `incompatible`，仍加载
 * - 重载后旧事件监听器已退订（缺陷 4 回归）
 * - 按工作区启停生效
 *
 * 每个用例都换一个临时 `A_DA_HOME`：配置（启停表、pluginConfig）写在里面，
 * 既不碰用户真实的 `~/.a-da`，也不会和别的测试文件互相看见。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { AgentEvent } from '../core/types'
import { getPluginDiagnostics } from './registry'
import { savePluginCapabilities, savePluginConfig, setPluginDisabled } from '../config'
import { ExtensionLoader } from '../tools/loader'
import { defaultToolRegistry } from '../tools/registry'

let workspace = ''
let homeDir = ''
let oldHome: string | undefined

beforeEach(async () => {
  workspace = await mkdtemp(join(tmpdir(), 'ada-m1-ws-'))
  homeDir = await mkdtemp(join(tmpdir(), 'ada-m1-home-'))
  oldHome = process.env.A_DA_HOME
  process.env.A_DA_HOME = homeDir
})

afterEach(async () => {
  if (oldHome !== undefined) process.env.A_DA_HOME = oldHome
  else delete process.env.A_DA_HOME
  await rm(workspace, { recursive: true, force: true }).catch(() => {})
  await rm(homeDir, { recursive: true, force: true }).catch(() => {})
  delete (globalThis as Record<string, unknown>).__m1_listener_calls
})

/** 在临时工作区里写一个扩展文件。 */
async function writeExtension(fileName: string, code: string, target = workspace): Promise<void> {
  const dir = join(target, '.ada', 'extensions')
  await mkdir(dir, { recursive: true })
  await writeFile(join(dir, fileName), code, 'utf-8')
}

/** 一个最小可用的工具字面量（嵌进扩展源码里）。 */
function toolSnippet(name: string): string {
  return `{
      name: '${name}',
      description: '${name} 的说明',
      parameters: { type: 'object' },
      async execute() {
        return { output: 'ok', ok: true }
      },
    }`
}

describe('M1：两种导出形态产出等价的 LoadedPlugin', () => {
  test('描述符形态与函数形态：字段结构一致、溯源一致、状态一致', async () => {
    await writeExtension(
      'desc-form.ts',
      `export default {
  name: '描述符形态插件',
  description: '声明式导出',
  tools: [${toolSnippet('desc_tool')}],
}
`
    )
    await writeExtension(
      'func-form.ts',
      `export default function (context) {
  context.registerTool(${toolSnippet('func_tool')})
}
`
    )

    const loader = new ExtensionLoader()
    const loaded = await loader.autoLoadExtensions(workspace)
    expect(loaded).toContain('desc_tool')
    expect(loaded).toContain('func_tool')
    expect(defaultToolRegistry.getToolOrigin('desc_tool')?.pluginId).toBe('workspace:desc-form.ts')
    expect(defaultToolRegistry.getToolOrigin('func_tool')?.pluginId).toBe('workspace:func-form.ts')

    const items = await loader.scanPlugins(workspace)
    const desc = items.find((item) => item.id === 'workspace:desc-form.ts')!
    const func = items.find((item) => item.id === 'workspace:func-form.ts')!
    expect(desc).toBeDefined()
    expect(func).toBeDefined()

    // "一种产物、两个加载器"的可验证含义：形状完全一致
    expect(Object.keys(desc.plugin).sort()).toEqual(Object.keys(func.plugin).sort())
    expect(Object.keys(desc.plugin.manifest).sort()).toEqual(Object.keys(func.plugin.manifest).sort())
    expect(Object.keys(desc.plugin.contributions).sort()).toEqual(
      Object.keys(func.plugin.contributions).sort()
    )
    expect(desc.plugin.declarative).toBe(false)
    expect(func.plugin.declarative).toBe(false)
    expect(desc.status).toBe('ready')
    expect(func.status).toBe('ready')
    expect(desc.tools.map((tool) => tool.name)).toEqual(['desc_tool'])
    expect(func.tools.map((tool) => tool.name)).toEqual(['func_tool'])
  })

  test('内置插件与第三方插件加载后字段结构一致', async () => {
    await writeExtension(
      'third-party.ts',
      `export default { name: '第三方插件', tools: [${toolSnippet('third_party_tool')}] }
`
    )

    const loader = new ExtensionLoader()
    await loader.autoLoadExtensions(workspace)
    const items = await loader.scanPlugins(workspace)
    const builtin = items.find((item) => item.id === 'builtin:git-tools')!
    const third = items.find((item) => item.id === 'workspace:third-party.ts')!

    expect(Object.keys(builtin.plugin).sort()).toEqual(Object.keys(third.plugin).sort())
    expect(Object.keys(builtin.plugin.manifest).sort()).toEqual(
      Object.keys(third.plugin.manifest).sort()
    )
    expect(builtin.plugin.contributions.tools!.length).toBeGreaterThan(0)
    expect(third.plugin.contributions.tools!.length).toBe(1)

    // 唯一的实质差别是"能否不执行代码就展示"（内置可，jiti 出来的不可）
    expect(builtin.plugin.declarative).toBe(true)
    expect(third.plugin.declarative).toBe(false)
    expect(builtin.plugin.status).toBe('ready')
    expect(third.plugin.status).toBe('ready')
  })

  test('导出对象既不是工具也不是描述符时给出诊断，而不是静默当作工具', async () => {
    await writeExtension(
      'malformed.ts',
      `export default function () {
  return { name: '看起来像工具但没有 execute' }
}
`
    )

    const loader = new ExtensionLoader()
    const loaded = await loader.autoLoadExtensions(workspace)
    expect(loaded).not.toContain('看起来像工具但没有 execute')

    const items = await loader.scanPlugins(workspace)
    const item = items.find((entry) => entry.id === 'workspace:malformed.ts')!
    expect(item.diagnostics.some((diagnostic) => diagnostic.level === 'warn')).toBe(true)
  })
})

describe('M1：依赖、版本与必填配置的门禁', () => {
  test('依赖缺失 → broken 且工具不注册；依赖满足 → ready 且工具注册', async () => {
    await writeExtension(
      'needs-missing.ts',
      `export default {
  name: '缺依赖的插件',
  dependsOn: ['workspace:not-installed.ts'],
  tools: [${toolSnippet('needs_missing_tool')}],
}
`
    )
    await writeExtension(
      'needs-git.ts',
      `export default {
  name: '依赖内置 git 插件',
  dependsOn: ['builtin:git-tools'],
  tools: [${toolSnippet('needs_git_tool')}],
}
`
    )

    const loader = new ExtensionLoader()
    const loaded = await loader.autoLoadExtensions(workspace)

    expect(loaded).toContain('needs_git_tool')
    expect(loaded).not.toContain('needs_missing_tool')

    const items = await loader.scanPlugins(workspace)
    const missing = items.find((item) => item.id === 'workspace:needs-missing.ts')!
    expect(missing.status).toBe('broken')
    expect(
      missing.diagnostics.some(
        (diagnostic) => diagnostic.level === 'error' && diagnostic.message.includes('not-installed')
      )
    ).toBe(true)

    // 加载不再静默：诊断可查询（M1-4）
    expect(
      getPluginDiagnostics().some(
        (diagnostic) =>
          diagnostic.pluginId === 'workspace:needs-missing.ts' && diagnostic.level === 'error'
      )
    ).toBe(true)
  })

  test('缺必填配置 → not-ready 且工具不注册；补上配置后变 ready', async () => {
    await writeExtension(
      'gated.ts',
      `export default {
  name: '需要配置的插件',
  configSchema: {
    properties: { token: { type: 'string', title: '访问令牌', required: true } },
  },
  tools: [${toolSnippet('gated_tool')}],
}
`
    )

    const loader = new ExtensionLoader()
    let loaded = await loader.autoLoadExtensions(workspace)
    expect(loaded).not.toContain('gated_tool')

    let item = (await loader.scanPlugins(workspace)).find(
      (entry) => entry.id === 'workspace:gated.ts'
    )!
    expect(item.status).toBe('not-ready')
    expect(
      item.diagnostics.some(
        (diagnostic) => diagnostic.level === 'error' && diagnostic.message.includes('token')
      )
    ).toBe(true)
    // 缺配置的插件也不该出现在工具表里
    expect(
      defaultToolRegistry
        .getToolsForWorkspace(workspace)
        .some((tool) => tool.name === 'gated_tool')
    ).toBe(false)

    await savePluginConfig('workspace:gated.ts', { token: 'abc' })

    loaded = await loader.autoLoadExtensions(workspace)
    expect(loaded).toContain('gated_tool')
    item = (await loader.scanPlugins(workspace)).find(
      (entry) => entry.id === 'workspace:gated.ts'
    )!
    expect(item.status).toBe('ready')
  })

  test('engines 与当前应用不匹配 → incompatible，但仍加载并只给警告', async () => {
    await writeExtension(
      'future.ts',
      `export default {
  name: '声明了未来版本的插件',
  engines: { a_da: '>=99.0.0' },
  tools: [${toolSnippet('future_tool')}],
}
`
    )

    const loader = new ExtensionLoader()
    const loaded = await loader.autoLoadExtensions(workspace)
    expect(loaded).toContain('future_tool')

    const item = (await loader.scanPlugins(workspace)).find(
      (entry) => entry.id === 'workspace:future.ts'
    )!
    expect(item.status).toBe('incompatible')
    expect(item.diagnostics.some((diagnostic) => diagnostic.level === 'warn')).toBe(true)
    expect(item.diagnostics.some((diagnostic) => diagnostic.level === 'error')).toBe(false)
  })
})

describe('M1：冲突、监听器与工作区启停', () => {
  test('两个插件抢同名工具：工具表只留一份，双方状态标为 conflict', async () => {
    await writeExtension(
      'dup-a.ts',
      `export default { name: '抢名字的 A', tools: [${toolSnippet('dup_tool')}] }
`
    )
    await writeExtension(
      'dup-b.ts',
      `export default { name: '抢名字的 B', tools: [${toolSnippet('dup_tool')}] }
`
    )

    const loader = new ExtensionLoader()
    const loaded = await loader.autoLoadExtensions(workspace)
    expect(loaded.filter((name) => name === 'dup_tool')).toHaveLength(1)

    // 真正的验收点是**工具表**不出现同名两份
    const tableNames = defaultToolRegistry
      .getToolsForWorkspace(workspace)
      .map((tool) => tool.name)
    expect(tableNames.filter((name) => name === 'dup_tool')).toHaveLength(1)

    const items = await loader.scanPlugins(workspace)
    const first = items.find((item) => item.id === 'workspace:dup-a.ts')!
    const second = items.find((item) => item.id === 'workspace:dup-b.ts')!
    expect(first.status).toBe('conflict')
    expect(second.status).toBe('conflict')
    expect(
      [first, second].some((item) =>
        item.diagnostics.some((diagnostic) => diagnostic.message.includes('dup_tool'))
      )
    ).toBe(true)
  })

  test('重载后旧事件监听器已退订（缺陷 4 回归）', async () => {
    await writeExtension(
      'listener.ts',
      `export default function (context) {
  context.onEvent(() => {
    globalThis.__m1_listener_calls = (globalThis.__m1_listener_calls ?? 0) + 1
  })
  context.registerTool(${toolSnippet('listener_tool')})
}
`
    )

    const loader = new ExtensionLoader()
    await loader.autoLoadExtensions(workspace)
    await loader.autoLoadExtensions(workspace)
    await loader.autoLoadExtensions(workspace)

    // 三次加载只该留下一个监听器
    expect(loader.getListenerStats()['workspace:listener.ts']).toBe(1)

    const globals = globalThis as Record<string, unknown>
    globals.__m1_listener_calls = 0
    loader.dispatchAgentEvent({ type: 'agent_start' } as unknown as AgentEvent)
    expect(globals.__m1_listener_calls).toBe(1)
  })

  test('按工作区停用只影响这个工作区，且插件仍在列表里可见', async () => {
    const extensionCode = `export default { name: '可停用的插件', tools: [${toolSnippet('ws_toggle_tool')}] }
`
    await writeExtension('ws-toggle.ts', extensionCode)
    // 另一个工作区装同一个插件：用来验证"停用只作用于记下的那个工作区"
    const otherWorkspace = await mkdtemp(join(tmpdir(), 'ada-m1-other-'))
    await writeExtension('ws-toggle.ts', extensionCode, otherWorkspace)
    const loader = new ExtensionLoader()

    expect(await loader.autoLoadExtensions(workspace)).toContain('ws_toggle_tool')

    await setPluginDisabled('workspace:ws-toggle.ts', true, workspace)

    expect(await loader.autoLoadExtensions(workspace)).not.toContain('ws_toggle_tool')
    expect(await loader.autoLoadExtensions(otherWorkspace)).toContain('ws_toggle_tool')

    const item = (await loader.scanPlugins(workspace)).find(
      (entry) => entry.id === 'workspace:ws-toggle.ts'
    )!
    expect(item.enabled).toBe(false)

    await setPluginDisabled('workspace:ws-toggle.ts', false, workspace)
    expect(await loader.autoLoadExtensions(workspace)).toContain('ws_toggle_tool')
    await rm(otherWorkspace, { recursive: true, force: true }).catch(() => {})
  })
})

/**
 * `allowBuiltinShadow` 曾经是个"幽灵开关"：声明了、有默认值、界面还写明关掉后的效果，
 * 但没有任何代码读它，用户关掉什么都不会发生。这一组用例把两个方向都钉住——
 * 默认开着时"覆盖 + 标冲突"，关掉时"不注册 + 保留内置 + 原因可见"。
 *
 * 同时钉住 `isWriteTool` 的来源判定：插件借走 `read_file` 这个名字时，它不能因此
 * 获得只读身份（否则 plan 模式放行、readonly 审批不问）。
 */
describe('能力开关：allowBuiltinShadow', () => {
  const shadowCode = `export default {
  name: '借名插件',
  tools: [${toolSnippet('read_file')}, ${toolSnippet('shadow_side_tool')}],
}
`

  test('默认开着：插件工具覆盖同名内置工具，状态标为 conflict，并失去只读身份', async () => {
    await writeExtension('shadow-on.ts', shadowCode)

    const loader = new ExtensionLoader()
    const loaded = await loader.autoLoadExtensions(workspace)

    expect(loaded).toContain('read_file')
    expect(defaultToolRegistry.getToolOrigin('read_file')?.pluginId).toBe('workspace:shadow-on.ts')
    // 借走只读名字不再等于拿到只读身份
    expect(defaultToolRegistry.isWriteTool('read_file')).toBe(true)

    const item = (await loader.scanPlugins(workspace)).find(
      (entry) => entry.id === 'workspace:shadow-on.ts'
    )!
    expect(item.status).toBe('conflict')
    expect(item.plugin.blockedTools).toBeUndefined()
  })

  test('关掉后：同名工具不注册、内置工具保留，且原因在插件卡上可见', async () => {
    await writeExtension('shadow-off.ts', shadowCode)
    await savePluginCapabilities({ allowBuiltinShadow: false })

    const loader = new ExtensionLoader()
    const loaded = await loader.autoLoadExtensions(workspace)

    expect(loaded).not.toContain('read_file')
    // 同一个插件里没借名的工具照常注册：挡下的是那一个工具，不是整个插件
    expect(loaded).toContain('shadow_side_tool')
    expect(defaultToolRegistry.getToolOrigin('read_file')).toBeUndefined()

    // 工具表里只有一份 read_file，而且是核心内置那份
    const names = defaultToolRegistry.getToolsForWorkspace(workspace).map((tool) => tool.name)
    expect(names.filter((name) => name === 'read_file')).toHaveLength(1)
    // 内置工具的只读身份没被牵连
    expect(defaultToolRegistry.isWriteTool('read_file')).toBe(false)

    const item = (await loader.scanPlugins(workspace)).find(
      (entry) => entry.id === 'workspace:shadow-off.ts'
    )!
    expect(item.status).toBe('conflict')
    expect(item.plugin.blockedTools).toEqual(['read_file'])
    expect(
      item.diagnostics.some(
        (diagnostic) => diagnostic.level === 'warn' && diagnostic.message.includes('allowBuiltinShadow')
      )
    ).toBe(true)
    // 被挡下的工具仍在卡片上列着——不能凭空消失，否则用户查不出为什么没生效
    expect(item.tools.map((tool) => tool.name)).toContain('read_file')
  })
})
