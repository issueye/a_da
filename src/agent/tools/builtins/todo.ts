/**
 * 任务规划与待办列表工具 (todo / plan)
 * 对齐 ZCode 任务分解与待办进度跟踪设计
 */

import type { AgentTool, AgentToolResult } from '../../core/types'
import { readPluginCapabilities } from '../../config'
import { composePluginHooks } from '../../plugins/hook-runtime'
import { readPluginConfig } from '../../config'

export interface TodoStep {
  id?: string
  title: string
  status: 'pending' | 'in_progress' | 'completed'
}

export interface TodoToolArgs {
  todos: TodoStep[]
  notes?: string
}

/**
 * 上一次生效的清单，按会话记。
 *
 * 清单本身没有独立的持久化状态——它只活在工具卡片的 `details.todos` 里（界面读最后一张
 * 卡片）。但"事前/事后"这对钩子需要知道**上一次是什么**，否则 `afterTodoUpdate` 没法
 * 回答"实际生效的这份相对上次变了什么"。所以这里留一份内存副本：它只服务于钩子的回执，
 * 丢了也只影响一次回执，不影响清单本身。
 */
const lastTodosByThread = new Map<string, TodoStep[]>()

/** 把清单换成便于比较的指纹（标题 + 状态）。 */
function fingerprint(todos: TodoStep[]): string {
  return todos.map((todo) => `${todo.title}\u0000${todo.status}`).join('\u0001')
}

/**
 * 算出相对上次的变化：有变化的项数，以及"完成了又被改回未完成"的项。
 *
 * `reopened` 单独拎出来是因为它最值得被指出来——计划被悄悄回滚，是"任务清单"这类
 * 状态最容易出的问题。
 */
export function diffTodos(
  previous: TodoStep[],
  next: TodoStep[]
): { changed: number; reopened: string[] } {
  const before = new Map(previous.map((todo) => [todo.title, todo.status]))
  const after = new Map(next.map((todo) => [todo.title, todo.status]))

  let changed = 0
  const reopened: string[] = []
  for (const [title, status] of after) {
    if (!before.has(title)) {
      changed += 1
      continue
    }
    if (before.get(title) !== status) {
      changed += 1
      if (before.get(title) === 'completed' && status !== 'completed') reopened.push(title)
    }
  }
  for (const title of before.keys()) {
    if (!after.has(title)) changed += 1
  }
  return { changed, reopened }
}

export function createTodoTool(workspace?: string, threadId?: string): AgentTool<TodoToolArgs> {
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
      let todos = Array.isArray(args.todos) ? args.todos : []
      const key = threadId ?? workspace ?? '(未知)'
      const previous = lastTodosByThread.get(key) ?? []

      // 任务清单的成对点位（见 core/events.ts 的说明）：插件可以补上模型漏掉的验收项、
      // 拆掉过碎的步骤，也可以直接拦下。清单不是授权、不参与审批，所以替换不需要额外开关。
      const hooks = workspace
        ? await composePluginHooks({
            kind: 'main',
            workspace,
            threadId,
            capabilities: await readPluginCapabilities(workspace),
          })
        : {}

      let appendNote: string | undefined
      if (hooks.beforeTodoUpdate) {
        const verdict = await hooks.beforeTodoUpdate({
          kind: 'main',
          workspace,
          threadId,
          todos,
          previous,
          trace: (message) => console.warn(`[插件] ${message}`),
        })
        if (verdict?.block) {
          // 拦下不是"执行失败"：理由要像正常结果一样回给模型，它才知道该怎么改
          return {
            ok: false,
            output: verdict.blockReason ?? '这次任务清单更新被插件拦下了。',
            details: { todos: previous, blocked: true, pluginId: verdict.by },
          }
        }
        if (Array.isArray(verdict?.todos)) todos = verdict.todos
      }

      const completed = todos.filter((t) => t.status === 'completed').length
      const active = todos.find((t) => t.status === 'in_progress')
      const summary = `已完成 ${completed}/${todos.length} 项${active ? `，当前：${active.title}` : ''}`

      if (hooks.afterTodoUpdate) {
        const { changed, reopened } = diffTodos(previous, todos)
        const result = await hooks.afterTodoUpdate({
          kind: 'main',
          workspace,
          threadId,
          todos,
          previous,
          changed,
          reopened,
        })
        appendNote = result?.appendNote
      }

      lastTodosByThread.set(key, todos)
      // 旁注直接跟在工具结果后面：模型读到的是同一段输出，不需要另建交付通道
      const output = JSON.stringify({ todos, notes: args.notes, summary }, null, 2)
      return {
        ok: true,
        output: appendNote ? `${output}\n\n【任务清单旁注】\n${appendNote}` : output,
        details: { todos, completed, total: todos.length },
      }
    },
  }
}
