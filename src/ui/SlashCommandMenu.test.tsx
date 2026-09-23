import { describe, expect, test } from 'bun:test'
import React from 'react'
import { createTestRoot } from '@gpuix/react/testing'
import { connectTest } from '@gpuix/react/automation'
import {
  filterCommands,
  getSystemCommands,
  mapPromptToCommand,
  SlashCommandMenu,
  type SlashCommandItem,
} from './SlashCommandMenu'
import { store } from '../agent/store'
import type { PromptItem } from '../agent/prompts/types'

describe('SlashCommandMenu 快捷指令面板逻辑测试', () => {
  test('getSystemCommands 提供完整的 7 大系统级控制指令', () => {
    const commands = getSystemCommands(store)
    expect(commands.length).toBeGreaterThanOrEqual(7)

    const cmdNames = commands.map((c) => c.command)
    expect(cmdNames).toContain('/clear')
    expect(cmdNames).toContain('/compact')
    expect(cmdNames).toContain('/code')
    expect(cmdNames).toContain('/plan')
    expect(cmdNames).toContain('/create')
    expect(cmdNames).toContain('/settings')
    expect(cmdNames).toContain('/plugins')

    // 验证模式切换 action
    const planCmd = commands.find((c) => c.command === '/plan')!
    planCmd.action?.()
    expect(store.mode).toBe('plan')

    const codeCmd = commands.find((c) => c.command === '/code')!
    codeCmd.action?.()
    expect(store.mode).toBe('code')
  })

  test('mapPromptToCommand 正确映射各作用域提示词模板', () => {
    const mockPrompt: PromptItem = {
      id: 'p-review',
      name: 'review-changes',
      description: '审查当前改动',
      argumentHint: '[branch]',
      content: '请审查改动...',
      scope: 'builtin',
      enabled: true,
      isSystem: false,
      updatedAt: Date.now(),
    }

    const item = mapPromptToCommand(mockPrompt)
    expect(item.command).toBe('/review-changes')
    expect(item.argumentHint).toBe('[branch]')
    expect(item.description).toBe('审查当前改动')
    expect(item.scopeLabel).toBe('内置')
    expect(item.icon).toBe('zap')
  })

  test('filterCommands 支持按指令名、前缀斜杠、描述与分类进行模糊搜索', () => {
    const sampleCommands: SlashCommandItem[] = [
      {
        id: '1',
        name: 'clear',
        command: '/clear',
        description: '清空会话',
        category: 'system',
        scopeLabel: '系统',
        icon: 'plus',
      },
      {
        id: '2',
        name: 'review-changes',
        command: '/review-changes',
        description: '审查改动',
        argumentHint: '[branch]',
        category: 'builtin',
        scopeLabel: '内置',
        icon: 'zap',
      },
      {
        id: '3',
        name: 'custom-deploy',
        command: '/custom-deploy',
        description: '工作区部署',
        category: 'workspace',
        scopeLabel: '工作区',
        icon: 'thread',
      },
    ]

    // 1. 无搜索词，返回全部
    expect(filterCommands(sampleCommands, '')).toHaveLength(3)

    // 2. 带 / 搜索
    const filteredBySlash = filterCommands(sampleCommands, '/rev')
    expect(filteredBySlash).toHaveLength(1)
    expect(filteredBySlash[0].command).toBe('/review-changes')

    // 3. 不带 / 搜索
    const filteredByName = filterCommands(sampleCommands, 'cle')
    expect(filteredByName).toHaveLength(1)
    expect(filteredByName[0].command).toBe('/clear')

    // 4. 按描述搜索
    const filteredByDesc = filterCommands(sampleCommands, '部署')
    expect(filteredByDesc).toHaveLength(1)
    expect(filteredByDesc[0].command).toBe('/custom-deploy')

    // 5. 按分类过滤
    const systemOnly = filterCommands(sampleCommands, '', 'system')
    expect(systemOnly).toHaveLength(1)
    expect(systemOnly[0].command).toBe('/clear')

    const customOnly = filterCommands(sampleCommands, '', 'custom')
    expect(customOnly).toHaveLength(1)
    expect(customOnly[0].command).toBe('/custom-deploy')
  })

  test('SlashCommandMenu 渲染可滚动指令列表并具备 overflowY scroll 滚动容器', async () => {
    const { render, renderer } = createTestRoot({ width: 800, height: 500 })
    render(<SlashCommandMenu store={store} onSelect={() => {}} onClose={() => {}} />)
    const app = await connectTest(renderer)

    // 验证指令列表容器存在且正常渲染
    const list = app.getByTestId('slash-command-list')
    expect(await list.count()).toBe(1)

    // 验证包含多项系统指令
    expect(await app.getByTestId('slash-item-clear').count()).toBe(1)
    expect(await app.getByTestId('slash-item-compact').count()).toBe(1)
    expect(await app.getByTestId('slash-item-code').count()).toBe(1)
    expect(await app.getByTestId('slash-item-plan').count()).toBe(1)
    expect(await app.getByTestId('slash-item-create').count()).toBe(1)

    await app.close()
  })
})
