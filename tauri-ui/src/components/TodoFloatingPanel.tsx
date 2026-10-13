import React, { useState } from 'react'
import {
  ListTodo,
  CheckCircle2,
  Circle,
  ArrowRight,
  Copy,
  Check,
  ChevronDown,
  ChevronUp,
} from 'lucide-react'
import type { Item, TodoStep } from '../types'
import { deriveTodoPlan } from '../utils/todo-plan'

/** 从工具项中解析出 Todo 规划列表 */
export function parseTodosFromItem(item: Item): TodoStep[] | null {
  if (item.name !== 'todo' && item.tool !== 'todo') return null

  // `todo` 工具的真实参数是 `{action:"set"|"get"|"update", items:[{id,title,done}]}`，
  // 而这里早先只认 `args.todos` / `output.todos` —— 于是**永远解析不到**，悬浮面板
  // 一次都没出现过（实测：卡片显示"任务规划"，右上角却什么都没有）。
  const normalize = (raw: any): TodoStep[] | null => {
    if (!Array.isArray(raw) || raw.length === 0) return null
    return raw.map((t: any) => {
      const status: TodoStep['status'] =
        t?.status === 'completed' || t?.status === 'in_progress' || t?.status === 'pending'
          ? t.status
          : t?.done === true
          ? 'completed'
          : 'pending'
      return {
        id: typeof t?.id === 'string' ? t.id : undefined,
        title: String(t?.title ?? t?.text ?? ''),
        status,
      }
    })
  }

  // 1. 参数对象里的 items / todos（模型直传）
  const args: any = item.args
  if (args && typeof args === 'object') {
    const fromItems = normalize(args.items)
    if (fromItems) return fromItems
    const fromTodos = normalize(args.todos)
    if (fromTodos) return fromTodos
  }
  // 参数可能是 JSON 字符串（历史数据 / 某些序列化路径）
  if (typeof args === 'string' && args.trim().startsWith('{')) {
    try {
      const parsed = JSON.parse(args)
      const fromItems = normalize(parsed?.items)
      if (fromItems) return fromItems
      const fromTodos = normalize(parsed?.todos)
      if (fromTodos) return fromTodos
    } catch {}
  }

  // 2. 输出/回执里的 items / todos
  if (typeof item.output === 'string' && item.output.trim().startsWith('{')) {
    try {
      const parsed = JSON.parse(item.output)
      const fromItems = normalize(parsed?.items)
      if (fromItems) return fromItems
      const fromTodos = normalize(parsed?.todos)
      if (fromTodos) return fromTodos
    } catch {}
  }
  const res: any = item.result
  if (res && typeof res === 'object') {
    const fromItems = normalize(res.items)
    if (fromItems) return fromItems
    const fromTodos = normalize(res.todos)
    if (fromTodos) return fromTodos
  }

  return null
}

/** 倒序检索会话历史，获取最新的一份任务规划步骤 */
export function getLatestTodoItem(items: Item[]): {
  item: Item
  todos: TodoStep[]
  notes?: string
} | null {
  if (!items || items.length === 0) return null

  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i]
    if (it.name === 'todo' || it.tool === 'todo') {
      const todos = parseTodosFromItem(it)
      if (todos && todos.length > 0) {
        let notes: string | undefined
        if (typeof it.args?.notes === 'string') {
          notes = it.args.notes
        } else if (it.output) {
          try {
            const parsed = JSON.parse(it.output)
            if (typeof parsed?.notes === 'string') notes = parsed.notes
          } catch {}
        }
        return { item: it, todos, notes }
      }
    }

    // 处理内部内嵌的多工具调用（同样认 `items` —— `todo` 工具的真实参数名）
    if (Array.isArray(it.toolCalls)) {
      for (const call of it.toolCalls) {
        if (call.name !== 'todo') continue
        const params: any = call.params || (call as any).arguments
        const raw = Array.isArray(params?.items)
          ? params.items
          : Array.isArray(params?.todos)
          ? params.todos
          : null
        if (raw) {
          const todos = raw.map((t: any) => ({
            id: typeof t?.id === 'string' ? t.id : undefined,
            title: String(t?.title ?? t?.text ?? ''),
            status:
              t?.status === 'completed' || t?.status === 'in_progress' || t?.status === 'pending'
                ? t.status
                : t?.done === true
                ? 'completed'
                : 'pending',
          })) as TodoStep[]
          return { item: it, todos, notes: params?.notes }
        }
      }
    }
  }

  return null
}

export interface TodoFloatingPanelProps {
  items: Item[]
}

/**
 * 任务规划步骤独立收缩悬浮框组件
 * 悬浮在会话区右上角，不在主消息流中穿插，保持会话区整洁。
 */
export const TodoFloatingPanel: React.FC<TodoFloatingPanelProps> = ({ items }) => {
  const [collapsed, setCollapsed] = useState(false)
  const [copied, setCopied] = useState(false)

  // 计划必须**按动作推演**（set 替换 / update 增量 / get 只读），而不是"倒序找第一个能
  // 解析出 items 的卡片"——后者会把 update 的子集、get 回的旧快照当成最新计划，
  // 于是过了几轮面板就停在旧计划上（实测 7/12 而正文写着 13/13 完成）。
  const plan = deriveTodoPlan(items)
  if (!plan) return null

  const { todos, notes } = plan
  const completedCount = todos.filter((t) => t.status === 'completed').length
  const allDone = completedCount === todos.length && todos.length > 0
  const activeStep =
    todos.find((t) => t.status === 'in_progress') ?? todos.find((t) => t.status !== 'completed')

  // 一键复制 Markdown 任务清单
  const handleCopy = (e: React.MouseEvent) => {
    e.stopPropagation()
    const markdown = [
      `### 任务规划步骤 (${completedCount}/${todos.length})`,
      ...(notes ? [`> 备注：${notes}`] : []),
      ...todos.map((t) => `- [${t.status === 'completed' ? 'x' : ' '}] ${t.title}`),
    ].join('\n')

    navigator.clipboard.writeText(markdown).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    })
  }

  // 1. 收缩态：极小占用空间的悬浮胶囊药丸
  if (collapsed) {
    return (
      <div
        role="button"
        tabIndex={0}
        onClick={() => setCollapsed(false)}
        className="absolute top-2.5 right-4 z-30 flex items-center space-x-2 h-7 px-2.5 rounded-full bg-white/95 dark:bg-[#1f1f23]/95 hover:bg-zinc-100 dark:hover:bg-zinc-800 border border-zinc-200 dark:border-zinc-700/80 shadow-md backdrop-blur-md cursor-pointer transition-all animate-in fade-in zoom-in-95 duration-150 select-none"
        title="点击展开任务规划卡片"
      >
        <ListTodo size={13} className={allDone ? 'text-emerald-500' : 'text-blue-500'} />
        <span className="text-xs font-semibold text-zinc-800 dark:text-zinc-200">任务规划</span>
        <span
          className={`px-1.5 py-0.2 rounded-full text-[10px] font-mono font-semibold ${
            allDone
              ? 'bg-emerald-100 dark:bg-emerald-950/60 text-emerald-700 dark:text-emerald-400'
              : 'bg-blue-100 dark:bg-blue-950/60 text-blue-700 dark:text-blue-400'
          }`}
        >
          {completedCount}/{todos.length}
        </span>
        {activeStep && !allDone && (
          <span className="text-[11px] text-zinc-500 dark:text-zinc-400 max-w-[130px] truncate">
            {activeStep.title}
          </span>
        )}
        <ChevronDown size={12} className="text-zinc-400 dark:text-zinc-500" />
      </div>
    )
  }

  // 2. 展开态：任务规划悬浮卡片
  return (
    <div className="absolute top-2.5 right-4 z-30 w-80 md:w-96 max-w-[calc(100vw-2rem)] bg-white/95 dark:bg-[#1a1a1e]/95 border border-zinc-200 dark:border-zinc-700/80 rounded-xl shadow-xl backdrop-blur-md p-3 select-none flex flex-col space-y-2 animate-in fade-in slide-in-from-top-1 duration-150">
      {/* 顶部标题行 */}
      <div className="flex items-center justify-between pb-1.5 border-b border-zinc-100 dark:border-zinc-800">
        <div className="flex items-center space-x-2 min-w-0">
          <ListTodo size={14} className={allDone ? 'text-emerald-500' : 'text-blue-500'} />
          <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100 truncate">
            任务规划步骤
          </span>
          <span
            className={`px-1.5 py-0.2 rounded-full text-[10px] font-mono font-medium ${
              allDone
                ? 'bg-emerald-100 dark:bg-emerald-950/60 text-emerald-700 dark:text-emerald-400'
                : 'bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-300'
            }`}
          >
            {completedCount} / {todos.length} 已完成
          </span>
        </div>

        {/* 顶部操作：复制与收起 */}
        <div className="flex items-center space-x-1 flex-shrink-0">
          <button
            type="button"
            onClick={handleCopy}
            className={`flex items-center space-x-1 px-1.5 py-0.5 rounded text-[11px] transition-colors cursor-pointer ${
              copied
                ? 'bg-emerald-100 dark:bg-emerald-950/50 text-emerald-700 dark:text-emerald-400'
                : 'hover:bg-zinc-100 dark:hover:bg-zinc-800 text-zinc-500 dark:text-zinc-400'
            }`}
            title="复制 Markdown 格式任务清单"
          >
            {copied ? <Check size={11} /> : <Copy size={11} />}
            <span>{copied ? '已复制' : '复制'}</span>
          </button>

          <button
            type="button"
            onClick={() => setCollapsed(true)}
            className="flex items-center space-x-0.5 px-1.5 py-0.5 rounded text-[11px] hover:bg-zinc-100 dark:hover:bg-zinc-800 text-zinc-500 dark:text-zinc-400 transition-colors cursor-pointer"
            title="收起为悬浮药丸"
          >
            <ChevronUp size={12} />
            <span>收起</span>
          </button>
        </div>
      </div>

      {/* 补充备注信息 */}
      {notes && (
        <div className="px-2.5 py-1.5 bg-zinc-50 dark:bg-zinc-900/60 rounded-lg text-[11px] text-zinc-600 dark:text-zinc-400 leading-relaxed border border-zinc-100 dark:border-zinc-800/80">
          <span className="font-medium text-zinc-700 dark:text-zinc-300">备注：</span>
          {notes}
        </div>
      )}

      {/* 步骤条目列表 */}
      <div className="space-y-1 max-h-64 overflow-y-auto pr-1">
        {todos.map((step, idx) => {
          const isDone = step.status === 'completed'
          const isRunning = step.status === 'in_progress'

          return (
            <div
              key={step.id || idx}
              className={`flex items-start space-x-2 p-1.5 rounded-lg transition-colors text-xs ${
                isRunning
                  ? 'bg-blue-50/70 dark:bg-blue-950/30 text-blue-900 dark:text-blue-200 font-medium border border-blue-200/50 dark:border-blue-800/30'
                  : isDone
                  ? 'text-zinc-400 dark:text-zinc-500 hover:bg-zinc-50 dark:hover:bg-zinc-900/30'
                  : 'text-zinc-700 dark:text-zinc-300 hover:bg-zinc-50 dark:hover:bg-zinc-900/30'
              }`}
            >
              <div className="pt-0.5 flex-shrink-0">
                {isDone ? (
                  <CheckCircle2 size={13} className="text-emerald-500" />
                ) : isRunning ? (
                  <ArrowRight size={13} className="text-blue-500 animate-pulse" />
                ) : (
                  <Circle size={13} className="text-zinc-400 dark:text-zinc-600" />
                )}
              </div>
              <span
                className={`flex-1 leading-relaxed ${
                  isDone ? 'line-through decoration-zinc-300 dark:decoration-zinc-600' : ''
                }`}
              >
                {step.title}
              </span>
            </div>
          )
        })}
      </div>
    </div>
  )
}
