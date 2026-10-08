/**
 * 进程内客户端的端到端语义（M1 验收里"多客户端 / 重连"在进程内的形态）。
 *
 * 验四件事：
 * 1. **种子**：新客户端一上来就拿到当前状态（对应"连上就要一份快照"，协议 §1.3）；
 * 2. **独立应用**：两个客户端各自应用同一份快照，互不干扰；
 * 3. **命令打到主机**：`request` 真的落到 store 上，两个客户端随后都能看到结果；
 * 4. **客户端本地字段各自独立**：确认框属于**每个客户端自己**（回调不可能跨客户端），
 *    A 打开不影响 B。
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { store } from '../../agent/store'
import { createInProcessClient } from './in-process'

const created: string[] = []

afterEach(async () => {
  for (const id of created.splice(0)) {
    await store.deleteThread(id)
  }
})

describe('进程内客户端：种子与独立应用', () => {
  test('新客户端立刻拿到当前状态（等于"连上先要一份快照"）', () => {
    const client = createInProcessClient(store)
    const state = client.state

    expect(state.activeId).toBe(store.activeId)
    expect(state.threads.length).toBe(store.threads.length)
    expect(state.project).toBe(store.project)
    // 读助手是本地推导出来的，不依赖主机内存对象
    expect(state.isThreadRunning(store.activeId)).toBe(store.isThreadRunning(store.activeId))
  })

  test('两个客户端各自应用同一份快照：一个变了，另一个跟着变；本地字段互不干扰', async () => {
    const a = createInProcessClient(store)
    const b = createInProcessClient(store)

    const before = a.state.threads.length
    expect(b.state.threads.length).toBe(before)

    // 命令打到主机 → 主机广播 → 两个客户端各自应用
    const { threadId } = await a.request('thread.create', { workspace: store.project })
    created.push(threadId)

    expect(a.state.threads.some((thread) => thread.id === threadId)).toBe(true)
    expect(b.state.threads.some((thread) => thread.id === threadId)).toBe(true)

    // 客户端本地字段：A 打开确认框不影响 B
    a.ui.showConfirm({ title: 'A 的确认框', message: '', onConfirm: () => {} })
    expect(a.state.confirmModal?.title).toBe('A 的确认框')
    expect(b.state.confirmModal).toBeNull()

    a.ui.closeConfirm()
    expect(a.state.confirmModal).toBeNull()
  })

  test('M1 边界：进程内嵌套对象是共享引用（M3 起才会是副本）；refreshState 随时可用', () => {
    const client = createInProcessClient(store)
    const target = store.active
    const before = target.title

    // 改**嵌套字段**：快照里放的是同一个 Thread 对象的引用，所以立刻可见——
    // 这是 M1 的既定边界（进程内共享引用），M3 起了反序列化副本后才会需要显式提交。
    target.title = `${before} · 改过`
    expect(client.state.active.title).toBe(`${before} · 改过`)

    target.title = before

    // 改**顶层字段**：即使不通知，"廉价结构判据"也会发现（防漏安全网）
    const modelBefore = store.currentModel
    store.currentModel = '测试模型'
    expect(client.state.currentModel).toBe('测试模型')
    store.currentModel = modelBefore

    // refreshState 是协议无关的提交通道：随时调用都保持自洽
    client.refreshState()
    expect(client.state.currentModel).toBe(modelBefore)
    expect(client.state.activeId).toBe(store.activeId)
  })
})
