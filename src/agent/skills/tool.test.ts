import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { SkillManager } from './manager'
import { createSkillTool } from './tool'

describe('Skill 工具执行测试', () => {
  let testDir: string
  let manager: SkillManager

  beforeEach(async () => {
    testDir = await mkdtemp(join(tmpdir(), 'ada-skill-tool-test-'))
    manager = new SkillManager()
  })

  afterEach(async () => {
    await rm(testDir, { recursive: true, force: true }).catch(() => {})
  })

  test('Skill 工具正常加载技能正文与路径信息', async () => {
    await manager.createSkillTemplate({
      name: 'perf-audit',
      description: '性能分析与瓶颈排查技能',
      scope: 'workspace',
      workspaceRoot: testDir,
      body: '## 步骤\n1. 采集 CPU 与内存火焰图；\n2. 检查循环与递归深度。',
    })

    const tool = createSkillTool(manager, testDir)
    expect(tool.name).toBe('Skill')

    // 执行加载
    const result = await tool.execute('call-1', { skill: 'perf-audit', workspace: testDir })
    expect(result.ok).toBe(true)
    expect(result.output).toContain('<skill_content name="perf-audit">')
    expect(result.output).toContain('1. 采集 CPU 与内存火焰图；')
    expect(result.output).toContain('Base directory for this skill:')
    expect(result.output).toContain('</skill_content>')
  })

  test('Skill 工具针对不存在的技能给出友好错误与建议', async () => {
    const tool = createSkillTool(manager, testDir)
    const result = await tool.execute('call-2', { skill: 'non-existent', workspace: testDir })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('未找到名为 "non-existent" 的已启用技能')
  })
})
