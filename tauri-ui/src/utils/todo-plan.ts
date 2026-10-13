/**
 * 任务规划（`todo` 工具）的**计划状态推演**——唯一真源。
 *
 * 为什么需要它：界面此前是"倒序找第一个能解析出 `items` 的 `todo` 卡片"，
 * 于是 `todo` 的三种动作（`set` / `update` / `get`）被一视同仁：
 * - `update` 只带改动项 → 解析出来的是**子集**，
 * - `get` 回的是旧快照、`set` 之前的那份还在，
 * - 早先还把"空数组"当成解析失败直接跳过。
 * 结果就是"过了几轮，面板停在旧计划上"（实测 `7/12`，而正文写着 13/13 完成）。
 *
 * 正确语义（按动作推演，而不是按卡片解析）：
 * - `set`：**替换**整份计划（基线）；
 * - `update`：**增量合并**（按 id 命中就改，没命中就追加）；
 * - `get`：**只读**，不改计划状态；
 * - 没写 `action`：当作 `set`（老数据/模型直传整份计划的形态）。
 */

export interface TodoStep {
  id?: string
  title: string
  status: 'pending' | 'in_progress' | 'completed'
}

function normalizeStatus(t: any): TodoStep['status'] {
  if (t?.status === 'completed' || t?.status === 'in_progress' || t?.status === 'pending') {
    return t.status
  }
  return t?.done === true ? 'completed' : 'pending'
}

function parseItemsValue(raw: any): TodoStep[] | null {
  if (!Array.isArray(raw)) return null
  return raw.map((t: any) => ({
    id: typeof t?.id === 'string' ? t.id : undefined,
    title: String(t?.title ?? t?.text ?? ''),
    status: normalizeStatus(t),
  }))
}

/** 从一张 `todo` 卡片里取出 `{action, items}`（认 args / args 是 JSON 字符串 / output / result）。 */
function readTodoCall(item: any): { action: string; items: TodoStep[] | null; notes?: string } | null {
  const fromArgs = (a: any) => {
    if (!a) return null
    if (typeof a === 'string') {
      if (!a.trim().startsWith('{')) return null
      try {
        return fromArgs(JSON.parse(a))
      } catch {
        return null
      }
    }
    if (typeof a !== 'object') return null
    const items = parseItemsValue(a.items) ?? parseItemsValue(a.todos)
    const action = typeof a.action === 'string' ? a.action : ''
    if (items === null && !action) return null
    return { action, items, notes: typeof a.notes === 'string' ? a.notes : undefined }
  }
  const direct = fromArgs(item?.args)
  if (direct) return direct
  const out = fromArgs(item?.output)
  if (out) return out
  return fromArgs(item?.result)
}

/**
 * 推演整条会话的**当前**计划状态。
 *
 * 返回 `null` = 这条会话没规划过。
 */
export function deriveTodoPlan(items: any[]): { todos: TodoStep[]; notes?: string } | null {
  if (!Array.isArray(items) || items.length === 0) return null

  let plan: TodoStep[] = []
  let notes: string | undefined
  let seen = false

  for (const it of items) {
    const isTodo = it?.name === 'todo' || it?.tool === 'todo'
    if (!isTodo) continue
    const call = readTodoCall(it)
    if (!call) continue
    seen = true
    if (call.notes) notes = call.notes

    const action = (call.action || 'set').toLowerCase()
    if (action === 'get') continue // 只读：不改计划状态
    if (call.items === null) continue

    if (action === 'set') {
      plan = call.items
      continue
    }
    // update（或缺省）：按 id 增量合并；没有 id 的按标题匹配
    for (const next of call.items) {
      const idx = plan.findIndex((p) =>
        next.id ? p.id === next.id : p.title === next.title
      )
      if (idx >= 0) {
        plan[idx] = { ...plan[idx], ...next, id: next.id ?? plan[idx].id }
      } else {
        plan = [...plan, next]
      }
    }
  }

  if (!seen || plan.length === 0) return null
  return { todos: plan, notes }
}
