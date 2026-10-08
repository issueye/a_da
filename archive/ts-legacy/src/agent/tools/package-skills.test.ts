import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { ExtensionLoader } from './loader'
import { defaultSkillManager } from '../skills'
import { defaultToolRegistry } from './registry'

describe('复合插件包（归纳 Tools 与 Skills）系统测试', () => {
  let testWorkspace: string
  let loader: ExtensionLoader

  beforeEach(async () => {
    testWorkspace = await mkdtemp(join(tmpdir(), 'ada-pkg-test-'))
    loader = new ExtensionLoader()
  })

  afterEach(async () => {
    await rm(testWorkspace, { recursive: true, force: true }).catch(() => {})
  })

  test('支持复合插件包目录模式：同时归纳工具代码与专属技能规范，并实现联动启停', async () => {
    const extDir = join(testWorkspace, '.ada', 'extensions')
    const pkgDir = join(extDir, 'db_toolkit')
    const pkgSkillsDir = join(pkgDir, 'skills', 'db_migration')
    await mkdir(pkgSkillsDir, { recursive: true })

    // 1. 在插件包中编写工具入口 index.ts
    const toolCode = `
export default function(context: any) {
  context.registerTool({
    name: 'db_query',
    description: '执行安全的数据库结构与慢查询诊断',
    parameters: { type: 'object', properties: { sql: { type: 'string' } } },
    async execute(callId: string, args: any) {
      return { ok: true, output: 'query executed: ' + args.sql }
    }
  })
}
`
    await writeFile(join(pkgDir, 'index.ts'), toolCode, 'utf8')

    // 2. 在插件包中编写专属技能规范 SKILL.md
    const skillContent = `---
name: db-migration
description: 数据库迁移与安全审计技能
allowed-tools: db_query, read_file
---
# 数据库迁移操作指南
1. 先导出原有 Schema；
2. 执行 Dry-run 演练；
3. 校验唯一索引与锁表风险。
`
    await writeFile(join(pkgSkillsDir, 'SKILL.md'), skillContent, 'utf8')

    // 3. 执行插件扫描
    const plugins = await loader.scanPlugins(testWorkspace)
    const dbPkg = plugins.find((p) => p.name === 'db_toolkit')

    expect(dbPkg).toBeDefined()
    expect(dbPkg?.isPackage).toBe(true)
    expect(dbPkg?.enabled).toBe(true)

    // 验证工具已被成功扫描归纳
    expect(dbPkg?.tools.length).toBe(1)
    expect(dbPkg?.tools[0]?.name).toBe('db_query')

    // 验证专属技能已被成功归纳到插件项中
    expect(dbPkg?.skills.length).toBe(1)
    expect(dbPkg?.skills[0]?.name).toBe('db-migration')
    expect(dbPkg?.skills[0]?.scope).toBe('plugin')
    expect(dbPkg?.skills[0]?.pluginName).toBe('db_toolkit')
    expect(dbPkg?.skills[0]?.allowedTools).toEqual(['db_query', 'read_file'])

    // 4. 执行插件目录工具动态加载
    const loaded = await loader.loadExtensionsFromDir(extDir, testWorkspace, new Set(), 'workspace')
    expect(loaded).toContain('db_query')
    expect(defaultToolRegistry.getCustomTools().some((t) => t.name === 'db_query')).toBe(true)

    // 5. 验证联动禁用：禁用插件时，其内建技能随之联动停用
    await loader.togglePlugin(dbPkg!.id, false, testWorkspace)
    const afterDisabledSkills = await defaultSkillManager.scanSkills(testWorkspace)
    const disabledSkill = afterDisabledSkills.find((s) => s.name === 'db-migration')
    expect(disabledSkill).toBeDefined()
    expect(disabledSkill?.enabled).toBe(false)

    // 6. 验证联动启用：重新启用插件后，内建技能恢复启用
    await loader.togglePlugin(dbPkg!.id, true, testWorkspace)
    const afterEnabledSkills = await defaultSkillManager.scanSkills(testWorkspace)
    const reEnabledSkill = afterEnabledSkills.find((s) => s.name === 'db-migration')
    expect(reEnabledSkill?.enabled).toBe(true)

    // 清理注册的工具
    defaultToolRegistry.unregister('db_query')
  })
})
