import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { SubagentManager } from './manager'

const TEST_DIR = join(import.meta.dir, 'tmp_manager_test')

describe('SubagentManager', () => {
  let manager: SubagentManager

  beforeEach(() => {
    rmSync(TEST_DIR, { recursive: true, force: true })
    mkdirSync(TEST_DIR, { recursive: true })
    manager = new SubagentManager()
  })

  afterEach(() => {
    rmSync(TEST_DIR, { recursive: true, force: true })
  })

  test('加载内置子智能体列表', async () => {
    const list = await manager.getSubagents()
    expect(list.length).toBeGreaterThanOrEqual(3)

    const researcher = list.find((a) => a.id === 'researcher')
    expect(researcher).toBeDefined()
    expect(researcher?.name).toBe('代码调研专员')
    expect(researcher?.mode).toBe('readonly')
    expect(researcher?.allowedTools).toContain('read_file')
    expect(researcher?.allowedTools).toContain('search_files')

    const reviewer = list.find((a) => a.id === 'code_reviewer')
    expect(reviewer).toBeDefined()
    expect(reviewer?.name).toBe('代码审查专家')

    const tester = list.find((a) => a.id === 'tester')
    expect(tester).toBeDefined()
    expect(tester?.mode).toBe('readwrite')
  })

  test('切换内置子智能体启用状态', async () => {
    const initial = await manager.getById('researcher')
    expect(initial?.enabled).toBe(true)

    await manager.toggleSubagent('researcher', false)
    const disabled = await manager.getById('researcher')
    expect(disabled?.enabled).toBe(false)

    // 恢复
    await manager.toggleSubagent('researcher', true)
    const reEnabled = await manager.getById('researcher')
    expect(reEnabled?.enabled).toBe(true)
  })

  test('从工作区目录加载自定义 JSON 子智能体', async () => {
    const workspaceSubagentsDir = manager.getWorkspaceDir(TEST_DIR)
    mkdirSync(workspaceSubagentsDir, { recursive: true })

    const customProfile = {
      name: '前端专员',
      description: '专注 React 与 UI 架构',
      systemPrompt: '你是一位前端专家',
      allowedTools: ['read_file', 'write_file'],
      mode: 'readwrite',
      maxSteps: 8,
      enabled: true,
    }

    writeFileSync(
      join(workspaceSubagentsDir, 'frontend.json'),
      JSON.stringify(customProfile),
      'utf8'
    )

    const list = await manager.getSubagents(TEST_DIR)
    const frontend = list.find((a) => a.name === '前端专员')
    expect(frontend).toBeDefined()
    expect(frontend?.scope).toBe('workspace')
    expect(frontend?.maxSteps).toBe(8)
    expect(frontend?.mode).toBe('readwrite')
  })

  test('保存与删除自定义子智能体', async () => {
    const custom = {
      id: 'custom_doc',
      name: '文档专员',
      description: '专注撰写技术文档',
      systemPrompt: '编写准确文档',
      allowedTools: ['read_file', 'write_file'],
      mode: 'readwrite' as const,
      maxSteps: 10,
      enabled: true,
      scope: 'workspace' as const,
    }

    await manager.saveSubagent(custom, TEST_DIR)

    const list = await manager.getSubagents(TEST_DIR)
    expect(list.some((a) => a.name === '文档专员')).toBe(true)

    const deleted = await manager.deleteSubagent('workspace_custom_doc', TEST_DIR)
    expect(deleted).toBe(true)

    const listAfter = await manager.getSubagents(TEST_DIR)
    expect(listAfter.some((a) => a.name === '文档专员')).toBe(false)
  })

  test('禁止删除内置子智能体', async () => {
    const deleted = await manager.deleteSubagent('researcher')
    expect(deleted).toBe(false)
    const researcher = await manager.getById('researcher')
    expect(researcher).toBeDefined()
  })

  test('内置全能执行专员配置与颜色标识', async () => {
    const general = await manager.getById('general_purpose')
    expect(general).toBeDefined()
    expect(general?.name).toBe('全能执行专员')
    expect(general?.mode).toBe('readwrite')
    expect(general?.color).toBe('blue')
    expect(general?.allowedTools).toContain('*')
    expect(general?.disallowedTools).toContain('invoke_subagent')
  })

  test('支持从 Markdown Frontmatter 解析 color 与 disallowedTools', async () => {
    const workspaceSubagentsDir = manager.getWorkspaceDir(TEST_DIR)
    mkdirSync(workspaceSubagentsDir, { recursive: true })

    const mdContent = `---
name: 性能优化专员
description: 专注算法性能与内存占用分析
mode: readonly
color: orange
tools: read_file, search_files
disallowedTools: write_file, edit_file
background: true
---
你是一位性能优化专家。`

    writeFileSync(join(workspaceSubagentsDir, 'perf.md'), mdContent, 'utf8')

    const list = await manager.getSubagents(TEST_DIR)
    const perf = list.find((a) => a.name === '性能优化专员')
    expect(perf).toBeDefined()
    expect(perf?.color).toBe('orange')
    expect(perf?.background).toBe(true)
    expect(perf?.allowedTools).toEqual(['read_file', 'search_files'])
    expect(perf?.disallowedTools).toEqual(['write_file', 'edit_file'])
  })

  test('formatProfilesPrompt 正确生成动态提示词与委派指南', async () => {
    const list = await manager.getSubagents()
    const { formatProfilesPrompt } = await import('./manager')
    const prompt = formatProfilesPrompt(list)

    expect(prompt).toContain('## 可委派子智能体列表')
    expect(prompt).toContain('general_purpose')
    expect(prompt).toContain('全能执行专员')
    expect(prompt).toContain('researcher')
    expect(prompt).toContain('代码调研专员')
    expect(prompt).toContain('## 委派准则与最佳实践')
    expect(prompt).toContain('自包含任务')
    expect(prompt).toContain('只取结论，不取大文本')
    expect(prompt).toContain('send_subagent_message')
  })
})
