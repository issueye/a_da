/**
 * 复制视图的语义测试（M1-1 / M1-4 / M1-5 / M1-6）。
 *
 * 纯逻辑，不挂窗口：这里验的是"快照什么时候重建、什么时候通知"，
 * 与 GPUIX 无关。用假来源（可控对象 + 手动通知）而不是真 store，
 * 才能精确构造"改了但不通知"这类边界。
 *
 * 对应验收（`docs/ui-host-split-dev-plan.md` M1）：
 * - `client.state` 不再是**来源本身**（M1-6 的前提）；
 * - 来源通知后能看到变化；**没通知但结构变了**也能看到（防漏安全网）；
 * - 多次通知在同一窗口内**合并成一次**（M1-5）；
 * - `flush()` 立即发布（测试造数据后的显式提交，M1-7 的通道）。
 */

import { describe, expect, test } from 'bun:test'
import type { ClientState } from './types'
import { createViewStore } from './view-store'

/** 可控来源：构造一个够用的 ClientState，并暴露 `notify()` 与直接改字段的能力。 */
function makeSource() {
  const listeners = new Set<() => void>()
  const items: Array<{ kind: 'user'; id: string; at: number; text: string }> = []
  const thread = {
    id: 't1',
    title: '标题',
    createdAt: 1,
    workspace: '/w',
    items: items as unknown[],
    messages: [],
  }

  const source = {
    threads: [thread],
    activeId: 't1',
    active: thread,
    queue: [],
    log: [],
    workspaceInfo: { files: 1, dirs: 1, scanning: false },
    entries: ['a.ts'],
    currentModel: 'm',
    contextWindow: 1000,
    supportsImages: false,
    approval: 'auto',
    effort: 'max',
    activeThreadStats: {},
    pendingAnswerQuestions: [],
    project: '/w',
    projects: ['/w'],
    projectThreads: [thread],
    openTabs: [thread],
    mode: 'code',
    running: false,
    pendingDraft: null as string | null,
    appearance: 'dark',
    debugOpen: false,
    settingsOpen: false,
    pluginsOpen: false,
    changesOpen: false,
    paletteOpen: false,
    sidebarOpen: true,
    searchOpen: false,
    confirmModal: null,
    isThreadRunning: () => false,
    isThreadWaiting: () => false,
    labelFor: (workspace: string) => workspace,
    isPublic: () => false,
    getThreadChangeCount: () => 0,
    getThreadFileChanges: () => [],
  } as unknown as ClientState

  return {
    source,
    notify: () => {
      for (const listener of [...listeners]) listener()
    },
    subscribe: (listener: () => void) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    /** 直接改字段的入口（模拟测试里造数据） */
    patch: (fields: Partial<Record<string, unknown>>) => {
      Object.assign(source as unknown as Record<string, unknown>, fields)
    },
    pushItem: (text: string) => {
      items.push({ kind: 'user', id: `i${items.length}`, at: 1, text })
    },
  }
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

describe('复制视图：快照与陈旧检测', () => {
  test('getState 返回的是快照，不是来源本身；同一变更周期内引用稳定', () => {
    const fake = makeSource()
    const view = createViewStore(fake.source, { subscribeToSource: fake.subscribe, coalesceMs: 0 })

    const first = view.getState()
    expect(first).not.toBe(fake.source as unknown)
    expect(first.activeId).toBe('t1')
    // 引用稳定：React 不会因为"读一次状态"就白重渲染
    expect(view.getState()).toBe(first)
  })

  test('来源通知后能看到变化；没有通知但结构变了也能看到（防漏安全网）', () => {
    const fake = makeSource()
    const view = createViewStore(fake.source, { subscribeToSource: fake.subscribe, coalesceMs: 0 })

    const before = view.getState()
    expect(before.pendingDraft).toBeNull()

    // ① 通知驱动的变化
    fake.patch({ pendingDraft: '来自通知' })
    fake.notify()
    expect(view.getState().pendingDraft).toBe('来自通知')
    expect(view.getState()).not.toBe(before)

    // ② 没通知、直接改字段（测试造数据的常见写法）也要能看到
    fake.patch({ pendingDraft: '没通知也要看到' })
    expect(view.getState().pendingDraft).toBe('没通知也要看到')

    // ③ 数组长度变化（push 型改动）同样兜住
    const itemsBefore = view.getState().active.items.length
    fake.pushItem('新条目')
    expect(view.getState().active.items.length).toBe(itemsBefore + 1)
  })

  test('多次通知在同一窗口内合并成一次；flush 立即发布', async () => {
    const fake = makeSource()
    const view = createViewStore(fake.source, { subscribeToSource: fake.subscribe, coalesceMs: 16 })

    let notified = 0
    view.subscribe(() => {
      notified += 1
    })

    // 连续三次通知：合帧窗口内只该通知一次
    fake.notify()
    fake.notify()
    fake.notify()
    expect(notified).toBe(0) // 还没到窗口
    await sleep(40)
    expect(notified).toBe(1)

    // flush 绕过窗口：立刻发布并通知
    fake.patch({ pendingDraft: 'flush 提交' })
    view.flush()
    expect(notified).toBe(2)
    expect(view.getState().pendingDraft).toBe('flush 提交')
  })

  test('每次重建都让 publishes 递增，便于诊断"到底刷了几次"', () => {
    const fake = makeSource()
    const view = createViewStore(fake.source, { subscribeToSource: fake.subscribe, coalesceMs: 0 })

    const before = view.publishes
    view.getState() // 首次访问：建立快照
    const afterFirst = view.publishes
    expect(afterFirst).toBeGreaterThanOrEqual(before)

    view.getState() // 无变化：不该重建
    expect(view.publishes).toBe(afterFirst)

    fake.patch({ currentModel: 'm2' })
    view.getState()
    expect(view.publishes).toBeGreaterThan(afterFirst)
  })
})
