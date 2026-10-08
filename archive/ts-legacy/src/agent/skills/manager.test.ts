import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { defaultSkillManager, SkillManager } from './manager'
import { expandSkillVariables, parseSkillMarkdown } from './parser'

describe('SKILL 系统解析与变量替换', () => {
  test('parseSkillMarkdown 正确解析标准 YAML Frontmatter 与 Markdown 正文', () => {
    const raw = `---
name: code-review
description: 专业的代码审查技能，重点检查内存泄漏、并发竞争与设计模式
version: 1.0.0
author: ada-team
tags: [review, rust, safety]
whenToUse: 当用户请求审查代码或 PR 时
---

# 代码审查指引

1. 检查潜在空指针或野指针；
2. 确认资源是否及时释放。
`
    const parsed = parseSkillMarkdown(raw, '/workspace/.ada/skills/code-review/SKILL.md')
    expect(parsed.hasFrontmatter).toBe(true)
    expect(parsed.metadata.name).toBe('code-review')
    expect(parsed.metadata.description).toContain('专业的代码审查技能')
    expect(parsed.metadata.version).toBe('1.0.0')
    expect(parsed.metadata.author).toBe('ada-team')
    expect(parsed.metadata.tags).toEqual(['review', 'rust', 'safety'])
    expect(parsed.metadata.whenToUse).toContain('当用户请求审查代码')
    expect(parsed.body).toContain('# 代码审查指引')
    expect(parsed.body).toContain('1. 检查潜在空指针或野指针；')
  })

  test('expandSkillVariables 正确替换技能目录路径变量', () => {
    const content = '请查看辅助脚本目录：${ADA_SKILL_DIR}/scripts/check.sh 以及 ${CLAUDE_SKILL_DIR}/ref'
    const expanded = expandSkillVariables(content, '/home/user/.ada/skills/code-review')
    expect(expanded).toBe(
      '请查看辅助脚本目录：/home/user/.ada/skills/code-review/scripts/check.sh 以及 /home/user/.ada/skills/code-review/ref'
    )
  })
})

describe('SkillManager 技能扫描、提示词生成与生命周期管理', () => {
  let testDir: string
  let manager: SkillManager

  beforeEach(async () => {
    testDir = await mkdtemp(join(tmpdir(), 'ada-skills-test-'))
    manager = new SkillManager()
  })

  afterEach(async () => {
    await rm(testDir, { recursive: true, force: true }).catch(() => {})
  })

  test('创建工作区技能模板，扫描并生成技能提示词', async () => {
    // 1. 创建技能模板
    const createdPath = await manager.createSkillTemplate({
      name: 'sql-optimizer',
      description: '针对慢查询的 SQL 性能优化技能',
      scope: 'workspace',
      workspaceRoot: testDir,
      body: '## 优化规则\n1. 检查索引命中情况；\n2. 避免全表扫描。',
    })

    expect(createdPath).toContain('sql-optimizer')

    // 2. 扫描发现技能
    const skills = await manager.scanSkills(testDir)
    expect(skills.length).toBeGreaterThanOrEqual(1)
    const sqlSkill = skills.find((s) => s.name === 'sql-optimizer')
    expect(sqlSkill).toBeDefined()
    expect(sqlSkill?.scope).toBe('workspace')
    expect(sqlSkill?.description).toBe('针对慢查询的 SQL 性能优化技能')
    expect(sqlSkill?.enabled).toBe(true)

    // 3. 构建提示词
    const promptCtx = await manager.buildSkillsPrompt(testDir)
    expect(promptCtx.activatedSkillNames).toContain('sql-optimizer')
    expect(promptCtx.prompt).toContain('### 可用技能库')
    expect(promptCtx.prompt).toContain('sql-optimizer')
    expect(promptCtx.prompt).toContain('针对慢查询的 SQL 性能优化技能')

    // 4. 加载技能内容
    const loaded = await manager.loadSkillContent('sql-optimizer', testDir)
    expect(loaded).toBeDefined()
    expect(loaded?.name).toBe('sql-optimizer')
    expect(loaded?.content).toContain('1. 检查索引命中情况；')

    // 5. 切换停用状态
    await manager.toggleSkill(sqlSkill!.id, false)
    const enabledSkills = await manager.getEnabledSkills(testDir)
    expect(enabledSkills.find((s) => s.name === 'sql-optimizer')).toBeUndefined()

    // 6. 删除技能
    await manager.deleteSkill(sqlSkill!.id, testDir)
    const afterDelete = await manager.scanSkills(testDir)
    expect(afterDelete.find((s) => s.name === 'sql-optimizer')).toBeUndefined()
  })
})
