/**
 * 任务清单的成对点位（`beforeTodoUpdate` / `afterTodoUpdate`）。
 *
 * 清单是有状态延续的东西：它留在工具卡片里、被界面读出来显示成"当前进度"，也被模型在
 * 后续轮次里当作计划引用。所以它和轮次一样需要成对——事前能改、事后能核对。
 *
 * 这里验三层：
 * 1. **运行层**：改写清单、拦下即定稿、旁注拼接、没插件时点位整体缺席；
 * 2. **工具层**：钩子真的改变了工具结果（`details.todos` 与 output），拦下时 ok=false
 *    且理由是正常的工具结果（不是"执行失败"）；
 * 3. **等价性**：没有插件时输出与改动前**逐字节相同**。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DEFAULT_PLUGIN_CAPABILITIES, readPluginCapabilities, type ResolvedPluginCapabilities } from '../../config'
import type { AgentToolResult } from '../../core/types'
import { composePluginHooks } from '../../plugins/hook-runtime'
import { clearLoadedPlugins } from '../../plugins/registry'
import type { LoadedPlugin } from '../../plugins/types'
import { createTodoTool, diffTodos, type TodoStep } from './todo'

const capabilities = (): ResolvedPluginCapabilities => {
  const caps = { ...DEFAULT_PLUGIN_CAPABILITIES }
  return { capabilities: caps, forPlugin: () => caps, invalid: [] }
}

function pluginWith(hooks: LoadedPlugin['contributions']['hooks']): LoadedPlugin {
  return {
    manifest: { id: 'builtin:todo-probe', name: '清单探针', description: '', scope: 'builtin' },
    contributions: { tools: [], hooks },
    declarative: true,
    status: 'ready',
    diagnostics: [],
  }
}

const todo = (title: string, status: TodoStep['status'] = 'pending'): TodoStep => ({ title, status })

const context = {
  kind: 'main' as const,
  workspace: '/tmp/ws',
  todos: [todo('写测试'), todo('跑门禁')],
  previous: [] as TodoStep[],
}

describe('运行层：改写、拦下与旁注', () => {
  test('改写清单：后一个插件看到的是前一个改过的', async () => {
    const seen: string[][] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        pluginWith({
          beforeTodoUpdate: async (ctx) => {
            seen.push(ctx.todos.map((item) => item.title))
            return { todos: [...ctx.todos, todo('补验收标准')] }
          },
        }),
        pluginWith({
          beforeTodoUpdate: async (ctx) => {
            seen.push(ctx.todos.map((item) => item.title))
            return undefined
          },
        }),
      ],
    })

    const result = await hooks.beforeTodoUpdate!(context)
    expect(seen[0]).toEqual(['写测试', '跑门禁'])
    expect(seen[1]).toEqual(['写测试', '跑门禁', '补验收标准'])
    expect(result?.todos?.map((item) => item.title)).toEqual(['写测试', '跑门禁', '补验收标准'])
  })

  test('拦下即定稿：后续插件不再被问，理由与来源都带出来', async () => {
    let asked = 0
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        pluginWith({
          beforeTodoUpdate: async () => {
            asked += 1
            return { block: true, blockReason: '先把第 1 项做完再改计划' }
          },
        }),
        pluginWith({
          beforeTodoUpdate: async () => {
            asked += 1
            return { todos: [todo('不该被采纳')] }
          },
        }),
      ],
    })

    const result = await hooks.beforeTodoUpdate!(context)
    expect(result?.block).toBe(true)
    expect(result?.blockReason).toBe('先把第 1 项做完再改计划')
    expect(result?.by).toBe('builtin:todo-probe')
    expect(asked).toBe(1)
  })

  test('旁注按插件顺序拼接；没旁注就不返回东西', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        pluginWith({ afterTodoUpdate: async () => ({ appendNote: '第 3 项缺少验收标准' }) }),
        pluginWith({ afterTodoUpdate: async () => undefined }),
      ],
    })

    const result = await hooks.afterTodoUpdate!({
      kind: 'main',
      todos: [todo('写测试')],
      previous: [],
      changed: 1,
      reopened: [],
    })
    expect(result?.appendNote).toBe('第 3 项缺少验收标准')
  })

  test('没插件时这对点位整体缺席（零开销）', () => {
    const hooks = composePluginHooks({ kind: 'main', capabilities: capabilities(), plugins: [] })
    expect(hooks.beforeTodoUpdate).toBeUndefined()
    expect(hooks.afterTodoUpdate).toBeUndefined()
  })
})

describe('diffTodos：回执要能看出"被悄悄回滚的项"', () => {
  test('新增 / 删除 / 改状态都算变化，完成被改回未完成单列出来', () => {
    const previous = [todo('甲', 'completed'), todo('乙', 'pending')]
    const next = [todo('甲', 'pending'), todo('丙', 'in_progress')]

    const { changed, reopened } = diffTodos(previous, next)
    // 甲改了状态、乙没了、丙是新的 → 3 处变化
    expect(changed).toBe(3)
    expect(reopened).toEqual(['甲'])
  })

  test('完全没变时变化数为 0', () => {
    const list = [todo('甲', 'pending')]
    expect(diffTodos(list, [...list])).toEqual({ changed: 0, reopened: [] })
  })
})

describe('工具层：钩子真的改变了工具结果', () => {
  let workspace = ''

  beforeEach(async () => {
    workspace = await mkdtemp(join(tmpdir(), 'ada-todo-ws-'))
    await mkdir(join(workspace, '.ada', 'extensions'), { recursive: true })
  })

  afterEach(async () => {
    clearLoadedPlugins(workspace)
    await rm(workspace, { recursive: true, force: true }).catch(() => {})
  })

  /** 在工作区里放一个清单插件并加载，然后按真实入口造工具。 */
  async function toolWithPlugin(body: string, fileName = 'todo-probe.ts') {
    await writeFile(
      join(workspace, '.ada', 'extensions', fileName),
      `export default {
  name: '清单探针',
  tools: [],
  hooks: { ${body} },
}
`,
      'utf-8'
    )
    const { defaultExtensionLoader } = await import('../loader')
    await defaultExtensionLoader.autoLoadExtensions(workspace)
    return createTodoTool(workspace, 'thread-1')
  }

  test('改写清单：写进 details.todos 的就是插件改过的那份', async () => {
    const tool = await toolWithPlugin(
      `beforeTodoUpdate: async (ctx) => ({ todos: [...ctx.todos, { title: '补验收标准', status: 'pending' }] }),`
    )

    const result = (await tool.execute('c1', { todos: [todo('写测试')] })) as AgentToolResult<{
      todos: TodoStep[]
    }>
    expect(result.ok).toBe(true)
    expect(result.details?.todos.map((item) => item.title)).toEqual(['写测试', '补验收标准'])
    expect(result.output).toContain('补验收标准')
  })

  test('拦下：ok=false，理由是普通的工具结果（模型能据此改正）', async () => {
    const tool = await toolWithPlugin(
      `beforeTodoUpdate: async () => ({ block: true, blockReason: '没有验收标准之前不许改计划' }),`
    )

    const result = await tool.execute('c1', { todos: [todo('写测试')] })
    expect(result.ok).toBe(false)
    expect(result.output).toBe('没有验收标准之前不许改计划')
  })

  test('旁注跟在工具结果后面（模型读的是同一段输出）', async () => {
    const tool = await toolWithPlugin(
      `afterTodoUpdate: async (ctx) => ctx.reopened.length > 0
        ? ({ appendNote: '这些项被改回了未完成：' + ctx.reopened.join('、') })
        : undefined,`
    )

    // 第一次：建立基线
    await tool.execute('c1', { todos: [todo('写测试', 'completed')] })
    // 第二次：把已完成的项改回未完成
    const result = await tool.execute('c2', { todos: [todo('写测试', 'pending')] })

    expect(result.output).toContain('【任务清单旁注】')
    expect(result.output).toContain('这些项被改回了未完成：写测试')
  })

  test('等价性：没有插件时输出与改动前逐字节相同', async () => {
    // 这个工作区里没有插件
    const tool = createTodoTool(workspace, 'thread-plain')
    const todos = [todo('写测试', 'in_progress'), todo('跑门禁')]

    const result = await tool.execute('c1', { todos, notes: '备注' })

    expect(result.ok).toBe(true)
    expect(result.details).toEqual({ todos, completed: 0, total: 2 })
    expect(result.output).toBe(
      JSON.stringify(
        { todos, notes: '备注', summary: '已完成 0/2 项，当前：写测试' },
        null,
        2
      )
    )
  })
})
