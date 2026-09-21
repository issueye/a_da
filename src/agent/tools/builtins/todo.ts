/**
 * 任务规划与待办列表工具 (todo / plan)
 * 对齐 ZCode 任务分解与待办进度跟踪设计
 */

import type { AgentTool, AgentToolResult } from '../../core/types'

export interface TodoStep {
  id?: string
  title: string
  status: 'pending' | 'in_progress' | 'completed'
}

export interface TodoToolArgs {
  todos: TodoStep[]
  notes?: string
}

export function createTodoTool(): AgentTool<TodoToolArgs> {
  return {
    name: 'todo',
    label: '任务规划',
    description: '创建或更新任务规划与待办列表。在执行复杂的多步任务前制定步骤，或在完成关键步骤后更新其状态。',
    parameters: {
      type: 'object',
      properties: {
        todos: {
          type: 'array',
          description: '待办任务步骤列表',
          items: {
            type: 'object',
            properties: {
              id: { type: 'string', description: '步骤标识（可选）' },
              title: { type: 'string', description: '步骤标题描述' },
              status: {
                type: 'string',
                enum: ['pending', 'in_progress', 'completed'],
                description: '任务状态：pending (待处理), in_progress (进行中), completed (已完成)',
              },
            },
            required: ['title', 'status'],
          },
        },
        notes: {
          type: 'string',
          description: '补充备注（可选）',
        },
      },
      required: ['todos'],
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      const todos = Array.isArray(args.todos) ? args.todos : []
      const completed = todos.filter((t) => t.status === 'completed').length
      const active = todos.find((t) => t.status === 'in_progress')
      const summary = `已完成 ${completed}/${todos.length} 项${active ? `，当前：${active.title}` : ''}`
      return {
        ok: true,
        output: JSON.stringify({ todos, notes: args.notes, summary }, null, 2),
        details: { todos, completed, total: todos.length },
      }
    },
  }
}
