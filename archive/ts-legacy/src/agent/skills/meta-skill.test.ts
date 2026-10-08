import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { SkillManager } from './manager'
import { createManageSkillTool } from './meta-skill'

describe('manage_skill 工具 CRUD 测试', () => {
  let testDir: string
  let manager: SkillManager

  beforeEach(async () => {
    testDir = await mkdtemp(join(tmpdir(), 'ada-meta-skill-test-'))
    manager = new SkillManager()
  })

  afterEach(async () => {
    await rm(testDir, { recursive: true, force: true }).catch(() => {})
  })

  test('支持完整的 create -> read -> update -> list -> delete 闭环', async () => {
    const tool = createManageSkillTool(manager, testDir)

    // 1. 创建技能
    const createRes = await tool.execute('call-create', {
      action: 'create',
      name: 'fast-api',
      description: 'FastAPI 规范生成器',
      whenToUse: '当需要构建 Python RESTful 接口时',
      scope: 'workspace',
      body: '## 步骤\n1. 定义 Pydantic 模型；\n2. 挂载路由。',
      workspace: testDir,
    })
    expect(createRes.ok).toBe(true)
    expect(createRes.output).toContain('成功创建技能 "fast-api"')

    // 2. 读取技能
    const readRes = await tool.execute('call-read', {
      action: 'read',
      name: 'fast-api',
      workspace: testDir,
    })
    expect(readRes.ok).toBe(true)
    expect(readRes.output).toContain('FastAPI 规范生成器')
    expect(readRes.output).toContain('1. 定义 Pydantic 模型；')

    // 3. 更新技能
    const updateRes = await tool.execute('call-update', {
      action: 'update',
      name: 'fast-api',
      description: '企业级 FastAPI 规范生成器',
      body: '## 新步骤\n1. 增加 JWT 鉴权；\n2. 挂载路由。',
      workspace: testDir,
    })
    expect(updateRes.ok).toBe(true)
    expect(updateRes.output).toContain('成功更新技能 "fast-api"')

    // 再次读取确认更新生效
    const readUpdated = await tool.execute('call-read-2', {
      action: 'read',
      name: 'fast-api',
      workspace: testDir,
    })
    expect(readUpdated.output).toContain('企业级 FastAPI 规范生成器')
    expect(readUpdated.output).toContain('1. 增加 JWT 鉴权；')

    // 4. 列出技能（应包含 fast-api 与预置技能）
    const listRes = await tool.execute('call-list', {
      action: 'list',
      workspace: testDir,
    })
    expect(listRes.ok).toBe(true)
    expect(listRes.output).toContain('fast-api')
    expect(listRes.output).toContain('vibe-coding')

    // 5. 删除技能
    const deleteRes = await tool.execute('call-del', {
      action: 'delete',
      name: 'fast-api',
      workspace: testDir,
    })
    expect(deleteRes.ok).toBe(true)
    expect(deleteRes.output).toContain('成功删除技能 "fast-api"')

    // 确认已删除
    const readDeleted = await tool.execute('call-read-3', {
      action: 'read',
      name: 'fast-api',
      workspace: testDir,
    })
    expect(readDeleted.ok).toBe(false)
  })
})
