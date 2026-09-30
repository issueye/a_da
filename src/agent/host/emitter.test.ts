/**
 * 主机侧发射器的语义测试（M1-2 / M1-3）。
 *
 * 验的是"主机怎么产出事件"：挂点只有一处（`store.subscribe`，那 88 处 `notify()` 的共同出口）、
 * `seq` 单调、合帧窗口、以及快照里**该有的字段都在**（覆盖度清单的守门）。
 *
 * 用真 store 单例（`A_DA_HOME` 由 `scripts/test-preload.ts` 指到临时目录），
 * 因为这里要验的正是"store 的广播真的被接上了"——用假来源就绕过了唯一挂点。
 */

import { describe, expect, test } from 'bun:test'
import { store } from '../store'
import { createHostEmitter } from './emitter'
import { readHostSnapshot } from './snapshot'

/**
 * 手动的广播源：把 listen 交给测试驱动。
 *
 * 为什么需要它：真 store 上一次 `trace()` 会产生**不止一次**广播（`push` 自己也通知），
 * 用它验"三次广播合成一个事件"就不精确了。
 */
function manualSource() {
  let listener: (() => void) | null = null
  return {
    subscribeToSource: (next: () => void): (() => void) => {
      listener = next
      return () => {
        listener = null
      }
    },
    /** 在函数体里读，避免外层控制流把 listener 收窄成 null */
    fire: (): void => {
      if (listener) listener()
    },
  }
}

describe('主机发射器：当前快照', () => {
  test('快照带齐 UI 要的字段（覆盖度守门）', () => {
    const snapshot = readHostSnapshot(store)

    // 会话与运行状态
    expect(Array.isArray(snapshot.threads)).toBe(true)
    expect(Array.isArray(snapshot.runningThreadIds)).toBe(true)
    expect(Array.isArray(snapshot.waitingThreadIds)).toBe(true)
    // 队列 / 日志 / 工作区 / 配置 / 待答提问
    expect(Array.isArray(snapshot.queue)).toBe(true)
    expect(Array.isArray(snapshot.log)).toBe(true)
    expect(typeof snapshot.workspace.files).toBe('number')
    expect(Array.isArray(snapshot.workspace.entries)).toBe(true)
    expect(typeof snapshot.config.model).toBe('string')
    expect(['auto', 'ask', 'readonly']).toContain(snapshot.config.approval)
    expect(Array.isArray(snapshot.pendingQuestions)).toBe(true)
    // 推导输入
    expect(typeof snapshot.publicWorkspace).toBe('string')
    // 纯客户端状态的主机镜像（M2 会搬走）
    expect(typeof snapshot.ui.activeId).toBe('string')
    expect(Array.isArray(snapshot.ui.openTabIds)).toBe(true)
    expect(snapshot.ui).toHaveProperty('pendingDraft')
    // 确认框**不在**快照里：它带回调，永远不可能上线
    expect(snapshot).not.toHaveProperty('confirmModal')
  })

  test('stalenessKeys 是一组可比较的原始值', () => {
    const keys = createHostEmitter(store).stalenessKeys()
    expect(Array.isArray(keys)).toBe(true)
    expect(keys.length).toBeGreaterThan(10)
    // 重复调用长度稳定（它就是拿来做逐项比对的）
    expect(createHostEmitter(store).stalenessKeys().length).toBe(keys.length)
  })
})

describe('主机发射器：事件流', () => {
  test('挂点接在 store 的广播上：一次 notify → 一个事件，seq 单调递增', () => {
    const emitter = createHostEmitter(store, { coalesceMs: 0 })
    const seqs: number[] = []
    const unsubscribe = emitter.subscribe((event) => {
      expect(event.topic).toBe('evt.state.snapshot')
      seqs.push(event.seq)
    })

    store.trace('发射器测试 1')
    store.trace('发射器测试 2')

    unsubscribe()
    emitter.dispose()

    expect(seqs).toHaveLength(2)
    expect(seqs[1]!).toBeGreaterThan(seqs[0]!)
  })

  test('合帧：窗口内多次广播合并成一个事件（M1-5 的机制）', async () => {
    const manual = manualSource()
    const emitter = createHostEmitter(store, { coalesceMs: 16, subscribeToSource: manual.subscribeToSource })
    let events = 0
    const unsubscribe = emitter.subscribe(() => {
      events += 1
    })

    manual.fire()
    manual.fire()
    manual.fire()
    expect(events).toBe(0) // 还在窗口里

    await new Promise((resolve) => setTimeout(resolve, 40))
    unsubscribe()
    emitter.dispose()

    expect(events).toBe(1) // 三次合成一次
  })

  test('flush 绕过窗口立刻发一次', () => {
    const manual = manualSource()
    const emitter = createHostEmitter(store, {
      coalesceMs: 60_000,
      subscribeToSource: manual.subscribeToSource,
    })
    let events = 0
    const unsubscribe = emitter.subscribe(() => {
      events += 1
    })

    manual.fire()
    emitter.flush()

    unsubscribe()
    emitter.dispose()

    expect(events).toBe(1)
  })
})
