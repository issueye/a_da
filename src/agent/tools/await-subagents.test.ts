/**
 * 子智能体等待 / 唤醒机制：主智能体挂起等待，由子智能体把结论送回来。
 *
 * 这套机制替代的是「主智能体反复 check_subagent 轮询」——每一圈轮询都是一整轮模型请求，
 * 又慢又贵。它真正的难点不在正常路径，而在那些会让人挂死或丢结论的边角：子智能体比
 * 主智能体先结束、唤醒打在没有等待的会话上、超时、中止、看护对象已全部结束。测试主要
 * 盯这些。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { cleanupTempDir } from '../../../scripts/test-preload'
import type { Item, Thread } from '../types'
import { AgentStore, store as singleton, type SubagentWake } from '../store'
import {
  createAwaitSubagentsTool,
  createNotifyParentTool,
} from './builtins/subagent'
import { defaultToolRegistry } from './registry'
import { defaultCheckpointManager } from '../checkpoint'

let workspace = ''
let home = ''
let oldHome: string | undefined

/** 测试要直接摆布「哪些会话在运行」，而 runningThreadIds 是私有的。 */
function internals(store: AgentStore): {
  runningThreadIds: Set<string>
  waitingThreadIds: Set<string>
} {
  return store as unknown as {
    runningThreadIds: Set<string>
    waitingThreadIds: Set<string>
  }
}

beforeEach(async () => {
  workspace = await mkdtemp(join(tmpdir(), 'ada-await-ws-'))
  home = await mkdtemp(join(tmpdir(), 'ada-await-home-'))
  oldHome = process.env.A_DA_HOME
  process.env.A_DA_HOME = home
})

afterEach(async () => {
  if (oldHome === undefined) delete process.env.A_DA_HOME
  else process.env.A_DA_HOME = oldHome
  await cleanupTempDir(workspace)
  await cleanupTempDir(home)
})

/** 造一个父会话 + 一个「正在运行」的子会话，返回两者。 */
function setupParentAndChild(store: AgentStore): { parent: Thread; child: Thread } {
  const parent = store.newThread(workspace)
  const child: Thread = {
    id: `subagent_test_${Math.random().toString(36).slice(2)}`,
    title: '调研任务',
    createdAt: Date.now(),
    workspace,
    items: [],
    messages: [],
    parentId: parent.id,
    subagentId: 'researcher',
    isSubagent: true,
  }
  store.threads.push(child)
  internals(store).runningThreadIds.add(child.id)
  return { parent, child }
}

function wakeOf(child: Thread, summary: string, status: SubagentWake['status'] = 'report'): SubagentWake {
  return {
    threadId: child.id,
    subagentId: child.subagentId,
    name: child.title,
    summary,
    status,
    at: Date.now(),
  }
}

function userItems(thread: Thread): number {
  return thread.items.filter((it: Item) => it.kind === 'user').length
}

describe('suspendForSubagents：挂起与唤醒', () => {
  test('子智能体显式唤醒后，挂起立即返回并带回内容', async () => {
    const store = new AgentStore(workspace)
    const { parent, child } = setupParentAndChild(store)

    const pending = store.suspendForSubagents(parent, { timeoutMs: 5000 })
    // 挂起期间界面必须能看出它在等，而不是在思考
    expect(store.isThreadWaiting(parent.id)).toBe(true)

    store.wakeParent(wakeOf(child, '已定位到关键实现，建议下一步改造 store.ts'))
    const outcome = await pending

    expect(outcome.aborted).toBe(false)
    expect(outcome.timedOut).toBe(false)
    expect(outcome.wakes).toHaveLength(1)
    expect(outcome.wakes[0]!.summary).toContain('已定位到关键实现')
    expect(outcome.wakes[0]!.status).toBe('report')
    // 醒来之后等待标识要撤掉
    expect(store.isThreadWaiting(parent.id)).toBe(false)

    store.deleteThread(parent.id)
  })

  test('report 类唤醒不必等其余子任务：立即结束等待', async () => {
    const store = new AgentStore(workspace)
    const parent = store.newThread(workspace)
    const first: Thread = {
      id: 'sub_first', title: 'A', createdAt: Date.now(), workspace, items: [], messages: [],
      parentId: parent.id, subagentId: 'researcher', isSubagent: true,
    }
    const second: Thread = {
      id: 'sub_second', title: 'B', createdAt: Date.now(), workspace, items: [], messages: [],
      parentId: parent.id, subagentId: 'tester', isSubagent: true,
    }
    store.threads.push(first, second)
    internals(store).runningThreadIds.add(first.id)
    internals(store).runningThreadIds.add(second.id)

    const pending = store.suspendForSubagents(parent, { timeoutMs: 5000 })
    // 只有 A 回报，B 还在跑；report 语义是「主智能体该做下一步了」，所以不等 B
    store.wakeParent(wakeOf(first, 'A 的阶段性结论'))

    const outcome = await pending
    expect(outcome.wakes).toHaveLength(1)
    expect(outcome.wakes[0]!.threadId).toBe('sub_first')
    expect(store.isThreadWaiting(parent.id)).toBe(false)

    store.deleteThread(parent.id)
  })

  test('done 类唤醒要等看护对象全部结束，结论一并交付', async () => {
    const store = new AgentStore(workspace)
    const parent = store.newThread(workspace)
    const first: Thread = {
      id: 'sub_a', title: 'A', createdAt: Date.now(), workspace, items: [], messages: [],
      parentId: parent.id, subagentId: 'researcher', isSubagent: true,
    }
    const second: Thread = {
      id: 'sub_b', title: 'B', createdAt: Date.now(), workspace, items: [], messages: [],
      parentId: parent.id, subagentId: 'tester', isSubagent: true,
    }
    store.threads.push(first, second)
    internals(store).runningThreadIds.add(first.id)
    internals(store).runningThreadIds.add(second.id)

    let settled = false
    const pending = store.suspendForSubagents(parent, { timeoutMs: 5000 }).then((outcome) => {
      settled = true
      return outcome
    })

    // A 跑完（完成类唤醒）：B 还在跑，所以等待不该结束
    internals(store).runningThreadIds.delete(first.id)
    store.wakeParent(wakeOf(first, 'A 已完成', 'done'))
    await new Promise((resolve) => setTimeout(resolve, 30))
    expect(settled).toBe(false)
    expect(store.isThreadWaiting(parent.id)).toBe(true)

    // B 也跑完：这时全部结束，一并交付
    internals(store).runningThreadIds.delete(second.id)
    store.wakeParent(wakeOf(second, 'B 已完成', 'done'))

    const outcome = await pending
    expect(outcome.wakes).toHaveLength(2)
    expect(outcome.wakes.map((w) => w.threadId).sort()).toEqual(['sub_a', 'sub_b'])

    store.deleteThread(parent.id)
  })
})

describe('suspendForSubagents：不该白等或挂死', () => {
  test('没有正在运行的子智能体时立即返回，不干等', async () => {
    const store = new AgentStore(workspace)
    const parent = store.newThread(workspace)

    const startedAt = Date.now()
    const outcome = await store.suspendForSubagents(parent, { timeoutMs: 60_000 })
    const elapsed = Date.now() - startedAt

    expect(outcome.wakes).toHaveLength(0)
    expect(outcome.timedOut).toBe(false)
    // 关键：不是等满 60 秒才回来
    expect(elapsed).toBeLessThan(1000)
    expect(store.isThreadWaiting(parent.id)).toBe(false)

    store.deleteThread(parent.id)
  })

  test('看护对象在挂起前就已全部结束 → 立即返回', async () => {
    const store = new AgentStore(workspace)
    const { parent, child } = setupParentAndChild(store)
    // 指定的目标已经不在运行了
    internals(store).runningThreadIds.delete(child.id)

    const startedAt = Date.now()
    const outcome = await store.suspendForSubagents(parent, {
      threadIds: [child.id],
      timeoutMs: 60_000,
    })
    const elapsed = Date.now() - startedAt

    expect(outcome.timedOut).toBe(false)
    expect(elapsed).toBeLessThan(1000)

    store.deleteThread(parent.id)
  })

  test('超时返回已收集到的结论，并标明超时', async () => {
    const store = new AgentStore(workspace)
    const { parent, child } = setupParentAndChild(store)

    const pending = store.suspendForSubagents(parent, { timeoutMs: 40 })
    // 先送一份 done 结论（不会立即结束等待，因为子任务仍在运行），随后超时
    store.wakeParent(wakeOf(child, '超时前的部分结论', 'done'))

    const outcome = await pending
    expect(outcome.timedOut).toBe(true)
    expect(outcome.wakes).toHaveLength(1)
    expect(outcome.wakes[0]!.summary).toContain('超时前的部分结论')
    expect(store.isThreadWaiting(parent.id)).toBe(false)

    store.deleteThread(parent.id)
  })

  test('中止时干净退出，不留等待状态', async () => {
    const store = new AgentStore(workspace)
    const { parent } = setupParentAndChild(store)

    const controller = new AbortController()
    const pending = store.suspendForSubagents(parent, { timeoutMs: 60_000, signal: controller.signal })
    expect(store.isThreadWaiting(parent.id)).toBe(true)

    controller.abort()
    const outcome = await pending

    expect(outcome.aborted).toBe(true)
    expect(store.isThreadWaiting(parent.id)).toBe(false)
    // 中止后没有残留的等待条目：再挂起一次应该能正常注册
    expect(internals(store).waitingThreadIds.has(parent.id)).toBe(false)

    store.deleteThread(parent.id)
  })

  test('已经是 aborted 的 signal 也能干净退出', async () => {
    const store = new AgentStore(workspace)
    const { parent } = setupParentAndChild(store)

    const controller = new AbortController()
    controller.abort()
    const outcome = await store.suspendForSubagents(parent, { signal: controller.signal })

    expect(outcome.aborted).toBe(true)
    expect(store.isThreadWaiting(parent.id)).toBe(false)

    store.deleteThread(parent.id)
  })
})

describe('wakeParent：父智能体不在等待时', () => {
  test('只回报、不启动新轮次', async () => {
    const store = new AgentStore(workspace)
    const { parent, child } = setupParentAndChild(store)
    const usersBefore = userItems(parent)

    const result = store.wakeParent(wakeOf(child, '结论：无需修改'))

    expect(result.delivered).toBe(false)
    expect(result.reason).toContain('未挂起等待')
    // 关键约束：不能凭空开一轮，也不该留下用户消息
    expect(store.isThreadRunning(parent.id)).toBe(false)
    expect(userItems(parent)).toBe(usersBefore)
    // 但内容要留下痕迹，用户/主智能体之后能看到
    expect(parent.items.some((it) => it.kind === 'notice' && it.text.includes('无需修改'))).toBe(true)

    store.deleteThread(parent.id)
  })

  test('缓冲下来的结论会在下一次挂起时立即交付', async () => {
    const store = new AgentStore(workspace)
    const { parent, child } = setupParentAndChild(store)

    // 先唤醒（父智能体没在等）→ 进缓冲
    const first = store.wakeParent(wakeOf(child, '先到的结论'))
    expect(first.delivered).toBe(false)

    // 之后才挂起：缓冲里那份应当立刻交付，而不是被丢掉
    const startedAt = Date.now()
    const outcome = await store.suspendForSubagents(parent, { timeoutMs: 60_000 })
    expect(Date.now() - startedAt).toBeLessThan(1000)
    expect(outcome.wakes).toHaveLength(1)
    expect(outcome.wakes[0]!.summary).toBe('先到的结论')

    store.deleteThread(parent.id)
  })

  test('子会话没有父会话时如实拒绝', () => {
    const store = new AgentStore(workspace)
    const orphan = store.newThread(workspace)
    const result = store.wakeParent({
      threadId: orphan.id,
      summary: '无处投递',
      status: 'report',
      at: Date.now(),
    })
    expect(result.delivered).toBe(false)
    expect(result.reason).toContain('没有父会话')
    store.deleteThread(orphan.id)
  })
})

describe('真实委派链路：后台子智能体跑完自动唤醒等待中的主智能体', () => {
  test('无 LLM 的兜底子任务完成时，唤醒等待中的父智能体', async () => {
    const store = new AgentStore(workspace)
    const parent = store.newThread(workspace)
    store.selectThread(parent.id)

    // 后台模式（提供 onStepUpdate 即视为后台），兜底路径会立刻产出结论
    await store.startSubagentThread({
      parentThreadId: parent.id,
      subagentId: 'researcher',
      task: '调研代码库结构',
      onStepUpdate: () => {},
    })

    // 子任务已结束 → 结论要么当场交付给等待者，要么进缓冲；
    // 这里父智能体随后才挂起，应该从缓冲/已完成里拿到它。
    const outcome = await store.suspendForSubagents(parent, { timeoutMs: 5000 })
    expect(outcome.timedOut).toBe(false)
    expect(outcome.wakes.length).toBeGreaterThan(0)
    expect(outcome.wakes[0]!.status).toBe('done')
    expect(outcome.wakes[0]!.summary.length).toBeGreaterThan(0)

    store.deleteThread(parent.id)
  })

  test('删除父会话会清掉等待，不留下悬挂的 promise', async () => {
    const store = new AgentStore(workspace)
    const { parent, child } = setupParentAndChild(store)

    const pending = store.suspendForSubagents(parent, { timeoutMs: 60_000 })
    expect(store.isThreadWaiting(parent.id)).toBe(true)

    store.deleteThread(parent.id)

    // 必须 resolve 掉（aborted），否则这个 promise 会永远挂着
    const outcome = await pending
    expect(outcome.aborted).toBe(true)
    expect(store.isThreadWaiting(parent.id)).toBe(false)
    expect(child.id).toBeDefined()
  })
})

describe('工具接线与可见性', () => {
  test('await_subagents 在主工具表里；notify_parent 不在（它只属于子智能体身份）', () => {
    const names = defaultToolRegistry.getToolsForWorkspace(workspace).map((t) => t.name)
    expect(names).toContain('await_subagents')
    expect(names).not.toContain('notify_parent')
  })

  test('两个工具都被认作只读，plan 模式与只读子智能体才用得上', () => {
    expect(defaultToolRegistry.isWriteTool('await_subagents')).toBe(false)
    expect(defaultToolRegistry.isWriteTool('notify_parent')).toBe(false)
  })

  test('await_subagents 的参数结构包含等待目标与超时', () => {
    const tool = createAwaitSubagentsTool(workspace)
    expect(tool.name).toBe('await_subagents')
    const props = tool.parameters.properties as Record<string, unknown>
    expect(props.subagent_thread_ids).toBeDefined()
    expect(props.timeout_ms).toBeDefined()
    // 描述里必须把「别去轮询」说清楚，这是这个工具存在的理由
    expect(tool.description).toContain('check_subagent')
  })

  test('notify_parent 绑定到具体子会话，避免并发时认错人', () => {
    const tool = createNotifyParentTool('subagent_thread_xyz')
    expect(tool.name).toBe('notify_parent')
    expect(tool.parameters.required).toContain('summary')
  })

  test('notify_parent 把内容送进等待中的父智能体', async () => {
    // 工具内部动态 import 的是模块级 store 单例（生产环境就是它），所以这里必须用同一个
    const parent = singleton.newThread(workspace)
    const child: Thread = {
      id: `subagent_notify_${Math.random().toString(36).slice(2)}`,
      title: '调研任务',
      createdAt: Date.now(),
      workspace,
      items: [],
      messages: [],
      parentId: parent.id,
      subagentId: 'researcher',
      isSubagent: true,
    }
    singleton.threads.push(child)
    internals(singleton).runningThreadIds.add(child.id)

    const pending = singleton.suspendForSubagents(parent, { timeoutMs: 5000 })

    const tool = createNotifyParentTool(child.id)
    const result = await tool.execute('call_notify_1', {
      summary: '需要上层决定：方案 A 还是 B',
      message: '细节补充：两者都可行，A 更快但更侵入',
    })

    expect(result.ok).toBe(true)
    expect((result.details as any).delivered).toBe(true)

    const outcome = await pending
    expect(outcome.wakes).toHaveLength(1)
    // summary 与 message 应合并送达
    expect(outcome.wakes[0]!.summary).toContain('方案 A 还是 B')
    expect(outcome.wakes[0]!.summary).toContain('细节补充')

    singleton.deleteThread(parent.id)
  })

  test('notify_parent 缺 summary 时明确报错', () => {
    const tool = createNotifyParentTool('whatever')
    return tool.execute('call_notify_2', { summary: '   ' }).then((result) => {
      expect(result.ok).toBe(false)
      expect(result.output).toContain('summary')
    })
  })

  test('await_subagents 工具能等到唤醒并回传结论', async () => {
    const parent = singleton.newThread(workspace)
    const child: Thread = {
      id: `subagent_await_${Math.random().toString(36).slice(2)}`,
      title: '调研任务',
      createdAt: Date.now(),
      workspace,
      items: [],
      messages: [],
      parentId: parent.id,
      subagentId: 'researcher',
      isSubagent: true,
    }
    singleton.threads.push(child)
    internals(singleton).runningThreadIds.add(child.id)

    const tool = createAwaitSubagentsTool(workspace, parent.id)
    const pending = tool.execute('call_await_1', { subagent_thread_ids: [child.id], timeout_ms: 5000 })

    // 等工具真正注册好等待，再唤醒
    for (let i = 0; i < 100 && !singleton.isThreadWaiting(parent.id); i++) {
      await new Promise((resolve) => setTimeout(resolve, 10))
    }
    expect(singleton.isThreadWaiting(parent.id)).toBe(true)

    singleton.wakeParent({
      threadId: child.id,
      subagentId: child.subagentId,
      name: child.title,
      summary: '调研结论：入口在 app.tsx',
      status: 'report',
      at: Date.now(),
    })
    const result = await pending

    expect(result.ok).toBe(true)
    expect(result.output).toContain('调研结论')
    expect((result.details as any).received).toBe(1)

    singleton.deleteThread(parent.id)
    await defaultCheckpointManager.discard(parent.id)
  })

  test('await_subagents 在没有可等待对象时如实说明', async () => {
    // 这个父会话没有任何子会话，工具应当立即返回说明而不是干等
    const parent = singleton.newThread(workspace)
    singleton.selectThread(parent.id)

    const tool = createAwaitSubagentsTool(workspace, parent.id)
    const startedAt = Date.now()
    const result = await tool.execute('call_await_2', { timeout_ms: 60_000 })

    expect(result.ok).toBe(true)
    expect(Date.now() - startedAt).toBeLessThan(2000)
    expect((result.details as any).received).toBe(0)

    singleton.deleteThread(parent.id)
  })
})
