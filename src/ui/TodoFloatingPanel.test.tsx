import { describe, expect, test } from 'bun:test'
import React from 'react'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { connectTest } from '@gpuix/react/automation'
import type { Item } from '../agent/types'
import {
  getLatestTodoItem,
  parseTodosFromItem,
  TodoFloatingPanel,
} from './TodoFloatingPanel'
import { Transcript } from './Transcript'
import { store } from '../agent/store'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

describe('TodoFloatingPanel data extraction', () => {
  test('parseTodosFromItem parses todos from args or output', () => {
    const fromArgs: Extract<Item, { kind: 'tool' }> = {
      kind: 'tool',
      id: 'tool-1',
      at: 100,
      callId: 'call-1',
      name: 'todo',
      args: {
        todos: [
          { title: 'Step 1', status: 'completed' },
          { title: 'Step 2', status: 'in_progress' },
        ],
      },
      rawArgs: '',
      status: 'done',
    }
    const parsedArgs = parseTodosFromItem(fromArgs)
    expect(parsedArgs).toHaveLength(2)
    expect(parsedArgs![0].title).toBe('Step 1')
    expect(parsedArgs![0].status).toBe('completed')

    const fromOutput: Extract<Item, { kind: 'tool' }> = {
      kind: 'tool',
      id: 'tool-2',
      at: 200,
      callId: 'call-2',
      name: 'todo',
      args: {},
      rawArgs: '',
      status: 'done',
      output: JSON.stringify({
        todos: [{ title: 'Step from output', status: 'pending' }],
        notes: 'Some note',
      }),
    }
    const parsedOutput = parseTodosFromItem(fromOutput)
    expect(parsedOutput).toHaveLength(1)
    expect(parsedOutput![0].title).toBe('Step from output')

    const nonTodo: Extract<Item, { kind: 'tool' }> = {
      kind: 'tool',
      id: 'tool-3',
      at: 300,
      callId: 'call-3',
      name: 'read_file',
      args: { path: 'a.txt' },
      rawArgs: '',
      status: 'done',
    }
    expect(parseTodosFromItem(nonTodo)).toBeNull()
  })

  test('getLatestTodoItem finds newest todo in session items', () => {
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 10, text: 'Hello' },
      {
        kind: 'tool',
        id: 't1',
        at: 20,
        callId: 'c1',
        name: 'todo',
        args: {
          todos: [{ title: 'Old step', status: 'pending' }],
          notes: 'Old notes',
        },
        rawArgs: '',
        status: 'done',
      },
      { kind: 'assistant', id: 'a1', at: 30, text: 'Working on it' },
      {
        kind: 'tool',
        id: 't2',
        at: 40,
        callId: 'c2',
        name: 'run_command',
        args: { command: 'ls' },
        rawArgs: '',
        status: 'done',
      },
      {
        kind: 'tool',
        id: 't3',
        at: 50,
        callId: 'c3',
        name: 'todo',
        args: {
          todos: [
            { title: 'New step 1', status: 'completed' },
            { title: 'New step 2', status: 'in_progress' },
          ],
          notes: 'New notes',
        },
        rawArgs: '',
        status: 'done',
      },
    ]

    const latest = getLatestTodoItem(items)
    expect(latest).not.toBeNull()
    expect(latest!.item.id).toBe('t3')
    expect(latest!.todos).toHaveLength(2)
    expect(latest!.todos[0].title).toBe('New step 1')
    expect(latest!.notes).toBe('New notes')
  })

  test('getLatestTodoItem returns null when items have no todos', () => {
    const items: Item[] = [
      { kind: 'user', id: 'u1', at: 10, text: 'Hello' },
      { kind: 'assistant', id: 'a1', at: 20, text: 'Hi' },
    ]
    expect(getLatestTodoItem(items)).toBeNull()
  })
})

describeNative('TodoFloatingPanel UI', () => {
  test('renders floating panel and toggles collapse state', async () => {
    const thread = store.active
    thread.items = [
      {
        kind: 'tool',
        id: 'todo-ui-1',
        at: Date.now(),
        callId: 'call-ui-1',
        name: 'todo',
        args: {
          todos: [
            { title: 'UI Step 1', status: 'completed' },
            { title: 'UI Step 2', status: 'in_progress' },
            { title: 'UI Step 3', status: 'pending' },
          ],
          notes: 'Test execution',
        },
        rawArgs: '',
        status: 'done',
      },
    ]

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ position: 'relative', width: 800, height: 600 }}>
        <TodoFloatingPanel store={store} />
      </div>,
    )
    const app = await connectTest(renderer)

    const screen = () => renderer.getPaintedText().join('\n')
    const painted = async (needle: string, timeoutMs = 10_000): Promise<void> => {
      const started = Date.now()
      while (Date.now() - started < timeoutMs) {
        if (screen().includes(needle)) return
        renderer.flush?.()
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
      throw new Error(`never painted ${needle}\n${screen()}`)
    }
    const gone = async (needle: string, timeoutMs = 10_000): Promise<void> => {
      const started = Date.now()
      while (Date.now() - started < timeoutMs) {
        if (!screen().includes(needle)) return
        renderer.flush?.()
        await new Promise((resolve) => setTimeout(resolve, 50))
      }
      throw new Error(`still paints ${needle}\n${screen()}`)
    }

    // 默认展开卡片
    await painted('任务规划步骤')
    expect(screen()).toContain('UI Step 1')
    expect(screen()).toContain('UI Step 2')
    expect(screen()).toContain('UI Step 3')

    // 点击收起按钮
    await app.getByTestId('collapse-todo-panel').click()
    await gone('任务规划步骤')
    await painted('1/3')

    // 点击展开药丸再次展开
    await app.getByTestId('todo-floating-pill').click()
    await painted('任务规划步骤')
    expect(screen()).toContain('UI Step 1')

    await app.close()
  }, 30_000)

  test('Transcript does not render todo tool card in chat message stream', async () => {
    const thread = store.active
    thread.items = [
      { kind: 'user', id: 'u-1', at: 1, text: '开始处理任务' },
      {
        kind: 'tool',
        id: 'todo-stream-1',
        at: 2,
        callId: 'call-stream-1',
        name: 'todo',
        args: {
          todos: [{ title: '隐藏在会话流外的步骤', status: 'in_progress' }],
        },
        rawArgs: '',
        status: 'done',
      },
      { kind: 'assistant', id: 'a-1', at: 3, text: '已制定计划，正在执行' },
    ]

    const { render, renderer } = createTestRoot({ width: 800, height: 600 })
    render(
      <div style={{ display: 'flex', flexDirection: 'column', position: 'relative', width: 800, height: 600 }}>
        <Transcript store={store} />
      </div>,
    )
    const app = await connectTest(renderer)

    // 会话流中不应出现包含 tool-todo-stream-1 的工具卡片
    expect(await app.getByTestId('tool-todo-stream-1').count()).toBe(0)
    expect(await app.getByTestId('tool-head-todo-stream-1').count()).toBe(0)

    // 会话流正常呈现用户和助理消息
    const text = renderer.getPaintedText().join('\n')
    expect(text).toContain('开始处理任务')
    expect(text).toContain('已制定计划，正在执行')

    // 悬浮框存在且显示该步骤
    expect(await app.getByTestId('todo-floating-card').count()).toBe(1)
    expect(text).toContain('隐藏在会话流外的步骤')

    await app.close()
  }, 30_000)
})
