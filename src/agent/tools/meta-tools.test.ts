import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { ExtensionLoader } from './loader'
import { createManageTool } from './builtins/meta-tools'

describe('manage_tool 工具 CRUD 测试', () => {
  let testDir: string
  let loader: ExtensionLoader

  beforeEach(async () => {
    testDir = await mkdtemp(join(tmpdir(), 'ada-meta-tool-test-'))
    loader = new ExtensionLoader()
  })

  afterEach(async () => {
    await rm(testDir, { recursive: true, force: true }).catch(() => {})
  })

  test('支持自定义工具插件的 create -> read -> list -> delete 闭环', async () => {
    const tool = createManageTool(loader, testDir)

    // 1. 创建自定义工具
    const sampleCode = `
export default function (context: any) {
  context.registerTool({
    name: 'timestamp_now',
    description: '获取当前时间戳',
    parameters: { type: 'object', properties: {} },
    async execute() {
      return { output: String(Date.now()), ok: true }
    },
  })
}
`
    const createRes = await tool.execute('call-c1', {
      action: 'create',
      name: 'timestamp_tool',
      description: '生成当前时间戳的工具',
      scope: 'workspace',
      code: sampleCode,
      workspace: testDir,
    })
    expect(createRes.ok).toBe(true)
    expect(createRes.output).toContain('成功创建自定义工具插件 "timestamp_tool"')

    // 2. 读取插件
    const readRes = await tool.execute('call-r1', {
      action: 'read',
      name: 'timestamp_tool',
      workspace: testDir,
    })
    expect(readRes.ok).toBe(true)
    expect(readRes.output).toContain('timestamp_now')
    expect(readRes.output).toContain('TypeScript 插件源码')

    // 3. 列出插件
    const listRes = await tool.execute('call-l1', {
      action: 'list',
      workspace: testDir,
    })
    expect(listRes.ok).toBe(true)
    expect(listRes.output).toContain('timestamp_tool')

    // 4. 删除插件
    const deleteRes = await tool.execute('call-d1', {
      action: 'delete',
      name: 'timestamp_tool',
      workspace: testDir,
    })
    expect(deleteRes.ok).toBe(true)
    expect(deleteRes.output).toContain('成功删除工具插件')

    // 再次读取确认已不存在
    const readDeleted = await tool.execute('call-r2', {
      action: 'read',
      name: 'timestamp_tool',
      workspace: testDir,
    })
    expect(readDeleted.ok).toBe(false)
  })

  test('ToolRegistry.getToolsForMode 在不同模式下精准分发工具', () => {
    const { defaultToolRegistry } = require('./registry')

    // plan 模式：只读安全，不包含写工具
    const planTools = defaultToolRegistry.getToolsForMode(testDir, 'plan')
    const planToolNames = planTools.map((t: any) => t.name)
    expect(planToolNames).toContain('read_file')
    expect(planToolNames).toContain('list_files')
    expect(planToolNames).toContain('read_url_content')
    expect(planToolNames).toContain('Skill')
    expect(planToolNames).not.toContain('write_file')
    expect(planToolNames).not.toContain('edit_file')
    expect(planToolNames).not.toContain('run_command')

    // create 模式：激活 manage_tool 与 manage_skill 元工具
    const createTools = defaultToolRegistry.getToolsForMode(testDir, 'create')
    const createToolNames = createTools.map((t: any) => t.name)
    expect(createToolNames).toContain('manage_tool')
    expect(createToolNames).toContain('manage_skill')
    expect(createToolNames).toContain('write_file')

    // code 模式：默认不暴露 manage_tool，拥有常规读写工具
    const codeTools = defaultToolRegistry.getToolsForMode(testDir, 'code')
    const codeToolNames = codeTools.map((t: any) => t.name)
    expect(codeToolNames).toContain('write_file')
    expect(codeToolNames).toContain('run_command')
    expect(codeToolNames).not.toContain('manage_tool')
  })
})
