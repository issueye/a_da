/**
 * M0 等价性测试：结构调整后**行为必须完全没变**。
 *
 * docs/plugin-system-design.md §4.3.3 定的策略——本项目尚未正式使用，所以不做兼容层，
 * 而是用一次性的等价性验证来兜住"改名 / 契约统一"这类机械性改动：
 * 改名很容易顺带改错一个工具名、标签或描述，而那不会被 `typecheck` 发现。
 *
 * 这里钉住三类事实：
 * 1. 6 个内置插件的**工具名集合**与**技能/提示词名**（改名不该动它们）；
 * 2. 契约统一后，工具实例的**标签与描述**仍在（插件管理页要展示）；
 * 3. 核心工具目录与插件工具的**职责边界**——决策工具属插件，不该再出现在核心目录里。
 */

import { describe, expect, test } from 'bun:test'
import { BUILTIN_PLUGINS } from '../tools/builtin-plugins'
import { BUILTIN_TOOLS_CATALOG } from '../tools/registry'

/** 把插件的 tools 就地实例化，拿到工具名集合（工厂按 workspace 调用）。 */
function toolNamesOf(plugin: (typeof BUILTIN_PLUGINS)[number]): string[] {
  return plugin.tools
    .map((t) => (typeof t === 'function' ? t('/tmp/ws') : t))
    .map((t) => t.name)
}

describe('M0 等价性：内置插件改名后行为不变', () => {
  test('六个内置插件都在，且 id 未变', () => {
    const ids = BUILTIN_PLUGINS.map((p) => p.id).sort()
    expect(ids).toEqual(
      ['batch-ops', 'code-outline', 'decision', 'git-tools', 'project-inspector', 'test-runner'].sort(),
    )
  })

  test('每个插件的工具名集合与改动前一致', () => {
    // 这是改名最容易被改错的地方：名字集合必须逐字对上。
    const expected: Record<string, string[]> = {
      'git-tools': ['git_status', 'git_diff', 'git_log'],
      'code-outline': ['get_outline'],
      'project-inspector': ['inspect_project'],
      'test-runner': ['run_test_focused'],
      'batch-ops': ['read_files', 'edit_files'],
      decision: ['decide', 'design_decision', 'check_gate'],
    }

    for (const plugin of BUILTIN_PLUGINS) {
      expect(toolNamesOf(plugin).sort()).toEqual([...expected[plugin.id]].sort())
    }
  })

  test('每个插件的技能与提示词名未变', () => {
    const expectedSkills: Record<string, string[]> = {
      'git-tools': ['git-workflow'],
      'code-outline': ['code-navigation'],
      'project-inspector': ['project-setup'],
      'test-runner': ['tdd-workflow'],
      'batch-ops': ['batch-efficiency'],
      decision: ['decision-discipline'],
    }
    const expectedPrompts: Record<string, string[]> = {
      'git-tools': ['git-diff-summary'],
      'code-outline': ['outline'],
      'project-inspector': ['diagnose'],
      'test-runner': ['fix-test'],
      'batch-ops': ['batch-refactor'],
      decision: ['decide', 'gate'],
    }

    for (const plugin of BUILTIN_PLUGINS) {
      expect((plugin.skills ?? []).map((s) => s.name).sort()).toEqual(
        [...expectedSkills[plugin.id]].sort(),
      )
      expect((plugin.prompts ?? []).map((p) => p.name).sort()).toEqual(
        [...expectedPrompts[plugin.id]].sort(),
      )
    }
  })

  test('工具实例仍带标签与描述（插件管理页要展示，改名不该清掉）', () => {
    for (const plugin of BUILTIN_PLUGINS) {
      for (const factory of plugin.tools) {
        const tool = typeof factory === 'function' ? factory('/tmp/ws') : factory
        expect(tool.name.length).toBeGreaterThan(0)
        expect(tool.description.length).toBeGreaterThan(0)
      }
    }
  })

  test('插件元信息（名称/说明）非空', () => {
    for (const plugin of BUILTIN_PLUGINS) {
      expect(plugin.name.length).toBeGreaterThan(0)
      expect(plugin.description.length).toBeGreaterThan(0)
    }
  })
})

describe('M0 等价性：核心工具目录与插件工具的边界', () => {
  test('决策工具属插件，不该出现在核心工具目录里（消除重复来源）', () => {
    const names = BUILTIN_TOOLS_CATALOG.map((t) => t.name)
    expect(names).not.toContain('decide')
    expect(names).not.toContain('design_decision')
    expect(names).not.toContain('check_gate')
  })

  test('核心工具目录覆盖全部核心工具，且无重名', () => {
    const names = BUILTIN_TOOLS_CATALOG.map((t) => t.name)
    expect(new Set(names).size).toBe(names.length)
    // 抽样钉住几个关键核心工具仍在目录里
    for (const key of ['read_file', 'write_file', 'run_command', 'invoke_subagent', 'Skill']) {
      expect(names).toContain(key)
    }
  })

  test('目录里每条都有标签与说明（插件管理页直接渲染）', () => {
    for (const info of BUILTIN_TOOLS_CATALOG) {
      expect(info.label.length).toBeGreaterThan(0)
      expect(info.description.length).toBeGreaterThan(0)
      expect(typeof info.isReadOnly).toBe('boolean')
    }
  })
})
