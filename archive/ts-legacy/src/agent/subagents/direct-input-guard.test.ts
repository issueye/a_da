/**
 * 子智能体会话的**直接输入防线**（拍板结论：子智能体标签页不接受直接输入）。
 *
 * 背景（`docs/unfinished-features.md` §一.9）：原先有两条路能"以主会话身份"跑子智能体会话——
 * 用户在子智能体标签页里直接打字（`send → drain → turn`），以及 `steerSubagentThread` 对已停止
 * 子智能体的"重新排队 + drain"。两条路都不过门禁、也不应用 profile 白名单，于是只读子智能体
 * 能拿到写工具，且没有任何提示（属于"看起来装上了、其实没生效"）。
 *
 * 这个文件钉住三件事：
 * 1. `send` 挡掉并**指路**（不是静默丢弃）；
 * 2. `turn()` 兜底拒绝——即使有人绕过 `send` 塞进队列，也不会以主会话身份执行；
 * 3. `steerSubagentThread` 的续跑分支**改为委派**给 `resumeSubagentThread`（自带门禁 + profile）。
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { store } from '../store'
import type { Item, Thread } from '../types'

/** 起一个父会话 + 一个已停止的子智能体会话；返回两者。 */
async function createStoppedSubagent(): Promise<{ parent: Thread; subagent: Thread }> {
  const parent = store.newThread(process.cwd())
  store.selectThread(parent.id)
  const { thread, resultPromise } = await store.startSubagentThread({
    subagentId: 'code_reviewer',
    task: '审查测试代码',
    onStepUpdate: () => {},
  })
  // 离线环境下这一轮会失败收尾，但一定会停下来——门禁/续跑都要求"不在运行中"
  await resultPromise.catch(() => {})
  return { parent, subagent: thread }
}

const created: string[] = []

afterEach(() => {
  for (const id of created.splice(0)) store.deleteThread(id)
})

describe('子智能体会话不接受直接输入', () => {
  test('send 挡掉：不进会话流、不排队，并给出一条指路提示', async () => {
    const { parent, subagent } = await createStoppedSubagent()
    created.push(parent.id)

    store.selectThread(subagent.id)
    const itemsBefore = subagent.items.length
    const queueBefore = store.queue.length

    store.send('我直接跟子智能体说话')

    expect(subagent.items.length).toBe(itemsBefore)
    expect(store.queue.length).toBe(queueBefore)
    // 提示与 trace 是两条（push 的是给人看的，trace 的是给排查用的），这里找前者
    const notice = store.log.filter((entry) => entry.text.includes('不接受直接输入')).at(-1)
    expect(notice?.text).toContain('子智能体专属执行会话')
    // 提示里要**指出该走哪条路**，否则用户只知道自己被拒了
    expect(notice?.text).toContain('send_subagent_message')
    expect(store.isThreadRunning(subagent.id)).toBe(false)
  })

  test('兜底：绕过 send 塞进队列，也不会以主会话身份执行子智能体会话', async () => {
    const { parent, subagent } = await createStoppedSubagent()
    created.push(parent.id)

    // 白盒：直接往队列里塞一条（模拟任何绕过 send 的路径：命令通道、将来的新入口…）
    const privateStore = store as unknown as {
      queues: Map<string, Array<{ thread: Thread; text: string; item?: Item }>>
      drain: (thread: Thread) => Promise<void>
    }
    // 子智能体自己首轮已经留下了条目，所以比的是**增量**
    const executionsBefore = subagent.items.filter(
      (item) => item.kind === 'assistant' || item.kind === 'tool'
    ).length
    privateStore.queues.set(subagent.id, [{ thread: subagent, text: '绕过 send 的输入' }])

    await privateStore.drain(subagent)

    // 关键：没有产生任何**新的**助手/工具条目——也就是没有真的跑一轮
    const executionsAfter = subagent.items.filter(
      (item) => item.kind === 'assistant' || item.kind === 'tool'
    ).length
    expect(executionsAfter).toBe(executionsBefore)
    expect(store.log.some((entry) => entry.text.includes('已阻止在子智能体会话'))).toBe(true)
    expect(store.isThreadRunning(subagent.id)).toBe(false)
  })

  test('steer 对已停止的子智能体走正式恢复路径（委派，不再自己排队）', async () => {
    const { parent, subagent } = await createStoppedSubagent()
    created.push(parent.id)

    // 断言"委派"这个**结构事实**：把 resumeSubagentThread 换成一个记录调用的桩。
    // 这样即使不搭门禁插件的脚手架，也能证明这条路不再自己 queue + drain
    // （门禁与 profile 白名单的覆盖由 `subagents/gate-delegation.test.ts` 提供，
    //  它测的就是 resumeSubagentThread）。
    const mutable = store as unknown as {
      resumeSubagentThread: (options: unknown) => Promise<{ thread: Thread; resultPromise: Promise<unknown> }>
    }
    const original = mutable.resumeSubagentThread
    let delegated: Record<string, unknown> | null = null
    mutable.resumeSubagentThread = async (options: unknown) => {
      delegated = options as Record<string, unknown>
      return { thread: subagent, resultPromise: Promise.resolve({}) }
    }

    try {
      const result = await store.steerSubagentThread({
        subagentThreadId: subagent.id,
        message: '接着上次的继续',
      })
      expect(result.status).toBe('resumed')
      expect(delegated).toMatchObject({
        subagentThreadId: subagent.id,
        instruction: '接着上次的继续',
      })
      // 没有被塞进队列（旧实现会在这里 push 一条）
      expect(store.queue.length).toBe(0)
    } finally {
      mutable.resumeSubagentThread = original
    }
  })
})
