/**
 * 会话状态解析（纯函数，无 React / 无桌面壳依赖）。
 *
 * 从 `components/Sidebar.tsx` 抽出来**不是为了好看**：侧边栏与标签页共用同一份判定，
 * 而这里曾经有一个只有把真数据喂进去才看得见的 bug（见下面 `resolveThreadStatus` 的 3a/3b）。
 * 纯函数 + 无副作用 = 可以用一段脚本直接对着**真实快照**跑，而不是靠盯界面猜。
 *
 * 判定优先级（顺序即语义，别随意调换）：
 * 1. 等待用户操作（审批/提问）—— 它比"执行中"更需要被看见；
 * 2. 全局运行集合 `runningThreadIds`；
 * 3. 会话自身 items 的运行态；
 * 4. 子智能体：到父会话里找**它那一张**委派卡片；
 * 5. 自身 items 的终态；
 * 6. 空会话 → 就绪。
 */

import type { Item, Thread } from '../types'

export type ThreadStatus = 'running' | 'waiting' | 'failed' | 'completed' | 'idle'

export interface ThreadStatusInfo {
  status: ThreadStatus
  label: string
}

/** 委派子智能体的工具名（与 `ToolDescriptor` 注册表一致）。 */
export const DELEGATION_TOOL = 'invoke_subagent'

/** 兜底匹配的时间窗：同一父会话里同角色子代理的委派卡片只认这个范围内的。 */
const DELEGATION_FALLBACK_WINDOW_MS = 30 * 60 * 1000

/** 这张卡片是不是一次子智能体委派。 */
export function isDelegationItem(it: Item): boolean {
  return (it.name || it.tool) === DELEGATION_TOOL
}

/**
 * 取出「这张委派卡片**确切**绑定的子会话 id」，拿不到就返回 `undefined`。
 *
 * 这是本模块唯一可信的"卡片 ↔ 子会话"纽带，全部字段形态都在这里收口：
 * 回执 `details`（`subagent_thread_id` / `threadId`，见 `agent-node` 的 `SubagentTool`）、
 * 结构化结果、以及多轮续跑时调用参数里的 `thread_id`。
 *
 * 🔴 拿不到时**必须**返回 `undefined`，不要退化成"按角色猜一个"——把"猜"留在调用方
 * 显式地做（并排除已被别人认领的卡片），否则就会出现"张冠李戴"。
 */
export function boundSubagentThreadId(it: Item): string | undefined {
  const d: any = it.details || {}
  const a: any = (it.args as any) || {}
  const r: any = (it.result as any) || {}
  const raw =
    d.subagent_thread_id ??
    d.threadId ??
    d.thread_id ??
    r.subagent_thread_id ??
    r.threadId ??
    a.thread_id ??
    a.threadId
  return typeof raw === 'string' && raw.trim() ? raw : undefined
}

/** 这张委派卡片声明的子智能体角色（`subagent_id` / `role`）。 */
function delegationRole(it: Item): string | undefined {
  const d: any = it.details || {}
  const a: any = (it.args as any) || {}
  const raw = a.subagent_id ?? a.role ?? d.subagent_id
  return typeof raw === 'string' && raw.trim() ? raw : undefined
}

/** 卡片的开始时刻（排序锚点）：优先真实开始时间，退回落盘时间。 */
function itemTime(it: Item): number {
  return it.startedAt || it.createdAt || 0
}

/** 解析并计算会话（包括子智能体）的当前执行状态。 */
export function resolveThreadStatus(
  thread: Thread,
  allThreadMap: Map<string, Thread>,
  runningThreadIds: string[]
): ThreadStatusInfo {
  const items = thread.items || []

  // 1. **等待用户操作优先于"执行中"**：审批/提问等待时，轮次仍在跑（runningThreadIds 里
  //    也有它），早先把 running 判在前面，于是标签页一直显示"执行中"——用户根本不知道
  //    有东西在等他（实测一条审批请求就这样静默等了 5 分钟超时被拒）。
  const hasAwaitingItem = items.some(
    (it) =>
      it.status === 'waiting_approval' ||
      it.status === 'awaiting' ||
      it.state === 'waiting_approval' ||
      it.state === 'awaiting'
  )
  if (hasAwaitingItem) {
    return { status: 'waiting', label: '等待审批' }
  }

  // 2. 全局运行集合判定
  if (runningThreadIds.includes(thread.id)) {
    return { status: 'running', label: '执行中' }
  }

  // 3. 会话内部 items 的运行态
  const hasRunningItem = items.some(
    (it) => it.status === 'running' || it.state === 'running'
  )
  if (hasRunningItem) {
    return { status: 'running', label: '执行中' }
  }

  // 4. 子智能体：关联检查父会话中的对应委派卡片状态
  //
  // 🔴 这里修的是一个实测到的"张冠李戴"（用户报的：子代理**还在跑**，侧边栏却是绿勾"执行完成"）。
  //
  // 原实现是一次 `parentItems.find(精确 id 或 同角色且 30 分钟内)`，而兜底那一支**过宽**：
  // 一个父会话先后派了多个**同角色**子智能体时（实测 PM 一次并行派了两个 `researcher`），
  // 排在前面的那张卡片（已 `done`）会先被 `find` 命中，于是**后面那个还在跑的子会话
  // 继承了前一个的"完成"**——侧边栏绿勾、标签页也绿勾，而它其实每分钟都在调工具。
  //
  // 现在的规则是**两趟**，且兜底支只认"还没被认领的卡片"：
  //   4a. 全表精确匹配绑定 id（唯一可信的纽带）；
  //   4b. 老数据没落 `threadId` 时才退到"角色吻合 + 时间最近"，且**已绑定给别的子会话的
  //       卡片一律排除**——这样同角色的多个子代理不会互相顶替。
  //
  // 顺手说明为什么"只靠 4b 也能对"：子会话的 `createdAt` 就是它的委派卡片开始的那一刻，
  // 取时间最近的那个必然是本卡片；被排除的那些早已名花有主。
  if (thread.parentId && allThreadMap.has(thread.parentId)) {
    const parent = allThreadMap.get(thread.parentId)!
    const delegations = (parent.items || []).filter(isDelegationItem)

    // 4a. 精确：卡片回执/参数里写明的就是这个子会话
    let delegationItem = delegations.find((it) => boundSubagentThreadId(it) === thread.id)

    // 4b. 兜底：只认未认领的卡片，且取时间最近的（不是"排在前面的"）
    if (!delegationItem) {
      const claimed = new Set<string>()
      for (const it of delegations) {
        const bound = boundSubagentThreadId(it)
        if (bound) claimed.add(bound)
      }
      const threadTime = thread.createdAt || 0
      const candidates = delegations.filter((it) => {
        if (boundSubagentThreadId(it)) return false
        const role = delegationRole(it)
        const roleOk = Boolean(
          role &&
            ((thread.subagentId && thread.subagentId === role) ||
              (thread.title || '').includes(role))
        )
        if (!roleOk) return false
        return Math.abs(itemTime(it) - threadTime) < DELEGATION_FALLBACK_WINDOW_MS
      })
      candidates.sort(
        (a, b) =>
          Math.abs(itemTime(a) - threadTime) - Math.abs(itemTime(b) - threadTime)
      )
      delegationItem = candidates[0]
    }

    if (delegationItem) {
      const toolStatus = (delegationItem.status || delegationItem.state) as string | undefined
      if (toolStatus === 'running') {
        return { status: 'running', label: '执行中' }
      }
      if (toolStatus === 'waiting_approval' || toolStatus === 'awaiting') {
        return { status: 'waiting', label: '等待审批' }
      }
      if (
        toolStatus === 'failed' ||
        toolStatus === 'error' ||
        Boolean(delegationItem.error) ||
        (delegationItem.result as any)?.ok === false
      ) {
        return { status: 'failed', label: '执行失败' }
      }
      if (
        toolStatus === 'done' ||
        toolStatus === 'success' ||
        (delegationItem.result as any)?.ok === true
      ) {
        return { status: 'completed', label: '执行完成' }
      }
    }
  }

  // 5. 检查自身 items 的终态
  if (items.length > 0) {
    const lastItem = items[items.length - 1]
    const lastStatus = lastItem.status || lastItem.state
    const hasFatalError =
      lastStatus === 'failed' ||
      lastStatus === 'error' ||
      lastStatus === 'denied' ||
      // `timeout`：引擎的策略超时切断（工具根本没返回）——不能算"执行完成"
      lastStatus === 'timeout' ||
      Boolean(lastItem.error)

    if (hasFatalError) {
      return { status: 'failed', label: '执行失败' }
    }

    const recentToolError = items.slice(-3).some((it) => {
      const s = it.status || it.state
      return s === 'failed' || s === 'error' || s === 'timeout' || Boolean(it.error)
    })
    if (recentToolError && lastItem.role !== 'assistant') {
      return { status: 'failed', label: '执行失败' }
    }

    return { status: 'completed', label: '执行完成' }
  }

  // 6. 空会话
  return { status: 'idle', label: '就绪' }
}
