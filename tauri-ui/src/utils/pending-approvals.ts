/**
 * 待批准工具调用的提取与预览（纯函数，无 React / 无宿主依赖）。
 *
 * 背景：审批卡原先只活在对话流里（`Transcript` 的 ToolCard），会话一长就被推到视口外，
 * 用户看不到、也不敢闭眼点批准。现在审批以**浮动窗口**的形式贴在输入框上方，
 * 那份"要批准什么"的内容必须和对话流里说的是同一份——所以预览逻辑收敛到这里，
 * `Transcript`（对话流里的记录态）与 `ApprovalFloatingPanel`（浮窗）共用同一个函数。
 */

import type { Item } from '../types'

/** 一条"等待用户批准"的工具调用（界面据此渲染审批浮窗的一条）。 */
export interface PendingApproval {
  /** 传给 `approval.decide` 的 `toolItemId`（优先 `callId`，与后端 `call_id` 对齐） */
  id: string
  /** 工具名（内置工具是英文 id，插件工具可能是 `scope:文件名`） */
  tool: string
  /** 待批准内容的预览：危险命令原文 / 目标路径 / 参数原文；取不到时是空串 */
  preview: string
  /** 卡片入列时刻（毫秒），用于显示"已等待多久" */
  at?: number
}

/** 提问工具：等的是"回答"不是"批准"，归提问面板管，不能混进审批浮窗 */
const QUESTION_TOOLS = new Set(['ask_user'])

function firstString(args: any, keys: string[]): string {
  if (!args || typeof args !== 'object') return ''
  for (const key of keys) {
    const v = (args as any)[key]
    if (typeof v === 'string' && v.trim()) return v
  }
  return ''
}

function previewFromArgs(tool: string, args: any, limit: number): string {
  const cmd = typeof args === 'string' ? args : firstString(args, ['command', 'cmd'])
  if (cmd) return `$ ${cmd}`

  const path = firstString(args, ['path', 'file', 'filePath', 'target'])
  if (path && path !== tool) return `${tool} → ${path}`

  if (args && typeof args === 'object') {
    const json = JSON.stringify(args, null, 2)
    if (json && json !== '{}') return json.slice(0, limit)
  }
  return typeof args === 'string' ? args.slice(0, limit) : ''
}

/**
 * 审批条上"要批准什么"的预览（危险命令原文 / 目标路径 / 参数原文）。
 *
 * 宿主在 `ApprovalRequested` 事件里带上了 args，卡片建出来就带着它；
 * 万一某条老路径没带，还要退到 `details.approval.args`（`set_tool_waiting_approval`
 * 把同一份参数也放在那里）——**绝不能什么都不显示**，那就又回到"只能闭眼点批准"。
 * 两处都取不到时返回空串，界面据此显式提示"参数缺失、谨慎批准"。
 */
export function approvalPreviewOf(item: Item, limit = 1200): string {
  const tool = item.tool || item.name || 'tool'
  const fromCard = previewFromArgs(tool, item.args, limit)
  if (fromCard) return fromCard
  // 兜底：审批详情里那份参数（args 为空对象 / 老卡片 / 增量帧没带上时）
  return previewFromArgs(tool, (item.details as any)?.approval?.args, limit)
}

/**
 * 从会话 items 里挑出**正在等待批准**的工具调用（按发生顺序）。
 *
 * 判据与 `Transcript` 的 `isAwaiting` 保持一致：`waiting_approval`（审批闸门）与
 * `awaiting`（等待用户操作）。`ask_user` 的 `awaiting` 是提问、不是审批，排除掉。
 *
 * 工具执行是顺序的（审批一次只问一件事），实际通常只有一条；这里仍返回数组，
 * 免得将来并发审批时浮窗只显示一条、剩下的静默挂到超时。
 */
export function collectPendingApprovals(items: Item[]): PendingApproval[] {
  const out: PendingApproval[] = []
  for (const item of items) {
    const tool = item.tool || item.name || ''
    if (QUESTION_TOOLS.has(tool)) continue
    if (item.question || (item.details as any)?.question) continue

    const status = item.state || item.status
    if (status !== 'waiting_approval' && status !== 'awaiting') continue

    const id = item.callId || item.id
    if (!id) continue

    out.push({
      id,
      tool: tool || 'tool',
      preview: approvalPreviewOf(item),
      at: item.at ?? item.startedAt,
    })
  }
  return out
}
