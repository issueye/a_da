import { describe, expect, test } from 'bun:test'
import { expandPromptTemplate, parseCommandArgs, substituteArgs } from './template'
import type { PromptItem } from './types'

describe('提示词模板引擎 (Prompt Template Engine)', () => {
  test('parseCommandArgs 正确解析带引号的命令行参数', () => {
    expect(parseCommandArgs('Button "click handler" \'outline style\'')).toEqual([
      'Button',
      'click handler',
      'outline style',
    ])
    expect(parseCommandArgs('')).toEqual([])
    expect(parseCommandArgs('single')).toEqual(['single'])
    expect(parseCommandArgs('  a   b   c  ')).toEqual(['a', 'b', 'c'])
  })

  test('substituteArgs 正确替换位置参数、全参数与默认值', () => {
    // 1. 位置参数 $1, $2
    expect(substituteArgs('创建组件 $1，属性：$2', ['Header', 'fixed'])).toBe(
      '创建组件 Header，属性：fixed'
    )

    // 2. 全参数 $@ 与 $ARGUMENTS
    expect(substituteArgs('任务描述：$@', ['优化', '前端', '首屏加载'])).toBe(
      '任务描述：优化 前端 首屏加载'
    )
    expect(substituteArgs('参数清单：$ARGUMENTS', ['a', 'b', 'c'])).toBe(
      '参数清单：a b c'
    )

    // 3. 默认值兜底 ${1:-default}
    expect(substituteArgs('输出 ${1:-5} 条建议', [])).toBe('输出 5 条建议')
    expect(substituteArgs('输出 ${1:-5} 条建议', ['10'])).toBe('输出 10 条建议')
    expect(substituteArgs('总结范围：${@:-全部模块}', [])).toBe('总结范围：全部模块')
    expect(substituteArgs('总结范围：${@:-全部模块}', ['认证', '支付'])).toBe('总结范围：认证 支付')

    // 4. 切片语法 ${@:N} 与 ${@:N:L}
    expect(substituteArgs('操作: $1，其余参数: ${@:2}', ['git', 'commit', '-m', 'test'])).toBe(
      '操作: git，其余参数: commit -m test'
    )
    expect(substituteArgs('提取中间项: ${@:2:2}', ['a', 'b', 'c', 'd'])).toBe(
      '提取中间项: b c'
    )
  })

  test('expandPromptTemplate 自动展开输入框中的 /指令模板', () => {
    const templates: PromptItem[] = [
      {
        id: 'workspace_review',
        name: 'review',
        description: '代码审查模板',
        content: '请深度审查 $1 的代码，重点关注性能与安全：${@:2:-所有分支}',
        scope: 'workspace',
        enabled: true,
        isSystem: false,
        updatedAt: 1000,
      },
      {
        id: 'builtin_commit',
        name: 'commit',
        description: '提交信息生成',
        content: '生成 Conventional Commits 消息：$@',
        scope: 'builtin',
        enabled: false, // 停用
        isSystem: false,
        updatedAt: 1000,
      },
    ]

    // 匹配并成功展开参数
    const expanded = expandPromptTemplate('/review "src/agent" --strict', templates)
    expect(expanded).toBe('请深度审查 src/agent 的代码，重点关注性能与安全：--strict')

    // 默认值展开
    const expandedDefault = expandPromptTemplate('/review "src/core"', templates)
    expect(expandedDefault).toBe('请深度审查 src/core 的代码，重点关注性能与安全：所有分支')

    // 停用模板不展开
    expect(expandPromptTemplate('/commit fix bug', templates)).toBe('/commit fix bug')

    // 未匹配的常规文本保持原样
    expect(expandPromptTemplate('常规提问 /help 不是指令开头', templates)).toBe(
      '常规提问 /help 不是指令开头'
    )
  })
})
