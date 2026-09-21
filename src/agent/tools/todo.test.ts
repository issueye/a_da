import { describe, expect, test } from 'bun:test'
import { createTodoTool } from './builtins/todo'
import { defaultToolRegistry } from './registry'
import { describeTool } from '../tools'

describe('todo builtin tool', () => {
  test('creates todo tool with valid schema and label', () => {
    const tool = createTodoTool()
    expect(tool.name).toBe('todo')
    expect(tool.label).toBe('任务规划')
    expect(tool.parameters.properties).toHaveProperty('todos')
  })

  test('is registered in defaultToolRegistry and marked as read-only', () => {
    const tools = defaultToolRegistry.getToolsForWorkspace(process.cwd())
    const todoTool = tools.find((t) => t.name === 'todo')
    expect(todoTool).toBeDefined()
    expect(defaultToolRegistry.isWriteTool('todo')).toBe(false)
  })

  test('executes and computes task completion progress', async () => {
    const tool = createTodoTool()
    const result = await tool.execute('call_1', {
      todos: [
        { id: '1', title: '初始化架构', status: 'completed' },
        { id: '2', title: '编写核心代码', status: 'in_progress' },
        { id: '3', title: '编写测试', status: 'pending' },
      ],
    })

    expect(result.ok).toBe(true)
    const data = JSON.parse(result.output)
    expect(data.todos.length).toBe(3)
    expect(data.summary).toContain('1/3')
    expect(data.summary).toContain('编写核心代码')
    expect(result.details).toEqual({
      todos: [
        { id: '1', title: '初始化架构', status: 'completed' },
        { id: '2', title: '编写核心代码', status: 'in_progress' },
        { id: '3', title: '编写测试', status: 'pending' },
      ],
      completed: 1,
      total: 3,
    })
  })

  test('describeTool formats todo progress and current task', () => {
    const summary = describeTool('todo', {
      todos: [
        { id: '1', title: '第一步', status: 'completed' },
        { id: '2', title: '第二步进行中', status: 'in_progress' },
      ],
    })
    expect(summary).toBe('第二步进行中')

    const allDone = describeTool('todo', {
      todos: [
        { id: '1', title: '第一步', status: 'completed' },
        { id: '2', title: '第二步', status: 'completed' },
      ],
    })
    expect(allDone).toBe('2/2')
  })
})
