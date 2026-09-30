/**
 * 门禁在**真实委派链路**里的行为（M3-4）。
 *
 * 前面 `access.test.ts` 验的是判定表本身；这里验的是接进 store 之后最要紧的两件事：
 *
 * 1. **拦下的委派不会让父会话永久挂起**——门禁抛在建会话之前，所以既没有子会话，
 *    也没有进 `runningThreadIds`，父智能体拿到的是一条工具错误而不是一次悬挂；
 * 2. **`afterSubagentEnd` 抛错不阻断唤醒**——它是可选增强，不是唤醒链路上的一环。
 *
 * 为了不依赖真实插件，这里用一个临时工作区里的扩展来提供钩子：走的是完整加载路径
 * （描述符 → 加载器 → 运行层 → 委派）。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { AgentStore } from '../store'
import { defaultExtensionLoader } from '../tools/loader'
import { clearLoadedPlugins } from '../plugins/registry'

let workspace = ''

beforeEach(async () => {
  workspace = await mkdtemp(join(tmpdir(), 'ada-gate-ws-'))
  await mkdir(join(workspace, '.ada', 'extensions'), { recursive: true })
})

afterEach(async () => {
  clearLoadedPlugins(workspace)
  await rm(workspace, { recursive: true, force: true }).catch(() => {})
})

/**
 * 在工作区里放一个带门禁钩子的插件。
 *
 * `mode` 决定它怎么判：拒绝（不含校准信息）/ 拒绝（含校准）/ 通过 / 抛错。
 */
async function writeGatePlugin(
  mode: 'deny' | 'deny-raw' | 'allow' | 'throw',
  fileName = 'gate-probe.ts'
): Promise<void> {
  const body =
    mode === 'deny-raw'
      ? `return { allowed: true, reason: '我说通过' }`
      : mode === 'deny'
        ? `return { allowed: false, confidence: 0.2, calibrated: false, reason: '任务描述里没有验收标准' }`
        : mode === 'allow'
          ? `return { allowed: true, confidence: 0.9, calibrated: false, reason: '满足标准' }`
          : `throw new Error('判定服务不可达')`

  await writeFile(
    join(workspace, '.ada', 'extensions', fileName),
    `globalThis.__gate_module_evals = (globalThis.__gate_module_evals ?? 0) + 1
export default {
  name: '门禁探针',
  tools: [],
  hooks: {
    beforeSubagentStart: async (ctx) => {
      globalThis.__gate_inputs = [...(globalThis.__gate_inputs ?? []), ctx.task]
      ctx.trace?.('[门禁探针] 收到判定请求：' + ctx.criteria)
      ${body}
    },
    afterSubagentEnd: async () => {
      // 稍微慢一点：父会话必须在此期间"开始等待"，才能确定性地验到
      // "复核旁注并进唤醒内容"这条路（否则子任务可能在父会话开始等之前就全跑完了，
      // suspendForSubagents 会走"就地采集子会话报告"的捷径，看不到旁注）
      await new Promise((resolve) => setTimeout(resolve, 40))
      return { appendParentNote: '复核：本次产出已看过一遍' }
    },
  },
}
`,
    'utf-8'
  )
  await defaultExtensionLoader.autoLoadExtensions(workspace)
}

describe('门禁：拦下的委派不留下悬挂的父会话', () => {
  test('判定不通过时抛出，且不创建子会话、不进运行集合', async () => {
    await writeGatePlugin('deny')
    const store = new AgentStore(workspace)
    const parent = store.newThread(workspace)
    store.selectThread(parent.id)

    // 给该 profile 配上验收标准（内置 profile 由管理器提供，这里直接改内存里的副本）
    const manager = await import('./manager')
    const original = await manager.defaultSubagentManager.getById('researcher', workspace)
    expect(original).toBeDefined()
    const patched = { ...original!, gate: { criteria: '任务描述里必须写明验收标准' } }
    const spy = {
      getById: async () => patched,
    }
    const managerModule = manager.defaultSubagentManager as unknown as {
      getById: typeof spy.getById
    }
    const restore = managerModule.getById
    managerModule.getById = spy.getById

    try {
      const before = store.threads.length
      await expect(
        store.startSubagentThread({
          parentThreadId: parent.id,
          subagentId: 'researcher',
          task: '随便看看',
          onStepUpdate: () => {},
        })
      ).rejects.toThrow(/未通过启动门禁/)

      // 关键：没有留下子会话，也没有把这个 id 留在运行集合里
      expect(store.threads.length).toBe(before)
      expect(store.isThreadRunning(parent.id)).toBe(false)

      // 而且父会话"等子智能体"时不会干等到超时——没有可等的对象
      const outcome = await store.suspendForSubagents(parent, { timeoutMs: 300 })
      expect(outcome.timedOut).toBe(false)
      expect(outcome.wakes).toEqual([])
    } finally {
      managerModule.getById = restore
      store.deleteThread(parent.id)
    }
  })

  test('判定方只报 allowed 不报依据 → 未配 failOpen 时仍放行（门禁未生效）', async () => {
    await writeGatePlugin('deny-raw')
    const store = new AgentStore(workspace)
    const parent = store.newThread(workspace)
    store.selectThread(parent.id)

    const manager = await import('./manager')
    const original = await manager.defaultSubagentManager.getById('researcher', workspace)
    const managerModule = manager.defaultSubagentManager as unknown as {
      getById: typeof manager.defaultSubagentManager.getById
    }
    const restore = managerModule.getById
    managerModule.getById = async () => ({ ...original!, gate: { criteria: '标准' } })

    try {
      // 放行 → 离线兜底路径会立刻跑完，于是我们能拿到一次真实唤醒
      const { thread } = await store.startSubagentThread({
        parentThreadId: parent.id,
        subagentId: 'researcher',
        task: '随便看看',
        onStepUpdate: () => {},
      })
      const outcome = await store.suspendForSubagents(parent, { timeoutMs: 5000 })
      expect(outcome.wakes.length).toBeGreaterThan(0)
      // afterSubagentEnd 的旁注并进了唤醒内容里（模型看得到，不只是日志）
      expect(outcome.wakes[0]!.summary).toContain('复核')
      store.deleteThread(parent.id)
      store.deleteThread(thread.id)
    } finally {
      managerModule.getById = restore
    }
  })
})

describe('门禁：恢复执行同样要过（不再只在启动时判）', () => {
  test('resume 被拦下时抛出，且会话原样不动', async () => {
    // 第一次放行，先把会话正常建出来并跑完
    await writeGatePlugin('allow')
    const store = new AgentStore(workspace)
    const parent = store.newThread(workspace)
    store.selectThread(parent.id)

    const manager = await import('./manager')
    const original = await manager.defaultSubagentManager.getById('researcher', workspace)
    const managerModule = manager.defaultSubagentManager as unknown as {
      getById: typeof manager.defaultSubagentManager.getById
    }
    const restore = managerModule.getById
    managerModule.getById = async () => ({ ...original!, gate: { criteria: '任务描述里必须写明验收标准' } })

    const globals = globalThis as Record<string, unknown>
    globals.__gate_inputs = []

    try {
      const { thread, resultPromise } = await store.startSubagentThread({
        parentThreadId: parent.id,
        subagentId: 'researcher',
        task: '把仓库扫一遍',
        onStepUpdate: () => {},
      })
      await resultPromise
      expect(store.isThreadRunning(thread.id)).toBe(false)
      expect(thread.messages.length).toBeGreaterThan(0)

      // 换成拒绝判定，再尝试恢复（重载必须真的重读文件，见 loader 的 importPluginModule）
      globals.__gate_module_evals = 0
      await writeGatePlugin('deny')
      expect(globals.__gate_module_evals).toBe(1)

      const messagesBefore = thread.messages.length
      const itemsBefore = thread.items.length
      const threadsBefore = store.threads.length

      let error: unknown
      try {
        await store.resumeSubagentThread({
          subagentThreadId: thread.id,
          instruction: '接着上次的继续',
        })
      } catch (caught) {
        error = caught
      }

      expect(String(error)).toMatch(/未通过启动门禁/)

      // 关键：拦下发生在写任何东西之前——没有多出"恢复指示"消息、没进运行集合、没多出会话
      expect(thread.messages.length).toBe(messagesBefore)
      expect(thread.items.length).toBe(itemsBefore)
      expect(store.threads.length).toBe(threadsBefore)
      expect(store.isThreadRunning(thread.id)).toBe(false)

      // 判定输入是「原始任务 + 本次恢复指示」：首轮已把 task 消耗掉，
      // 只给"接着上次的继续"判定方无从判断。断言的是**插件真正收到的输入**（副作用），
      // 而不是"钩子被调用过"——门禁链路上并没有 `ctx.trace`。
      const inputs = globals.__gate_inputs as string[]
      expect(inputs.length).toBeGreaterThanOrEqual(2)
      const resumeInput = inputs[inputs.length - 1]!
      expect(resumeInput).toContain('把仓库扫一遍')
      expect(resumeInput).toContain('接着上次的继续')

      store.deleteThread(parent.id)
      store.deleteThread(thread.id)
    } finally {
      managerModule.getById = restore
      delete globals.__gate_inputs
      delete globals.__gate_module_evals
    }
  })
})

describe('afterSubagentEnd：抛错不阻断唤醒', () => {
  test('钩子抛错时父会话照样被唤醒', async () => {
    // 只提供一个会抛错的 afterSubagentEnd
    await writeFile(
      join(workspace, '.ada', 'extensions', 'boom.ts'),
      `export default {
  name: '会炸的复核钩子',
  tools: [],
  hooks: {
    afterSubagentEnd: async () => {
      throw new Error('复核钩子炸了')
    },
  },
}
`,
      'utf-8'
    )
    await defaultExtensionLoader.autoLoadExtensions(workspace)

    const store = new AgentStore(workspace)
    const parent = store.newThread(workspace)
    store.selectThread(parent.id)

    const { thread } = await store.startSubagentThread({
      parentThreadId: parent.id,
      subagentId: 'researcher',
      task: '随便看看',
      onStepUpdate: () => {},
    })

    const outcome = await store.suspendForSubagents(parent, { timeoutMs: 5000 })
    expect(outcome.timedOut).toBe(false)
    expect(outcome.wakes.length).toBeGreaterThan(0)
    expect(outcome.wakes[0]!.status).toBe('done')
    // 唤醒链路上的异常被记下来，而不是把父会话卡死
    expect(store.log.some((entry) => entry.text.includes('afterSubagentEnd 抛错'))).toBe(true)

    store.deleteThread(parent.id)
    store.deleteThread(thread.id)
  })
})
