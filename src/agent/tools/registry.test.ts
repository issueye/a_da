/**
 * 工具注册表的溯源、冲突与去重（设计文档 §5.1、§3 缺陷 1/2/5）。
 *
 * 这些用例全部用 `new ToolRegistry()`：注册表是可独立实例化的，用单例会让
 * 「同名工具」这类断言受别的测试文件影响。
 */

import { describe, expect, spyOn, test } from 'bun:test'
import type { AgentTool } from '../core/types'
import { ToolRegistry } from './registry'

function probeTool(name: string): AgentTool {
  return {
    name,
    description: `${name} 的说明`,
    parameters: { type: 'object' },
    async execute() {
      return { output: 'ok', ok: true }
    },
  }
}

describe('ToolRegistry：溯源', () => {
  test('插件工具能查到来源；核心内置工具没有插件来源', () => {
    const registry = new ToolRegistry()
    registry.register(probeTool('git_status'), { pluginId: 'builtin:git-tools', scope: 'builtin' })
    registry.register(probeTool('web_search'), { pluginId: 'workspace:web-search.ts', scope: 'workspace' })

    expect(registry.getToolOrigin('git_status')).toEqual({
      pluginId: 'builtin:git-tools',
      scope: 'builtin',
    })
    expect(registry.getToolOrigin('web_search')?.pluginId).toBe('workspace:web-search.ts')
    // read_file 是核心工具，不归任何插件
    expect(registry.getToolOrigin('read_file')).toBeUndefined()
  })

  test('listByPlugin 只列该插件的工具', () => {
    const registry = new ToolRegistry()
    registry.register(probeTool('a'), { pluginId: 'builtin:x' })
    registry.register(probeTool('b'), { pluginId: 'builtin:x' })
    registry.register(probeTool('c'), { pluginId: 'global:y.ts' })

    expect(registry.listByPlugin('builtin:x')).toEqual(['a', 'b'])
    expect(registry.listByPlugin('global:y.ts')).toEqual(['c'])
    expect(registry.listByPlugin('builtin:不存在')).toEqual([])
  })

  test('unregisterPlugin 只摘掉该插件的工具，别的插件不受影响', () => {
    const registry = new ToolRegistry()
    registry.register(probeTool('a'), { pluginId: 'builtin:x' })
    registry.register(probeTool('b'), { pluginId: 'global:y.ts' })

    registry.unregisterPlugin('builtin:x')

    expect(registry.listByPlugin('builtin:x')).toEqual([])
    expect(registry.listByPlugin('global:y.ts')).toEqual(['b'])
  })
})

describe('ToolRegistry：冲突与去重', () => {
  test('插件工具遮蔽核心内置工具：记冲突、发出可见警告、工具表只留一份', () => {
    const registry = new ToolRegistry()
    const warn = spyOn(console, 'warn').mockImplementation(() => {})

    const conflict = registry.register(probeTool('read_file'), { pluginId: 'workspace:shadow.ts' })
    warn.mockRestore()

    expect(conflict).toEqual({
      name: 'read_file',
      pluginId: 'workspace:shadow.ts',
      shadowedBuiltin: true,
    })
    expect(registry.getConflicts()).toHaveLength(1)

    // 关键：不是"多出一条"，而是仍然只有一条 read_file，且是插件那条
    const names = registry.getToolsForWorkspace(process.cwd()).map((tool) => tool.name)
    expect(names.filter((name) => name === 'read_file')).toHaveLength(1)
    expect(registry.getToolOrigin('read_file')?.pluginId).toBe('workspace:shadow.ts')
  })

  test('两个插件抢同一个工具名：后注册者生效，双方都进冲突报告', () => {
    const registry = new ToolRegistry()
    const warn = spyOn(console, 'warn').mockImplementation(() => {})

    registry.register(probeTool('dup'), { pluginId: 'workspace:first.ts' })
    registry.register(probeTool('dup'), { pluginId: 'global:second.ts' })
    warn.mockRestore()

    expect(registry.getConflicts()).toEqual([
      { name: 'dup', pluginId: 'global:second.ts', shadowedPluginId: 'workspace:first.ts', shadowedBuiltin: false },
    ])
    const names = registry.getToolsForWorkspace(process.cwd()).map((tool) => tool.name)
    expect(names.filter((name) => name === 'dup')).toHaveLength(1)
    expect(registry.getToolOrigin('dup')?.pluginId).toBe('global:second.ts')
  })

  test('没有冲突时报告为空；重名不累积成多条历史', () => {
    const registry = new ToolRegistry()
    const warn = spyOn(console, 'warn').mockImplementation(() => {})

    registry.register(probeTool('fine'), { pluginId: 'builtin:x' })
    expect(registry.getConflicts()).toEqual([])

    // 反复注册同一个冲突名（重载会这样），报告里始终只有一条
    registry.register(probeTool('read_file'), { pluginId: 'builtin:x' })
    registry.register(probeTool('read_file'), { pluginId: 'builtin:x' })
    registry.register(probeTool('read_file'), { pluginId: 'builtin:x' })
    warn.mockRestore()

    expect(registry.getConflicts()).toHaveLength(1)
  })

  test('clearCustomTools 同时清掉工具与冲突记录', () => {
    const registry = new ToolRegistry()
    const warn = spyOn(console, 'warn').mockImplementation(() => {})
    registry.register(probeTool('edit_file'), { pluginId: 'builtin:x' })
    warn.mockRestore()
    expect(registry.getConflicts()).toHaveLength(1)

    registry.clearCustomTools()

    expect(registry.getCustomTools()).toEqual([])
    expect(registry.getConflicts()).toEqual([])
  })
})

/**
 * `isWriteTool` 是所有只读判定的唯一入口（plan 模式过滤、readonly 审批档、只读子智能体）。
 * 它过去只看名字，于是"名字在只读名单里"就等于"工具安全"——而插件可以借走内置名字
 * （`allowBuiltinShadow` 默认开）。这一组用例把"分类必须看来源"钉住。
 */
describe('ToolRegistry：写工具的判定要看来源，不能只看名字', () => {
  test('非内置插件借走只读内置名 → 按写处理', () => {
    const registry = new ToolRegistry()
    const warn = spyOn(console, 'warn').mockImplementation(() => {})
    registry.register(probeTool('read_file'), {
      pluginId: 'workspace:shadow.ts',
      scope: 'workspace',
    })
    warn.mockRestore()

    expect(registry.isWriteTool('read_file')).toBe(true)
    // 没被借走的核心工具照旧是只读：修复不能把正常路径一起收紧
    expect(registry.isWriteTool('list_files')).toBe(false)
  })

  test('官方内置插件的只读工具不受影响（scope 为 builtin）', () => {
    const registry = new ToolRegistry()
    registry.register(probeTool('git_status'), {
      pluginId: 'builtin:git-tools',
      scope: 'builtin',
    })
    registry.register(probeTool('get_outline'), {
      pluginId: 'builtin:code-outline',
      scope: 'builtin',
    })

    expect(registry.isWriteTool('git_status')).toBe(false)
    expect(registry.isWriteTool('get_outline')).toBe(false)
  })

  test('来源缺失（不走插件路径注册）也按写处理：失败安全', () => {
    const registry = new ToolRegistry()
    const warn = spyOn(console, 'warn').mockImplementation(() => {})
    registry.register(probeTool('edit_file'))
    warn.mockRestore()

    expect(registry.isWriteTool('edit_file')).toBe(true)
  })

  test('isBuiltinToolName 认得核心内置名，不认插件自己的名字', () => {
    const registry = new ToolRegistry()
    expect(registry.isBuiltinToolName('read_file')).toBe(true)
    expect(registry.isBuiltinToolName('web_search')).toBe(false)
  })
})
