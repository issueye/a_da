/**
 * 内置子智能体的工具表守门测试。
 *
 * 这些用例存在的理由是一次真实的疏漏：批量读写插件（batch-ops）加进工具注册表后，
 * 子智能体仍然「步骤很多、用不上多文件读取」——因为每个内置子智能体的 allowedTools
 * 是一份**写死的白名单**，早于插件存在，注册了新工具也不会自动进入它们的工具表。
 * 同理，官方插件里真正只读的工具（git_status、get_outline…）没进 READ_ONLY 名单时，
 * 会被只读模式的 mode 过滤器当成写工具挡掉。
 *
 * 所以这里不复述白名单内容，而是**复刻 store 的真实过滤逻辑**跑一遍，钉住「子智能体
 * 实际拿得到的工具」。以后再加插件工具，白名单忘了跟进，这些用例就会红，而不是等到
 * 用起来才发现子智能体退化成一轮读一个文件。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { BUILTIN_SUBAGENTS } from './builtins'
import { defaultToolRegistry } from '../tools/registry'
import { defaultExtensionLoader } from '../tools/loader'
import type { AgentTool } from '../core/types'
import type { SubagentProfile } from './types'

/** 官方内置插件的工具（batch-ops 等）要经 autoLoadExtensions 注册后才在表里。 */
let workspace = ''

beforeAll(async () => {
  workspace = await mkdtemp(join(tmpdir(), 'ada-subagent-guard-'))
})

afterAll(async () => {
  await rm(workspace, { recursive: true, force: true })
})

/**
 * 复刻 store.startSubagentThread 里的工具过滤（白名单/黑名单/通配符/只读限制）。
 * 与那段实现保持一致是关键：这里算出来的必须就是子智能体真正能调用的工具。
 */
function toolsForProfile(profile: SubagentProfile, all: AgentTool[]): AgentTool[] {
  const allowed = new Set(profile.allowedTools)
  const disallowed = new Set(profile.disallowedTools ?? [])
  // store 里为防套娃额外加的两条
  disallowed.add('invoke_subagent')
  disallowed.add('check_subagent')
  disallowed.add('send_subagent_message')
  disallowed.add('resume_subagent')
  disallowed.add('await_subagents')

  return all.filter((tool) => {
    if (disallowed.has(tool.name)) return false
    if (!allowed.has('*') && !allowed.has(tool.name)) return false
    if (profile.mode === 'readonly' && defaultToolRegistry.isWriteTool(tool.name)) return false
    return true
  })
}

/** 加载官方内置插件后的完整工具表（子智能体实际是从这份里过滤的）。 */
async function fullToolTable(): Promise<AgentTool[]> {
  // notify_parent 不在通用表里（只对子智能体身份有意义），所以不必假装它在
  await defaultExtensionLoader.autoLoadExtensions(workspace)
  return defaultToolRegistry.getToolsForWorkspace(workspace)
}

describe('内置子智能体的工具表守门', () => {
  test('每个内置子智能体都拿得到 read_files（多文件读取）', async () => {
    const all = await fullToolTable()
    for (const profile of BUILTIN_SUBAGENTS) {
      const names = toolsForProfile(profile, all).map((t) => t.name)
      expect({ id: profile.id, hasReadFiles: names.includes('read_files') }).toEqual({
        id: profile.id,
        hasReadFiles: true,
      })
    }
  })

  test('每个内置子智能体都拿得到 Skill（否则加载不了 batch-efficiency 等技能）', async () => {
    const all = await fullToolTable()
    for (const profile of BUILTIN_SUBAGENTS) {
      const names = toolsForProfile(profile, all).map((t) => t.name)
      expect({ id: profile.id, hasSkill: names.includes('Skill') }).toEqual({
        id: profile.id,
        hasSkill: true,
      })
    }
  })

  test('只读子智能体不被 mode 过滤器误拦：只读工具确实留得住', async () => {
    const all = await fullToolTable()
    // plan 模式下这些也不该被当成写工具
    const readOnlyPlugins = ['git_status', 'git_diff', 'git_log', 'get_outline', 'inspect_project', 'find_symbol', 'read_files']
    for (const name of readOnlyPlugins) {
      expect({ name, isWrite: defaultToolRegistry.isWriteTool(name) }).toEqual({ name, isWrite: false })
    }

    const researcher = BUILTIN_SUBAGENTS.find((p) => p.id === 'researcher')!
    const names = toolsForProfile(researcher, all).map((t) => t.name)
    // 这些是只读角色做调研的骨干，全都得在
    for (const expected of ['read_files', 'find_symbol', 'get_outline', 'git_diff', 'search_files']) {
      expect({ id: 'researcher', name: expected, present: names.includes(expected) }).toEqual({
        id: 'researcher',
        name: expected,
        present: true,
      })
    }
    // 只读角色不能拿到任何写工具
    expect(names).not.toContain('write_file')
    expect(names).not.toContain('edit_file')
    expect(names).not.toContain('edit_files')
    expect(names).not.toContain('run_command')
  })

  test('只读子智能体拿不到 run_test_focused（它执行测试命令，按写工具对待）', async () => {
    const all = await fullToolTable()
    const researcher = BUILTIN_SUBAGENTS.find((p) => p.id === 'researcher')!
    const names = toolsForProfile(researcher, all).map((t) => t.name)
    expect(defaultToolRegistry.isWriteTool('run_test_focused')).toBe(true)
    expect(names).not.toContain('run_test_focused')
  })

  test('tester 拿得到批量读写与精准测试工具', async () => {
    const all = await fullToolTable()
    const tester = BUILTIN_SUBAGENTS.find((p) => p.id === 'tester')!
    const names = toolsForProfile(tester, all).map((t) => t.name)
    for (const expected of ['read_files', 'edit_files', 'run_test_focused', 'get_outline']) {
      expect({ name: expected, present: names.includes(expected) }).toEqual({ name: expected, present: true })
    }
  })

  test('决策工具按角色最小授权', async () => {
    const all = await fullToolTable()
    const researcher = BUILTIN_SUBAGENTS.find((p) => p.id === 'researcher')!
    const reviewer = BUILTIN_SUBAGENTS.find((p) => p.id === 'code_reviewer')!
    const tester = BUILTIN_SUBAGENTS.find((p) => p.id === 'tester')!

    const researcherTools = toolsForProfile(researcher, all).map((t) => t.name)
    const reviewerTools = toolsForProfile(reviewer, all).map((t) => t.name)
    const testerTools = toolsForProfile(tester, all).map((t) => t.name)

    // 三个角色都能做类型化判定：归属/分类/分级是调研与审查的通用需求
    for (const [id, names] of [
      ['researcher', researcherTools],
      ['code_reviewer', reviewerTools],
      ['tester', testerTools],
    ] as const) {
      expect({ id, hasDecide: names.includes('decide') }).toEqual({ id, hasDecide: true })
    }

    // 只读角色：不得拿到写工具；这个描述里出现的 edit_files 必须不在表里
    expect(researcherTools).not.toContain('write_file')
    expect(researcherTools).not.toContain('edit_files')
    expect(reviewerTools).not.toContain('write_file')
    expect(reviewerTools).not.toContain('edit_files')

    // 审查者能自行探索判断维度
    expect(reviewerTools).toContain('design_decision')
    // 测试者能跑收尾门禁；它是 readwrite，所以批量编辑也在
    expect(testerTools).toContain('check_gate')
    expect(testerTools).toContain('edit_files')
  })

  test('子智能体一律拿不到嵌套委派与等待工具（防套娃）', async () => {
    const all = await fullToolTable()
    for (const profile of BUILTIN_SUBAGENTS) {
      const names = toolsForProfile(profile, all).map((t) => t.name)
      for (const banned of ['invoke_subagent', 'check_subagent', 'send_subagent_message', 'resume_subagent', 'await_subagents']) {
        expect({ id: profile.id, banned, present: names.includes(banned) }).toEqual({
          id: profile.id,
          banned,
          present: false,
        })
      }
    }
  })

  test('子智能体系统提示词引导使用批量工具（不是逐文件读法）', () => {
    for (const profile of BUILTIN_SUBAGENTS) {
      // 提示词里要提到批量读取（general_purpose 用 read_files，其余同理）
      expect({ id: profile.id, mentionsBatch: profile.systemPrompt.includes('read_files') }).toEqual({
        id: profile.id,
        mentionsBatch: true,
      })
    }
    // researcher 原来那句「最后使用 read_file 细读关键函数实现」的逐步读法应当已被取代
    const researcher = BUILTIN_SUBAGENTS.find((p) => p.id === 'researcher')!
    expect(researcher.systemPrompt).toContain('批量读取')
  })
})
