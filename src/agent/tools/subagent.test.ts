import { describe, expect, test } from 'bun:test'
import { createSubagentTool, createCheckSubagentTool } from './builtins/subagent'
import { defaultSubagentManager } from '../subagents/manager'
import { store } from '../store'

describe('invoke_subagent tool', () => {
  const tool = createSubagentTool(process.cwd())

  test('参数结构与元数据校验（包含 async 参数）', () => {
    expect(tool.name).toBe('invoke_subagent')
    expect(tool.executionMode).toBe('parallel')
    expect(tool.parameters.type).toBe('object')
    const props = tool.parameters.properties as Record<string, unknown>
    expect(props.subagent_id).toBeDefined()
    expect(props.task).toBeDefined()
    expect(props.additional_context).toBeDefined()
    expect(props.async).toBeDefined()
    expect(tool.parameters.required).toContain('subagent_id')
    expect(tool.parameters.required).toContain('task')
  })

  test('不存在的子智能体返回友好错误与可用列表', async () => {
    const result = await tool.execute('call_test_1', {
      subagent_id: 'non_existent_agent',
      task: '测试未知智能体',
    })

    expect(result.ok).toBe(false)
    expect(result.output).toContain('未找到 ID 为 "non_existent_agent" 的子智能体')
    expect(result.output).toContain('researcher')
  })

  test('处于禁用状态的子智能体应被拒绝', async () => {
    // 临时禁用 researcher
    await defaultSubagentManager.toggleSubagent('researcher', false)

    try {
      const result = await tool.execute('call_test_2', {
        subagent_id: 'researcher',
        task: '调研代码库',
      })
      expect(result.ok).toBe(false)
    } finally {
      // 恢复启用
      await defaultSubagentManager.toggleSubagent('researcher', true)
    }
  })

  test('动态工具描述生成包含可用角色与委派准则', () => {
    expect(tool.description).toContain('## 可委派子智能体列表')
    expect(tool.description).toContain('general_purpose')
    expect(tool.description).toContain('全能执行专员')
    expect(tool.description).toContain('## 委派准则与最佳实践')
    expect(tool.description).toContain('自包含任务')
  })

  test('invoke_subagent 执行时不会发生 Cannot access thread before initialization 错误', async () => {
    const parent = store.newThread(process.cwd())
    store.selectThread(parent.id)

    const updates: any[] = []
    const result = await tool.execute(
      'call_test_no_tdz',
      {
        subagent_id: 'researcher',
        task: '测试无 TDZ 异常',
        async: true,
      },
      undefined,
      (update) => updates.push(update)
    )

    expect(result.ok).toBe(true)
    expect(result.output).not.toContain("Cannot access 'thread' before initialization")
    expect(result.details?.subagent_thread_id).toBeDefined()
    expect(updates.length).toBeGreaterThan(0)

    store.deleteThread(parent.id)
  })
})

describe('check_subagent tool', () => {
  const checkTool = createCheckSubagentTool()

  test('参数结构与只读特性校验', () => {
    expect(checkTool.name).toBe('check_subagent')
    expect(checkTool.label).toContain('查询')
    const props = checkTool.parameters.properties as Record<string, unknown>
    expect(props.subagent_thread_id).toBeDefined()
    expect(props.subagent_id).toBeDefined()
  })

  test('未找到的子智能体会话返回提示', async () => {
    const result = await checkTool.execute('check_test_1', {
      subagent_thread_id: 'non_existent_subagent_thread_id',
    })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('未找到匹配的子智能体会话')
  })
})

describe('send_subagent_message tool & steering', () => {
  test('send_subagent_message 工具参数与执行', async () => {
    const { createSendSubagentMessageTool } = await import('./builtins/subagent')
    const sendTool = createSendSubagentMessageTool()
    expect(sendTool.name).toBe('send_subagent_message')
    expect(sendTool.label).toContain('向子智能体发送消息')

    // 未知会话测试
    const failRes = await sendTool.execute('msg_call_1', {
      subagent_thread_id: 'unknown_subagent_thread',
      message: '请转向关注 memory 模块',
    })
    expect(failRes.ok).toBe(false)
    expect(failRes.output).toContain('未找到 ID 为')

    // 真实子会话测试
    const parent = store.newThread(process.cwd())
    store.selectThread(parent.id)
    const { thread } = await store.startSubagentThread({
      subagentId: 'researcher',
      task: '查找所有测试文件',
    })

    const successRes = await sendTool.execute('msg_call_2', {
      subagent_thread_id: thread.id,
      message: '聚焦到 src/agent 目录',
      summary: '缩小搜索范围',
    })

    expect(successRes.ok).toBe(true)
    expect(successRes.output).toContain(thread.title)

    store.deleteThread(parent.id)
  })
})

describe('store subagent execution & thread management', () => {
  test('startSubagentThread 创建带有 parentId、isSubagent 的独立页签与会话', async () => {
    const parent = store.newThread(process.cwd())
    store.selectThread(parent.id)

    const { thread } = await store.startSubagentThread({
      subagentId: 'code_reviewer',
      task: '审查测试代码',
    })

    expect(thread.isSubagent).toBe(true)
    expect(thread.parentId).toBe(parent.id)
    expect(thread.subagentId).toBe('code_reviewer')
    expect(store.openTabIds).toContain(thread.id)

    // 删除父会话时应级联清理子智能体会话
    store.deleteThread(parent.id)
    expect(store.threads.some((t) => t.id === thread.id)).toBe(false)
    expect(store.openTabIds).not.toContain(thread.id)
  })

  test('startSubagentThread 显式指定 parentThreadId 时不受当前 activeId 切换影响', async () => {
    const parentA = store.newThread(process.cwd())
    const parentB = store.newThread(process.cwd())
    store.selectThread(parentB.id)

    // 虽然当前 active 是 parentB，但显式为 parentA 派发子智能体
    const { thread } = await store.startSubagentThread({
      parentThreadId: parentA.id,
      subagentId: 'researcher',
      task: '为会话 A 调研代码',
    })

    expect(thread.parentId).toBe(parentA.id)
    expect(thread.parentId).not.toBe(parentB.id)

    store.deleteThread(parentA.id)
    store.deleteThread(parentB.id)
  })

  test('当 active 处于子智能体会话时派发新子智能体会自动回溯至根会话作为 parentId', async () => {
    const root = store.newThread(process.cwd())
    store.selectThread(root.id)

    const { thread: sub1 } = await store.startSubagentThread({
      subagentId: 'researcher',
      task: '第一阶段调研',
    })

    // 用户切换聚焦到了 sub1
    store.selectThread(sub1.id)
    expect(store.activeId).toBe(sub1.id)

    // 未传 parentThreadId 时向上回溯，不能把 sub1 当成 parentId 导致孤儿嵌套
    const { thread: sub2 } = await store.startSubagentThread({
      subagentId: 'researcher',
      task: '第二阶段调研',
    })

    expect(sub2.parentId).toBe(root.id)
    expect(sub2.parentId).not.toBe(sub1.id)

    store.deleteThread(root.id)
  })
})
