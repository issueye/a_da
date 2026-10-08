/**
 * 复制视图的语义测试（M1-1 / M1-3 / M1-4 / M1-5 / M1-6）。
 *
 * 纯逻辑，不挂窗口：验的是"主机给的快照怎么被应用、什么时候通知、派生值从哪来"。
 * 用假来源（可控快照 + 手动发事件）而不是真 store，才能精确构造边界。
 *
 * 对应验收（`docs/ui-host-split-dev-plan.md` M1）：
 * - `client.state` 来自**主机快照**，不是主机内存对象（M1-3/M1-6）；
 * - 同一变更周期内引用稳定（不会让 React 白重渲染）；
 * - 读助手是**本地推导**（`isThreadRunning` / `labelFor` / `getThreadFileChanges`），
 *   输入只有快照数据——这是 M3 起客户端读不到主机内存的前提；
 * - `confirmModal` 是客户端本地字段，**不在快照里**（回调不可能上线）；
 * - 合帧把多次事件合并成一次发布，`flush()` 立即应用。
 */

import { describe, expect, test } from 'bun:test'
import type { ClientSnapshot, Item } from '../../shared/protocol'
import { C, applyAppearance } from '../../theme'
import { createViewStore, type SnapshotSource } from './view-store'

function makeThread(id: string, items: Item[] = []) {
  return {
    id,
    title: `会话 ${id}`,
    createdAt: 1,
    workspace: '/repo',
    items,
    messages: [],
  }
}

function makeSnapshot(patch: Partial<ClientSnapshot> = {}): ClientSnapshot {
  const threads = [makeThread('t1')]
  return {
    threads,
    activeThreadId: 't1',
    runningThreadIds: [],
    waitingThreadIds: [],
    queue: [],
    log: [],
    workspace: { project: '/repo', files: 1, dirs: 1, scanning: false, entries: ['a.ts'] },
    config: {
      model: 'm',
      contextWindow: 1000,
      supportsImages: false,
      approval: 'auto',
      effort: 'max',
      mode: 'code',
    },
    pendingQuestions: [],
    publicWorkspace: '/public',
    appearance: 'dark',
    ui: {
      activeId: 't1',
      openTabIds: ['t1'],
      pendingDraft: null,
      debugOpen: false,
      settingsOpen: false,
      pluginsOpen: false,
      changesOpen: false,
      paletteOpen: false,
      sidebarOpen: true,
      searchOpen: false,
    },
    ...patch,
  }
}

/** 可控来源：能手动发快照，也能选择是否提供"廉价陈旧判据"。 */
function makeSource(initial: ClientSnapshot, options: { staleness?: boolean } = {}) {
  const listeners = new Set<(snapshot: ClientSnapshot) => void>()
  let current = initial
  return {
    source: {
      snapshot: () => current,
      subscribe: (listener: (snapshot: ClientSnapshot) => void) => {
        listeners.add(listener)
        return () => listeners.delete(listener)
      },
      ...(options.staleness
        ? { stalenessKeys: () => [current, current.config.model, current.runningThreadIds.join(',')] }
        : {}),
    } satisfies SnapshotSource,
    /** 主机发一份新快照 */
    emit: (next: ClientSnapshot) => {
      current = next
      for (const listener of [...listeners]) listener(next)
    },
    /** 只改主机的"当前值"而不发事件（模拟漏通知） */
    setQuietly: (next: ClientSnapshot) => {
      current = next
    },
  }
}

const writeCard: Item = {
  kind: 'tool',
  id: 'card-1',
  at: 1,
  callId: 'c1',
  name: 'write_file',
  args: { path: 'src/a.ts' },
  rawArgs: '',
  status: 'done',
  patch: '--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1 +1,2 @@\n-old\n+new\n+more\n',
}

describe('复制视图：应用主机快照', () => {
  test('state 来自快照（不是主机内存对象），同一周期引用稳定', () => {
    const fake = makeSource(makeSnapshot())
    const view = createViewStore(fake.source)

    const first = view.getState()
    expect(first.activeId).toBe('t1')
    expect(first.threads[0]!.id).toBe('t1')
    expect(first.project).toBe('/repo')
    // 引用稳定：读一次状态不该让 React 白重渲染
    expect(view.getState()).toBe(first)
  })

  test('主机发新快照 → 通知一次且看到新值', () => {
    const fake = makeSource(makeSnapshot())
    const view = createViewStore(fake.source)

    let notified = 0
    view.subscribe(() => {
      notified += 1
    })
    const before = view.getState()

    fake.emit(makeSnapshot({ config: { ...makeSnapshot().config, model: 'm2' } }))
    expect(notified).toBe(1)
    expect(view.getState().currentModel).toBe('m2')
    expect(view.getState()).not.toBe(before)
  })

  test('客户端本地字段：confirmModal 不在快照里，开关走本地', () => {
    const fake = makeSource(makeSnapshot())
    const view = createViewStore(fake.source)

    expect(view.getState().confirmModal).toBeNull()
    let notified = 0
    view.subscribe(() => {
      notified += 1
    })
    let confirmed = false
    view.showConfirm({ title: '删会话', message: '确定？', onConfirm: () => (confirmed = true) })
    expect(view.getState().confirmModal?.title).toBe('删会话')
    expect(notified).toBe(1)

    view.getState().confirmModal?.onConfirm()
    expect(confirmed).toBe(true)

    view.closeConfirm()
    expect(view.getState().confirmModal).toBeNull()
    expect(notified).toBe(2)
  })
})

describe('复制视图：两项"必须落在界面进程里"的本地动作', () => {
  test('主题：应用快照时就把调色板换掉（拆分后主机换的是它自己那份）', () => {
    // 拆分前 store 与界面同进程，`store.setAppearance` 顺手就换了调色板；
    // 拆成两个进程后主机换的是**它自己**的，界面必须自己应用 —— 否则"明暗切换无效"。
    // 期望值取自公开的 applyAppearance（不写死颜色，免得调色板一改测试就假红）。
    applyAppearance('light')
    const lightCanvas = C.canvas
    applyAppearance('dark')
    const darkCanvas = C.canvas
    expect(lightCanvas).not.toBe(darkCanvas)

    const light = createViewStore(makeSource(makeSnapshot({ appearance: 'light' })).source)
    light.getState()
    expect(C.canvas).toBe(lightCanvas)

    const dark = createViewStore(makeSource(makeSnapshot({ appearance: 'dark' })).source)
    dark.getState()
    expect(C.canvas).toBe(darkCanvas)
  })

  test('焦点：点会话立刻生效（不等主机），主机确认到达后覆盖自行清掉', () => {
    const fake = makeSource(makeSnapshot())
    const view = createViewStore(fake.source)
    expect(view.getState().activeId).toBe('t1')

    // 本地即时生效：还没收到任何主机快照，就已经切过去了
    view.focusThread('t2')
    expect(view.getState().activeId).toBe('t2')

    // 主机镜像与本地一致 → 清掉覆盖（此后完全听主机的）
    fake.emit(makeSnapshot({ ui: { ...makeSnapshot().ui, activeId: 't2' } }))
    expect(view.getState().activeId).toBe('t2')

    // 主机还没跟上时，本地焦点不被旧快照拽回去
    view.focusThread('t3')
    fake.emit(makeSnapshot({ ui: { ...makeSnapshot().ui, activeId: 't2' } }))
    expect(view.getState().activeId).toBe('t3')
  })
})

describe('复制视图：读助手本地推导（不再问主机）', () => {
  test('isThreadRunning / isThreadWaiting 由快照里的集合推导', () => {
    const fake = makeSource(makeSnapshot({ runningThreadIds: ['t1'], waitingThreadIds: [] }))
    const view = createViewStore(fake.source)

    const state = view.getState()
    expect(state.isThreadRunning('t1')).toBe(true)
    expect(state.isThreadWaiting('t1')).toBe(false)
    expect(state.running).toBe(true) // 焦点会话在跑
  })

  test('labelFor / isPublic 由快照里的公共区路径推导', () => {
    const fake = makeSource(makeSnapshot({ publicWorkspace: '/public' }))
    const view = createViewStore(fake.source)
    const state = view.getState()

    expect(state.isPublic('/public')).toBe(true)
    expect(state.isPublic('E:\\repo'.replace(/\\/g, '\\'))).toBe(false)
    expect(state.labelFor('/public')).toBe('公共区')
  })

  test('getThreadFileChanges / getThreadChangeCount 由会话条目推导', () => {
    const fake = makeSource(makeSnapshot({ threads: [makeThread('t1', [writeCard])] }))
    const view = createViewStore(fake.source)

    const changes = view.getState().getThreadFileChanges('t1')
    expect(changes).toHaveLength(1)
    expect(changes[0]!.path).toBe('src/a.ts')
    expect(changes[0]!.editsCount).toBe(1)
    expect(changes[0]!.additions).toBe(2)
    expect(changes[0]!.deletions).toBe(1)
    expect(view.getState().getThreadChangeCount('t1')).toBe(1)

    // 未知会话：空结果，不抛错
    expect(view.getState().getThreadFileChanges('不存在')).toEqual([])
  })
})

describe('复制视图：合帧与显式提交', () => {
  test('窗口内多次快照合并成一次发布；flush 立即应用', async () => {
    const fake = makeSource(makeSnapshot())
    const view = createViewStore(fake.source, { coalesceMs: 16 })

    let notified = 0
    view.subscribe(() => {
      notified += 1
    })

    fake.emit(makeSnapshot({ config: { ...makeSnapshot().config, model: 'm2' } }))
    fake.emit(makeSnapshot({ config: { ...makeSnapshot().config, model: 'm3' } }))
    expect(notified).toBe(0) // 还在窗口里

    await new Promise((resolve) => setTimeout(resolve, 40))
    expect(notified).toBe(1) // 合并成一次
    expect(view.getState().currentModel).toBe('m3') // 保留最后一次

    fake.emit(makeSnapshot({ config: { ...makeSnapshot().config, model: 'm4' } }))
    view.flush()
    expect(notified).toBe(2)
    expect(view.getState().currentModel).toBe('m4')
  })

  test('进程内替身提供廉价陈旧判据时，漏通知也能被发现（防漏安全网）', () => {
    const fake = makeSource(makeSnapshot(), { staleness: true })
    const view = createViewStore(fake.source)

    expect(view.getState().currentModel).toBe('m')

    // 主机改了当前值但**没发事件**（模拟漏通知）
    fake.setQuietly(makeSnapshot({ config: { ...makeSnapshot().config, model: '悄悄改了' } }))
    expect(view.getState().currentModel).toBe('悄悄改了')

    const publishes = view.publishes
    view.getState() // 没变化：不该重建
    expect(view.publishes).toBe(publishes)
  })
})
