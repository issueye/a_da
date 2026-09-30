/**
 * 主机侧的事件发射器（M1-2）。
 *
 * ## 挂点只有一处
 *
 * 计划 M1-2 原写的是"88 处 `notify()` 逐条归类为 `evt.*`"。实测发现 `store.subscribe`
 * 就是那 88 处**共同的唯一广播出口**——所以这里挂一次即可：改动面从 88 处降到 1 处，
 * 而且**不存在"漏一处 notify"这种漏法**。那 88 处的价值随之从"逐个改造"变成
 * **快照覆盖度清单**：哪些状态必须进 `readHostSnapshot`。
 *
 * ## M1 只发一种事件
 *
 * `evt.state.snapshot`（整份快照，带 `seq`）——与计划 M1-4 的"先粗后细"一致：
 * 先把"主机产出 → 客户端应用"这条链跑通，再按测量把高频路径拆成增量
 * （M3 的 `evt.message.delta` / `evt.card.updated`）。
 */

import type { AgentStore } from '../store'
import type { ClientSnapshot, SnapshotEvent } from '../../shared/protocol'
import { readHostSnapshot, stalenessKeys } from './snapshot'

export interface HostEmitterOptions {
  /**
   * 合帧窗口（ms）。0 = 同步发（进程内默认）。
   *
   * 进程内没有带宽要省，而"同步发"与今天的行为完全一致（`notify()` 就是同步广播）；
   * WebSocket 传输才需要 16–33ms 的合并窗口（协议 §7.1）。
   */
  coalesceMs?: number
  /**
   * 广播源。默认接 `store.subscribe`（就是那 88 处 `notify()` 的共同出口）。
   *
   * 留成可注入是为了**测试能精确驱动**：真 store 上一次 `trace()` 会产生不止一次广播
   * （`push` 自己也会通知），用来验"三次广播合成一个事件"就不精确了。
   */
  subscribeToSource?: (listener: () => void) => () => void
}

export interface HostEmitter {
  /** 当前快照（初次连接时先给一份，协议 §1.3 的 `session.snapshot`）。 */
  snapshot(): ClientSnapshot
  /** 廉价结构判据：供进程内替身判断"来源变了但没通知"。 */
  stalenessKeys(): unknown[]
  /** 订阅事件流；返回退订函数。 */
  subscribe(listener: (event: SnapshotEvent) => void): () => void
  /** 立刻发一次（绕过合帧窗口）。 */
  flush(): void
  /** 已发事件数（每次发射 +1），诊断用。 */
  readonly emitted: number
  /** 退订来源并清空监听者（重建客户端时用；进程内一般活到进程结束）。 */
  dispose(): void
}

export function createHostEmitter(store: AgentStore, options: HostEmitterOptions = {}): HostEmitter {
  const coalesceMs = options.coalesceMs ?? 0
  const listeners = new Set<(event: SnapshotEvent) => void>()
  let seq = 0
  let emitted = 0
  let timer: ReturnType<typeof setTimeout> | null = null

  function emit(): void {
    seq += 1
    emitted += 1
    const event: SnapshotEvent = { seq, topic: 'evt.state.snapshot', payload: readHostSnapshot(store) }
    for (const listener of [...listeners]) listener(event)
  }

  function schedule(): void {
    if (coalesceMs <= 0) {
      emit()
      return
    }
    if (timer !== null) return
    timer = setTimeout(() => {
      timer = null
      emit()
    }, coalesceMs)
  }

  // 唯一的挂点：那 88 处 notify() 的共同出口
  const subscribeToSource = options.subscribeToSource ?? ((listener) => store.subscribe(listener))
  const unsubscribeSource = subscribeToSource(() => schedule())

  return {
    snapshot: () => readHostSnapshot(store),
    stalenessKeys: () => stalenessKeys(store),
    subscribe(listener) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    flush() {
      if (timer !== null) {
        clearTimeout(timer)
        timer = null
      }
      emit()
    },
    get emitted() {
      return emitted
    },
    dispose() {
      unsubscribeSource()
      listeners.clear()
      if (timer !== null) {
        clearTimeout(timer)
        timer = null
      }
    },
  }
}
