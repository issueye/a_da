/**
 * 审批浮窗（ApprovalFloatingPanel）
 *
 * 🔴 审批原先只在对话流里以工具卡片的形式出现：会话一长就被推到视口外，
 * 用户既看不到"正在等批准"，也不知道"到底在批准哪条命令"——实测一条 `cargo test`
 * 的审批就这样静默等到超时被自动拒绝。
 *
 * 现在审批**以浮动窗口的形式覆盖在对话内容之上**（绝对定位挂在 Composer 的容器里，
 * 输入框区域不随对话流滚动，因此始终可见）：
 * - 它是**覆盖层，不占布局空间**：出现/消失都不会把输入框或对话内容顶来顶去；
 * - 点名工具 + 等待时长，开门见山说明"这次调用会修改工作区"；
 * - 摆出待批准内容的原文（危险命令 / 目标路径 / 参数），**看不到内容就不该点批准**；
 * - 批准 / 拒绝按钮就在这里，不需要先去对话流里找那张卡。
 *
 * 对话流里的那张卡不再是操作面（只留记录与内容预览），避免同一件事出现两套按钮。
 */

import React, { useEffect, useState } from 'react'
import { Shield, ShieldAlert } from 'lucide-react'
import { TOOL_LABEL } from '../utils/tool-labels'
import type { PendingApproval } from '../utils/pending-approvals'

export interface ApprovalFloatingPanelProps {
  /** 当前会话正在等待批准的工具调用（按发生顺序；通常只有一条） */
  approvals: PendingApproval[]
  /** 批准 / 拒绝回执入口（对应 `approval.decide`） */
  onDecide?: (toolItemId: string, approved: boolean) => void
  className?: string
}

/** 已等待时长：审批有超时（超时按安全方向自动拒绝），把等待摆在明面上。 */
const WaitedSince: React.FC<{ since?: number }> = ({ since }) => {
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [])

  if (!since || since <= 0) return null
  const seconds = Math.max(0, Math.floor((now - since) / 1000))
  const text =
    seconds < 60
      ? `${seconds} 秒`
      : `${Math.floor(seconds / 60)} 分 ${String(seconds % 60).padStart(2, '0')} 秒`

  return (
    <span
      className="flex-shrink-0 text-[10px] font-mono text-amber-700/90 dark:text-amber-300/80"
      title="从收到审批请求开始计时；超时未处理会按安全方向自动拒绝"
    >
      已等待 {text}
    </span>
  )
}

export const ApprovalFloatingPanel: React.FC<ApprovalFloatingPanelProps> = ({
  approvals,
  onDecide,
  className,
}) => {
  /// 已点过、但快照还没回来的条目（点批准 → 引擎继续 → 下一帧快照实测要几秒）。
  ///
  /// 没有它，用户点完看不到任何反应会连点好几下——"点了没反应"正是这个仓库反复
  /// 修过的那类体感 bug（见 `ws-client.decideApproval` 的四段耗时日志）。
  const [inFlight, setInFlight] = useState<Record<string, boolean>>({})
  const approvalIds = (approvals || []).map((a) => a.id).join('|')

  // 条目从"待批准"里消失（批准后转 running / 拒绝后收尾）就清掉本地标记，
  // 免得下一轮同一 callId 复用时按钮是灰的。
  //
  // 依赖刻意用 id 串：快照每次都是新数组，直接依赖数组会每次渲染都跑一遍。
  useEffect(() => {
    setInFlight((prev) => {
      const keep = new Set((approvals || []).map((a) => a.id))
      const ids = Object.keys(prev)
      if (ids.every((id) => keep.has(id))) return prev
      const next: Record<string, boolean> = {}
      for (const id of ids) if (keep.has(id)) next[id] = prev[id]
      return next
    })
  }, [approvalIds])

  if (!onDecide || !approvals || approvals.length === 0) return null

  const decide = (id: string, approved: boolean) => {
    setInFlight((prev) => (prev[id] ? prev : { ...prev, [id]: true }))
    onDecide(id, approved)
    // 万一回执丢了（链路失败 / 后端没等到这个 callId），8 秒后把按钮放开——
    // 绝不把用户锁在一个按不动的按钮上（审批本身还有超时兜底）。
    window.setTimeout(() => {
      setInFlight((prev) => {
        if (!prev[id]) return prev
        const next = { ...prev }
        delete next[id]
        return next
      })
    }, 8000)
  }

  // 绝对定位：贴在所在容器**之上**覆盖对话内容，且**不占布局空间**。
  // `bottom-full` 把整个面板抬到容器顶边以上；左右各留 3 与下方输入框对齐。
  // 所在的 Composer 容器是 `relative`，所以这里是相对它定位。
  return (
    <div
      data-testid="approval-floating-panel"
      role="alertdialog"
      aria-label={`审批请求：${approvals.length} 项等待批准`}
      className={`absolute bottom-full inset-x-3 mb-2 z-40 rounded-xl border border-amber-300/90 dark:border-amber-600/50 bg-white/95 dark:bg-[#1f1a14]/95 shadow-xl backdrop-blur-md overflow-hidden select-none animate-in fade-in slide-in-from-bottom-2 duration-150 ${
        className || ''
      }`}
    >
      {/* 标题行：审批是阻塞项，标题只说一件事——"等你批准" */}
      <div className="flex items-center justify-between px-2.5 py-1.5 bg-amber-500/10 border-b border-amber-300/70 dark:border-amber-600/30">
        <div className="flex items-center space-x-2 min-w-0">
          <ShieldAlert size={13} className="flex-shrink-0 text-amber-600 dark:text-amber-400" />
          <span className="text-xs font-semibold text-amber-900 dark:text-amber-200">
            审批请求
          </span>
          <span className="px-1.5 py-0.2 rounded-full text-[10px] font-medium bg-amber-200/80 dark:bg-amber-500/20 text-amber-800 dark:text-amber-300">
            {approvals.length} 项待批准
          </span>
        </div>
        <span className="hidden sm:inline text-[10px] text-amber-700/80 dark:text-amber-300/80">
          超时未处理将按安全方向自动拒绝
        </span>
      </div>

      {/* 待批准条目：通常一条（工具执行是顺序的） */}
      <div className="max-h-[38vh] overflow-y-auto divide-y divide-amber-200/60 dark:divide-amber-900/40">
        {approvals.map((approval) => {
          const submitted = Boolean(inFlight[approval.id])
          return (
            <div key={approval.id} className="p-2.5 space-y-2">
              <div className="flex items-center justify-between gap-3">
                <div className="flex items-center space-x-2 min-w-0">
                  <Shield
                    size={12}
                    className="flex-shrink-0 text-amber-600 dark:text-amber-400"
                  />
                  <span className="truncate text-xs font-medium text-amber-900 dark:text-amber-200">
                    这次调用会修改工作区，是否执行？
                  </span>
                  <span
                    className="flex-shrink-0 px-1.5 py-0.2 rounded bg-amber-100/80 dark:bg-amber-950/60 border border-amber-300/70 dark:border-amber-700/50 text-[10.5px] font-mono text-amber-800 dark:text-amber-300"
                    title={`工具：${approval.tool}`}
                  >
                    {TOOL_LABEL[approval.tool] || approval.tool}
                  </span>
                  <WaitedSince since={approval.at} />
                </div>

                <div className="flex items-center space-x-1.5 flex-shrink-0">
                  {submitted && (
                    <span className="text-[10.5px] text-amber-700/90 dark:text-amber-300/80">
                      已提交，等待智能体继续…
                    </span>
                  )}
                  <button
                    type="button"
                    data-testid="approval-deny"
                    disabled={submitted}
                    onClick={() => decide(approval.id, false)}
                    className="px-2.5 py-1 rounded-lg bg-zinc-200/90 dark:bg-zinc-800 hover:bg-zinc-300 dark:hover:bg-zinc-700 text-zinc-700 dark:text-zinc-300 text-xs transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                    title="拒绝这次调用（工具不会执行，智能体会收到拒绝回执）"
                  >
                    拒绝
                  </button>
                  <button
                    type="button"
                    data-testid="approval-approve"
                    disabled={submitted}
                    onClick={() => decide(approval.id, true)}
                    className="px-2.5 py-1 rounded-lg bg-blue-600 hover:bg-blue-500 text-white text-xs font-medium transition-colors shadow-xs cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                    title="批准这次调用（按上方内容执行）"
                  >
                    批准
                  </button>
                </div>
              </div>

              {/* 🔴 批准前必须能看清"批的是什么"：危险命令的原文 / 要动的目标。
                  宿主把参数原文随审批事件一起送来了（`ApprovalRequested.args`）。 */}
              {approval.preview ? (
                <pre className="m-0 px-2 py-1.5 rounded-lg bg-amber-950/10 dark:bg-black/40 border border-amber-500/20 text-[11px] font-mono whitespace-pre-wrap break-all text-amber-900 dark:text-amber-200 max-h-[28vh] overflow-y-auto">
                  {approval.preview}
                </pre>
              ) : (
                <div className="text-[10.5px] text-amber-700/90 dark:text-amber-300/80">
                  宿主没带上这次调用的参数，无法预览待批准内容——请谨慎批准。
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}
