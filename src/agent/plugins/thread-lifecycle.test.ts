/**
 * 会话生命周期钩子（M3-7，设计文档 §6.7）。
 *
 * 覆盖开发计划的验收清单：
 * - `beforeThreadCreate` 的 `title` 建议被采纳；**空标题被忽略**（不产生无名会话）
 * - `beforeThreadCreate` 的 `data` 落进 `Thread.pluginData[pluginId]`，且**随会话持久化**
 * - `afterThreadCreate` 拿到的会话已就绪（能读到 id / workspace）
 * - `beforeThreadDelete` 阻止删除时**真的没删**，且理由可见；受 `allowThreadDeleteBlock` 约束
 * - 删除级联时**每个子会话的钩子各调一次**
 * - `archiveBeforeDelete` 在删除前留下副本
 * - `onThreadSwitch` 是纯通知，不改任何东西
 *
 * 另外钉住一条实现约定：**没有插件参与时 `deleteThread` 是同步完成的**——几十处调用点
 * 都不等它的返回值，这条不能破。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, rm, unlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { AgentStore } from '../store'
import { defaultExtensionLoader } from '../tools/loader'
import { clearLoadedPlugins } from '../plugins/registry'
import { defaultSessionManager } from '../session/manager'
import { composePluginHooks } from '../plugins/hook-runtime'
import { DEFAULT_PLUGIN_CAPABILITIES, type ResolvedPluginCapabilities } from '../config'

/** 会话文件的真实路径（目录是按工作区散列的，别在测试里猜）。 */
async function sessionPathOf(threadId: string, ws: string): Promise<string | undefined> {
  const sessions = await defaultSessionManager.listSessionsForWorkspace(ws)
  return sessions.find((item) => item.id === threadId)?.filePath
}
import type { LoadedPlugin } from '../plugins/types'

let workspace = ''

beforeEach(async () => {
  workspace = await mkdtemp(join(tmpdir(), 'ada-lifecycle-ws-'))
  await mkdir(join(workspace, '.ada', 'extensions'), { recursive: true })
})

afterEach(async () => {
  clearLoadedPlugins(workspace)
  await rm(workspace, { recursive: true, force: true }).catch(() => {})
})

const capabilities = (overrides: Partial<typeof DEFAULT_PLUGIN_CAPABILITIES> = {}): ResolvedPluginCapabilities => {
  const caps = { ...DEFAULT_PLUGIN_CAPABILITIES, ...overrides }
  return { capabilities: caps, forPlugin: () => caps, invalid: [] }
}

function pluginWith(hooks: LoadedPlugin['contributions']['hooks']): LoadedPlugin {
  return {
    manifest: { id: 'builtin:lifecycle-probe', name: '生命周期探针', description: '', scope: 'builtin' },
    contributions: { tools: [], hooks },
    declarative: true,
    status: 'ready',
    diagnostics: [],
  }
}

const createContext = {
  kind: 'main' as const,
  workspace: '/tmp/ws',
  isSubagent: false,
}

const deleteContext = {
  kind: 'main' as const,
  workspace: '/tmp/ws',
  threadId: 't1',
  title: '待删会话',
  isSubagent: false,
  cascaded: false,
}

describe('运行层：创建 / 删除 / 切换的合成规则', () => {
  test('多个插件都能建议标题与写数据：标题后者胜，数据按插件分键', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        pluginWith({ beforeThreadCreate: async () => ({ title: '第一个标题', data: { a: 1 } }) }),
        pluginWith({ beforeThreadCreate: async () => ({ title: '第二个标题', data: { b: 2 } }) }),
      ],
    })

    const result = await hooks.beforeThreadCreate!(createContext)
    expect(result?.title).toBe('第二个标题')
    expect(result?.data).toEqual({ 'builtin:lifecycle-probe': { b: 2 } })
  })

  test('空标题与纯空白被忽略（不产生无名会话）', async () => {
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [pluginWith({ beforeThreadCreate: async () => ({ title: '   ' }) })],
    })

    expect(await hooks.beforeThreadCreate!(createContext)).toBeUndefined()
  })

  test('删除：第一个拦下的即定稿（删除不可逆，没必要继续问）', async () => {
    let asked = 0
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities(),
      plugins: [
        pluginWith({
          beforeThreadDelete: async () => {
            asked += 1
            return { block: true, blockReason: '还有未提交的改动' }
          },
        }),
        pluginWith({
          beforeThreadDelete: async () => {
            asked += 1
            return undefined
          },
        }),
      ],
    })

    const result = await hooks.beforeThreadDelete!(deleteContext)
    expect(result?.block).toBe(true)
    expect(result?.blockReason).toBe('还有未提交的改动')
    expect(asked).toBe(1)
  })

  test('删除：allowThreadDeleteBlock 关掉时拦不住，但归档仍可要求', async () => {
    const traces: string[] = []
    const hooks = composePluginHooks({
      kind: 'main',
      capabilities: capabilities({ allowThreadDeleteBlock: false }),
      plugins: [
        pluginWith({
          beforeThreadDelete: async () => ({ block: true, blockReason: '不许删', archiveBeforeDelete: true }),
        }),
      ],
      trace: (message) => traces.push(message),
    })

    const result = await hooks.beforeThreadDelete!(deleteContext)
    expect(result?.block).toBeUndefined()
    expect(result?.archiveBeforeDelete).toBe(true)
    expect(traces.some((line) => line.includes('allowThreadDeleteBlock'))).toBe(true)
  })
})

describe('真实会话链路', () => {
  /** 在工作区放一个生命周期插件并加载。 */
  async function writePlugin(body: string): Promise<void> {
    await writeFile(
      join(workspace, '.ada', 'extensions', 'lifecycle.ts'),
      `export default {
  name: '生命周期探针',
  tools: [],
  hooks: { ${body} },
}
`,
      'utf-8'
    )
    await defaultExtensionLoader.autoLoadExtensions(workspace)
  }

  test('标题建议被采纳、pluginData 落进会话并写进落盘文件', async () => {
    await writePlugin(`
    beforeThreadCreate: async () => ({ title: '插件起的名字', data: { seen: 42 } }),
    afterThreadCreate: async (ctx) => {
      globalThis.__createHookSaw = { threadId: ctx.threadId, workspace: ctx.workspace, title: ctx.title }
    },
    `)

    const store = new AgentStore(workspace)
    const thread = store.newThread(workspace)

    // 钩子是异步落地的（newThread 保持同步），等它跑完
    const deadline = Date.now() + 3000
    while (Date.now() < deadline && thread.title !== '插件起的名字') {
      await new Promise((resolve) => setTimeout(resolve, 20))
    }

    expect(thread.title).toBe('插件起的名字')
    // 工作区插件的 id 是 `workspace:<文件名>`，不是 builtin
    expect(thread.pluginData).toEqual({ 'workspace:lifecycle.ts': { seen: 42 } })

    const saw = (globalThis as Record<string, unknown>).__createHookSaw as
      | { threadId: string; workspace: string; title: string }
      | undefined
    expect(saw?.threadId).toBe(thread.id)
    expect(saw?.workspace).toBe(workspace)

    // 持久化：写进会话文件的 header（重启后能读回来）
    const deadline2 = Date.now() + 3000
    let raw = ''
    while (Date.now() < deadline2) {
      const headerPath = await sessionPathOf(thread.id, workspace)
      if (headerPath && existsSync(headerPath)) {
        raw = await readFile(headerPath, 'utf-8')
        if (raw.includes('pluginData')) break
      }
      await new Promise((resolve) => setTimeout(resolve, 30))
    }
    expect(raw).toContain('workspace:lifecycle.ts')

    delete (globalThis as Record<string, unknown>).__createHookSaw
    await store.deleteThread(thread.id)
  })

  test('阻止删除时真的没删，理由是可见的；去掉插件后能删掉', async () => {
    await writePlugin(
      `beforeThreadDelete: async () => ({ block: true, blockReason: '插件要求保留这个会话' })`
    )

    const store = new AgentStore(workspace)
    const thread = store.newThread(workspace)
    const before = store.threads.length

    expect(await store.deleteThread(thread.id)).toBe('插件要求保留这个会话')
    expect(store.threads.length).toBe(before)
    expect(store.log.some((entry) => entry.text.includes('删除被插件拦下'))).toBe(true)

    // 去掉插件（用户停用它就是这种情况）：再删就该成功
    await unlink(join(workspace, '.ada', 'extensions', 'lifecycle.ts'))
    await defaultExtensionLoader.autoLoadExtensions(workspace)
    expect(await store.deleteThread(thread.id)).toBeNull()
    expect(store.threads.some((candidate) => candidate.id === thread.id)).toBe(false)
  })

  test('归档后删除：留下 *.jsonl.archived 副本', async () => {
    await writePlugin(`beforeThreadDelete: async () => ({ archiveBeforeDelete: true })`)

    const store = new AgentStore(workspace)
    const thread = store.newThread(workspace)
    // 等会话文件建立（newThread 异步落盘）
    const deadline = Date.now() + 3000
    let filePath: string | undefined
    while (Date.now() < deadline && !filePath) {
      filePath = await sessionPathOf(thread.id, workspace)
      if (!filePath) await new Promise((resolve) => setTimeout(resolve, 30))
    }
    expect(filePath).toBeDefined()

    expect(await store.deleteThread(thread.id)).toBeNull()
    expect(existsSync(`${filePath}.archived`)).toBe(true)
  })

  test('删除级联：每个子会话的钩子各调一次', async () => {
    await writePlugin(`
    beforeThreadDelete: async (ctx) => {
      globalThis.__deleted ??= []
      globalThis.__deleted.push(ctx.threadId)
      return undefined
    },
    `)

    const store = new AgentStore(workspace)
    const parent = store.newThread(workspace)
    // 造两个子会话（parentId 指向父会话）
    const childA = store.newThread(workspace)
    const childB = store.newThread(workspace)
    const mutable = store as unknown as { threads: Array<{ id: string; parentId?: string }> }
    for (const child of mutable.threads) {
      if (child.id === childA.id || child.id === childB.id) child.parentId = parent.id
    }

    await store.deleteThread(parent.id)

    const deleted = ((globalThis as Record<string, unknown>).__deleted ?? []) as string[]
    expect(deleted).toContain(parent.id)
    expect(deleted).toContain(childA.id)
    expect(deleted).toContain(childB.id)
    delete (globalThis as Record<string, unknown>).__deleted
  })

  test('onThreadSwitch 是纯通知：切了就通知，且不改变行为', async () => {
    await writePlugin(`
    onThreadSwitch: async (ctx) => {
      globalThis.__switched ??= []
      globalThis.__switched.push(ctx.threadId)
    },
    `)

    const store = new AgentStore(workspace)
    const first = store.newThread(workspace)
    const second = store.newThread(workspace)
    store.selectThread(first.id)

    const deadline = Date.now() + 3000
    const switched = () => ((globalThis as Record<string, unknown>).__switched ?? []) as string[]
    while (Date.now() < deadline && !switched().includes(first.id)) {
      await new Promise((resolve) => setTimeout(resolve, 20))
    }

    expect(switched()).toContain(first.id)
    expect(store.activeId).toBe(first.id)
    delete (globalThis as Record<string, unknown>).__switched
    await store.deleteThread(first.id)
    await store.deleteThread(second.id)
  })

  test('没有插件参与时 deleteThread 是同步删完的（几十处调用点不等它）', async () => {
    // 这个工作区里没有任何插件
    const store = new AgentStore(workspace)
    const thread = store.newThread(workspace)

    // 故意不 await：删除必须在返回 promise 之前就已经完成
    void store.deleteThread(thread.id)
    expect(store.threads.some((candidate) => candidate.id === thread.id)).toBe(false)
  })
})
