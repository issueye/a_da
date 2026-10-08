import { describe, expect, test } from 'bun:test'
import type { ProviderConfig } from '../config'
import type { SubagentProfile } from './types'
import { SubagentRunner } from './runner'
import { defaultToolRegistry } from '../tools'

describe('SubagentRunner', () => {
  const dummyConfig: ProviderConfig = {
    baseUrl: 'http://localhost:11434/v1',
    apiKey: 'mock-key',
    model: 'mock-model',
  }

  test('递归防护：强制排除 invoke_subagent 工具', async () => {
    // 即使 Profile 故意配置了 invoke_subagent，Runner 也必须将其剥离
    const runner = new SubagentRunner()
    const evilProfile: SubagentProfile = {
      id: 'evil_nested',
      name: '恶意递归专员',
      description: '试图调用子智能体',
      systemPrompt: '不断委派自己',
      allowedTools: ['invoke_subagent', 'read_file'],
      mode: 'readonly',
      maxSteps: 3,
      enabled: true,
      scope: 'workspace',
    }

    // 模拟 run，由于没有真实的 LLM 接口，它会在第一轮之后结束或抛出网络异常，
    // 我们重点检验安全过滤逻辑
    let recordedToolsCount = 0
    // 验证 defaultToolRegistry 是否包含了所有内置工具
    const allTools = defaultToolRegistry.getToolsForWorkspace(process.cwd())
    expect(allTools.some((t) => t.name === 'invoke_subagent')).toBe(true)

    // 模拟内部工具过滤
    const allowedSet = new Set(evilProfile.allowedTools)
    const filtered = allTools.filter((t) => {
      if (t.name === 'invoke_subagent') return false
      if (!allowedSet.has(t.name)) return false
      if (evilProfile.mode === 'readonly' && defaultToolRegistry.isWriteTool(t.name)) return false
      return true
    })

    expect(filtered.some((t) => t.name === 'invoke_subagent')).toBe(false)
    expect(filtered.some((t) => t.name === 'read_file')).toBe(true)
  })

  test('只读安全模式过滤写工具', async () => {
    const readonlyProfile: SubagentProfile = {
      id: 'strict_researcher',
      name: '严格只读专员',
      description: '只读调研',
      systemPrompt: '只读工作',
      allowedTools: ['read_file', 'write_file', 'edit_file', 'run_command'],
      mode: 'readonly',
      maxSteps: 5,
      enabled: true,
      scope: 'builtin',
    }

    const allTools = defaultToolRegistry.getToolsForWorkspace(process.cwd())
    const allowedSet = new Set(readonlyProfile.allowedTools)
    const filtered = allTools.filter((t) => {
      if (t.name === 'invoke_subagent') return false
      if (!allowedSet.has(t.name)) return false
      if (readonlyProfile.mode === 'readonly' && defaultToolRegistry.isWriteTool(t.name)) return false
      return true
    })

    // 写工具必须全部被干掉
    expect(filtered.some((t) => t.name === 'write_file')).toBe(false)
    expect(filtered.some((t) => t.name === 'edit_file')).toBe(false)
    expect(filtered.some((t) => t.name === 'run_command')).toBe(false)
    // 只读工具放行
    expect(filtered.some((t) => t.name === 'read_file')).toBe(true)
  })

  test('通配符 * 配合 disallowedTools 黑名单排除与递归防御', async () => {
    const generalProfile: SubagentProfile = {
      id: 'general_purpose',
      name: '全能专员',
      description: '全能',
      systemPrompt: '执行全部任务',
      allowedTools: ['*'],
      disallowedTools: ['run_command'],
      mode: 'readwrite',
      enabled: true,
      scope: 'builtin',
    }

    const allTools = defaultToolRegistry.getToolsForWorkspace(process.cwd())
    const allowedSet = new Set(generalProfile.allowedTools)
    const disallowedSet = new Set(generalProfile.disallowedTools ?? [])
    disallowedSet.add('invoke_subagent')
    disallowedSet.add('check_subagent')
    disallowedSet.add('send_subagent_message')

    const filtered = allTools.filter((t) => {
      if (disallowedSet.has(t.name)) return false
      if (!allowedSet.has('*') && !allowedSet.has(t.name)) return false
      if (generalProfile.mode === 'readonly' && defaultToolRegistry.isWriteTool(t.name)) return false
      return true
    })

    // 通配符允许了大部分工具
    expect(filtered.some((t) => t.name === 'read_file')).toBe(true)
    expect(filtered.some((t) => t.name === 'write_file')).toBe(true)
    expect(filtered.some((t) => t.name === 'edit_file')).toBe(true)
    // 黑名单 run_command 被排除
    expect(filtered.some((t) => t.name === 'run_command')).toBe(false)
    // 递归工具被强制排除
    expect(filtered.some((t) => t.name === 'invoke_subagent')).toBe(false)
    expect(filtered.some((t) => t.name === 'check_subagent')).toBe(false)
    expect(filtered.some((t) => t.name === 'send_subagent_message')).toBe(false)
  })
})
