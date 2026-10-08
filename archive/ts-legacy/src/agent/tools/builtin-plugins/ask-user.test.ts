/**
 * `ask_user` 问答通道的行为验收。
 *
 * 重点在会让人挂死或误解的边角——这些是这个机制唯一真正难的地方：
 * 用户中止时等待要收尾（否则整轮永久挂起）、子智能体必须被拒绝（并发派发时
 * 用户不知道在回答谁）、模型给的畸形选项不能进到界面里。
 *
 * 与审批的 `approvals` 是两套等待，这里刻意也验证它们互不干扰。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { cleanupTempDir } from '../../../../scripts/test-preload'
import type { Thread } from '../../types'
import { store as singleton, type AgentStore } from '../../store'
import { createAskUserTool } from './ask-user'
import { defaultToolRegistry } from '../registry'

let workspace = ''
let home = ''
let oldHome: string | undefined

beforeEach(async () => {
  workspace = await mkdtemp(join(tmpdir(), 'ada-ask-ws-'))
  home = await mkdtemp(join(tmpdir(), 'ada-ask-home-'))
  oldHome = process.env.A_DA_HOME
  process.env.A_DA_HOME = home
})

afterEach(async () => {
  if (oldHome === undefined) delete process.env.A_DA_HOME
  else process.env.A_DA_HOME = oldHome
  await cleanupTempDir(workspace)
  await cleanupTempDir(home)
})

/** 摆一个"主智能体正在跑、卡片已建"的现场：`ask_user` 依赖卡片拿到会话。 */
function setupRunningCall(store: AgentStore): { thread: Thread; callId: string } {
  const thread = store.newThread(workspace)
  store.selectThread(thread.id)
  internals(store).runningThreadIds.add(thread.id)
  const callId = `call_${Math.random().toString(36).slice(2)}`
  // 卡片由 gate() 在真实链路里创建；这里直接摆一张，等价于"工具正在执行"
  const card = {
    kind: 'tool' as const,
    id: `item_${callId}`,
    at: Date.now(),
    callId,
    name: 'ask_user',
    args: {},
    rawArgs: '{}',
    status: 'running' as const,
    threadId: thread.id,
  }
  thread.items.push(card)
  internals(store).cards.set(callId, card)
  return { thread, callId }
}

/** 收尾：单例是共享的，用完不删会让后面的用例看到别人的会话与卡片。 */
function cleanup(store: AgentStore, thread?: Thread): void {
  if (thread) {
    internals(store).cards.forEach((_card, key) => {
      if (key.startsWith('call_')) internals(store).cards.delete(key)
    })
    store.deleteThread(thread.id)
  }
}

/** 测试要直接摆布内部状态（卡片表、运行集合）。 */
function internals(store: AgentStore): {
  runningThreadIds: Set<string>
  cards: Map<string, { id: string; details?: Record<string, unknown> }>
} {
  return store as unknown as {
    runningThreadIds: Set<string>
    cards: Map<string, { id: string; details?: Record<string, unknown> }>
  }
}

/** 等提问注册好（它发生在工具 execute 内部）。 */
async function waitForPending(store: AgentStore, callId: string, timeoutMs = 1000): Promise<void> {
  const started = Date.now()
  while (Date.now() - started < timeoutMs) {
    if (store.isAwaitingAnswer(callId)) return
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
  throw new Error('提问一直没有注册：requestUserAnswer 可能根本没走到等待')
}

describe('ask_user 工具：契约与正常路径', () => {
  test('工具名、只读登记与参数结构', () => {
    const tool = createAskUserTool()
    expect(tool.name).toBe('ask_user')
    const props = tool.parameters.properties as Record<string, unknown>
    expect(props.question).toBeDefined()
    expect(props.choices).toBeDefined()
    // 只读：plan 阶段正是最需要澄清的时候
    expect(defaultToolRegistry.isWriteTool('ask_user')).toBe(false)
    // 描述里要说清"不要拿它问空泛的确认"，这是防滥用的引导
    expect(tool.description).toContain('不要用它问')
  })

  test('缺 question 时明确报错，不发起提问', async () => {
    const store = singleton
    const tool = createAskUserTool()
    const result = await tool.execute('c1', { question: '   ' })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('question')
    expect(store.isAwaitingAnswer('c1')).toBe(false)
  })

  test('选项畸形时如实报错，而不是渲染出点不动的按钮', async () => {
    const store = singleton
    setupRunningCall(store)
    const tool = createAskUserTool()
    const result = await tool.execute('c1', { question: '选哪个？', choices: [{ foo: 'bar' }] })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('label')
  })

  test('用户点选项：答案回给模型，且选项内容一并描述', async () => {
    const store = singleton
    const { callId } = setupRunningCall(store)
    const tool = createAskUserTool()
    const pending = tool.execute(callId, {
      question: '重构方式选哪种？',
      choices: [
        { id: 'a', label: '先补测试再重构' },
        { id: 'b', label: '一次性重构' },
      ],
    })

    await waitForPending(store, callId)
    store.answerQuestion(callId, { choice: 'a' })

    const result = await pending
    expect(result.ok).toBe(true)
    expect(result.output).toContain('先补测试再重构')
    expect(result.details?.question.status).toBe('answered')
    expect(result.details?.question.answer?.choice).toBe('a')
  })

  test('自由作答：没有选项时强制允许输入，答案原样回给模型', async () => {
    const store = singleton
    const { callId } = setupRunningCall(store)
    const tool = createAskUserTool()
    const pending = tool.execute(callId, { question: '要改哪个模块？' })

    await waitForPending(store, callId)
    store.answerQuestion(callId, { text: '只改 src/auth' })

    const result = await pending
    expect(result.ok).toBe(true)
    expect(result.output).toContain('只改 src/auth')
  })

  test('选项带 id 时按 id 回传，不依赖模型给的写法', async () => {
    const store = singleton
    const { callId } = setupRunningCall(store)
    const tool = createAskUserTool()
    const pending = tool.execute(callId, {
      question: '选哪个？',
      choices: [{ id: 'opt-x', label: '方案 X' }],
    })

    await waitForPending(store, callId)
    store.answerQuestion(callId, { choice: 'opt-x' })

    expect((await pending).details?.question.answer?.choice).toBe('opt-x')
  })
})

describe('ask_user 工具：中止与拒绝', () => {
  test('会话中止时如实说"未回答"，且不把它当有效输入（ok=false）', async () => {
    const store = singleton
    const { callId } = setupRunningCall(store)
    const controller = new AbortController()
    const tool = createAskUserTool()
    const pending = tool.execute(callId, { question: '要继续吗？' }, controller.signal)

    await waitForPending(store, callId)
    controller.abort()

    const result = await pending
    // 关键：ok=false，否则模型会把"没回答"当成"用户答了"
    expect(result.ok).toBe(false)
    expect(result.output).toContain('中止')
    expect(result.details?.question.status).toBe('aborted')
    // 等待必须被摘掉，否则整轮永久挂起
    expect(store.isAwaitingAnswer(callId)).toBe(false)
  })

  test('已经是 aborted 的 signal 也能干净收尾', async () => {
    const store = singleton
    const { callId } = setupRunningCall(store)
    const controller = new AbortController()
    controller.abort()

    const tool = createAskUserTool()
    const result = await tool.execute(callId, { question: '问？' }, controller.signal)
    expect(result.ok).toBe(false)
    expect(store.isAwaitingAnswer(callId)).toBe(false)
  })

  test('子智能体提问被如实拒绝，并指向 notify_parent', async () => {
    const store = singleton
    const parent = store.newThread(workspace)
    store.selectThread(parent.id)
    const child: Thread = {
      id: `subagent_q_${Math.random().toString(36).slice(2)}`,
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
    const callId = 'call_child'
    const card = {
      kind: 'tool' as const,
      id: 'item_child',
      at: Date.now(),
      callId,
      name: 'ask_user',
      args: {},
      rawArgs: '{}',
      status: 'running' as const,
      threadId: child.id,
    }
    child.items.push(card)
    internals(store).cards.set(callId, card)

    const tool = createAskUserTool()
    const result = await tool.execute(callId, { question: '该用哪个接口？' })

    expect(result.ok).toBe(false)
    expect(result.output).toContain('子智能体')
    expect(result.output).toContain('notify_parent')
  })
})

describe('ask_user：待答提问的查询（浮动面板的数据来源）', () => {
  test('提问挂起时出现在列表里，作答后立即消失', async () => {
    const store = singleton
    const { callId } = setupRunningCall(store)
    const tool = createAskUserTool()
    const pending = tool.execute(callId, { question: '先做哪个？', choices: [{ id: 'a', label: 'A' }] })

    await waitForPending(store, callId)
    const during = store.pendingAnswerQuestions
    expect(during.map((entry) => entry.callId)).toContain(callId)
    expect(during.find((entry) => entry.callId === callId)?.question.question).toBe('先做哪个？')

    store.answerQuestion(callId, { choice: 'a' })
    await pending
    expect(store.pendingAnswerQuestions.map((entry) => entry.callId)).not.toContain(callId)
  })

  test('并发会话各自的提问互不串台（只返回当前会话的）', async () => {
    const store = singleton
    const first = setupRunningCall(store)
    const second = setupRunningCall(store)
    const tool = createAskUserTool()
    const pendingFirst = tool.execute(first.callId, { question: '第一问？' })
    const pendingSecond = tool.execute(second.callId, { question: '第二问？' })

    await waitForPending(store, first.callId)
    await waitForPending(store, second.callId)
    // setupRunningCall 会把新会话选中，所以当前会话是第二个
    const callIds = store.pendingAnswerQuestions.map((entry) => entry.callId)
    expect(callIds).toContain(second.callId)
    expect(callIds).not.toContain(first.callId)

    // 切回第一个会话，它自己的提问就在列表里
    store.selectThread(first.thread.id)
    expect(store.pendingAnswerQuestions.map((entry) => entry.callId)).toEqual([first.callId])

    store.answerQuestion(first.callId, { text: '答' })
    store.answerQuestion(second.callId, { text: '答' })
    await Promise.all([pendingFirst, pendingSecond])
  })

  test('卡片被收尾（工具结束）后列表里不会残留点不动的问答卡', async () => {
    const store = singleton
    const { callId } = setupRunningCall(store)
    const tool = createAskUserTool()
    const pending = tool.execute(callId, { question: '还算数吗？' })
    await waitForPending(store, callId)
    expect(store.pendingAnswerQuestions).toHaveLength(1)

    // 模拟工具收尾：卡片被摘掉，但等待句柄还在（真实链路里 finishToolCall 之后
    // 就是这一刻）。此时列表必须为空——否则界面上会留下一个点不动的问答卡。
    internals(store).cards.delete(callId)
    expect(store.pendingAnswerQuestions).toHaveLength(0)

    store.answerQuestion(callId, { text: '收尾' })
    await pending
  })
})

describe('ask_user：与审批等待互不干扰', () => {
  test('作答不会误触发审批，且两套等待各自独立收尾', async () => {
    const store = singleton
    const { callId } = setupRunningCall(store)
    const tool = createAskUserTool()
    const pending = tool.execute(callId, { question: '选哪个？', choices: [{ id: 'a', label: 'A' }] })
    await waitForPending(store, callId)

    // 用一个不存在的调用 id 作答：不该影响任何东西
    store.answerQuestion('不存在的调用', { choice: 'a' })
    expect(store.isAwaitingAnswer(callId)).toBe(true)

    // 也不该被审批的 decide 解开（两套 maps 是分开的）
    store.decide(callId, true)
    expect(store.isAwaitingAnswer(callId)).toBe(true)

    store.answerQuestion(callId, { choice: 'a' })
    expect((await pending).ok).toBe(true)
  })
})

describe('ask_user：选项规整', () => {
  test('超过上限的选项被截断（选项一多就变成菜单）', async () => {
    const store = singleton
    const { callId } = setupRunningCall(store)
    const tool = createAskUserTool()
    const many = Array.from({ length: 10 }, (_, index) => ({ label: `选项 ${index + 1}` }))
    const pending = tool.execute(callId, { question: '选一个？', choices: many })

    await waitForPending(store, callId)
    store.answerQuestion(callId, { choice: 'c1' })

    const result = await pending
    expect(result.details?.question.choices).toHaveLength(6)
  })

  test('没给 id 的选项自动编号，且重复 id 被丢弃', async () => {
    const store = singleton
    const { callId } = setupRunningCall(store)
    const tool = createAskUserTool()
    const pending = tool.execute(callId, {
      question: '选一个？',
      choices: [{ label: 'A' }, { id: 'dup', label: 'B' }, { id: 'dup', label: 'C' }],
    })

    await waitForPending(store, callId)
    store.answerQuestion(callId, { choice: 'c1' })

    const result = await pending
    expect(result.details?.question.choices?.map((choice) => choice.id)).toEqual(['c1', 'dup'])
  })
})
