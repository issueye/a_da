import { describe, expect, test } from 'bun:test'
import React from 'react'
import { createTestRoot } from '@gpuix/react/testing'
import { connectTest } from '@gpuix/react/automation'
import {
  filterMentions,
  getFileIcon,
  MentionMenu,
  type MentionItem,
} from './MentionMenu'
import { store } from '../agent/store'
import { agentClient } from './client'

describe('MentionMenu @ 提及面板逻辑与纯函数测试', () => {
  test('getFileIcon 能智能推断不同文件扩展名的图标', () => {
    expect(getFileIcon('src/index.ts')).toBe('code')
    expect(getFileIcon('agent/loop.rs')).toBe('code')
    expect(getFileIcon('assets/icon.png')).toBe('sparkles')
    expect(getFileIcon('docs/README.md')).toBe('file')
    expect(getFileIcon('unknown.xyz')).toBe('file')
  })

  test('filterMentions 纯函数按搜索词与分类精确过滤', () => {
    const sampleItems: MentionItem[] = [
      {
        id: 'file:src/ui/Composer.tsx',
        title: 'Composer.tsx',
        subtitle: 'src/ui',
        category: 'file',
        categoryLabel: '文件',
        icon: 'code',
        insertText: 'src/ui/Composer.tsx',
      },
      {
        id: 'file:README.md',
        title: 'README.md',
        category: 'file',
        categoryLabel: '文件',
        icon: 'file',
        insertText: 'README.md',
      },
      {
        id: 'skill:review',
        title: 'review',
        subtitle: '代码审查规范',
        category: 'skill',
        categoryLabel: '技能',
        icon: 'sparkles',
        insertText: 'skill:review',
      },
      {
        id: 'subagent:coder',
        title: 'coder',
        subtitle: '特化编码子智能体',
        category: 'subagent',
        categoryLabel: '智能体',
        icon: 'compass',
        insertText: 'subagent:coder',
      },
    ]

    // 1. 无搜索词，返回全部
    expect(filterMentions(sampleItems, '')).toHaveLength(4)

    // 2. 按文件名模糊搜索
    const compFiltered = filterMentions(sampleItems, 'comp')
    expect(compFiltered).toHaveLength(1)
    expect(compFiltered[0].title).toBe('Composer.tsx')

    // 3. 按副标题/描述模糊搜索
    const reviewFiltered = filterMentions(sampleItems, '代码审查')
    expect(reviewFiltered).toHaveLength(1)
    expect(reviewFiltered[0].title).toBe('review')

    // 4. 按分类筛选
    const filesOnly = filterMentions(sampleItems, '', 'file')
    expect(filesOnly).toHaveLength(2)

    const skillsOnly = filterMentions(sampleItems, '', 'skill')
    expect(skillsOnly).toHaveLength(1)
    expect(skillsOnly[0].title).toBe('review')

    const subagentsOnly = filterMentions(sampleItems, '', 'subagent')
    expect(subagentsOnly).toHaveLength(1)
    expect(subagentsOnly[0].title).toBe('coder')
  })

  test('MentionMenu 组件挂载并正确渲染头部与条目列表', async () => {
    const originalEntries = store.entries
    store.entries = ['src/index.ts', 'src/ui/Composer.tsx', 'README.md']

    const { render, renderer } = createTestRoot({ width: 800, height: 500 })
    render(
      <MentionMenu
        client={agentClient}
        filterQuery=""
        onSelect={() => {}}
        onClose={() => {}}
      />
    )
    const app = await connectTest(renderer)

    // 验证提及面板整体容器
    const menu = app.getByTestId('mention-menu')
    expect(await menu.count()).toBe(1)

    // 验证条目列表容器
    const list = app.getByTestId('mention-item-list')
    expect(await list.count()).toBe(1)

    // 验证分类 tab
    expect(await app.getByTestId('mention-tab-all').count()).toBe(1)
    expect(await app.getByTestId('mention-tab-file').count()).toBe(1)
    expect(await app.getByTestId('mention-tab-skill').count()).toBe(1)
    expect(await app.getByTestId('mention-tab-subagent').count()).toBe(1)

    // 验证包含工作区文件项
    expect(await app.getByTestId('mention-item-file-Composer.tsx').count()).toBe(1)

    await app.close()
    store.entries = originalEntries
  })
})
