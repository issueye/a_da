/**
 * 提问浮窗（AskUserFloatingPanel）
 *
 * 🔴 提问原先有两个毛病，和审批修之前是同一类：
 * 1. **在对话流里**：会话一长就被推到视口外，用户看不到"正在等你回答"，
 *    只能干等超时（超时按安全方向自动拒绝/跳过）；对话流里那两张卡还各自摆一套按钮，
 *    用户会怀疑"点哪个才算数"。
 * 2. **占布局**：输入框上方那块面板是**在流里的**，一出现就把输入框往下顶，
 *    题多时能长到半屏——每答一次内容都在跳。
 *
 * 现在与审批浮窗（`ApprovalFloatingPanel`）同一套形态：绝对定位**覆盖在内容之上**，
 * **不占布局空间**，出现/消失都不推挤输入框与对话内容；多题时一次只显示一题，
 * 用「上一题 / 下一题」翻页（见 `AskUserPrompt`），面板高度与题数无关。
 *
 * 对话流里的提问卡退化成**只读记录**（`AskUserQuestionRecord`）——按钮只在这一处。
 *
 * 🔴 定位由 `Composer` 里的**浮层栈**统一负责（UI-02），本组件只画卡片。
 * 以前它与审批浮窗各自 `absolute bottom-full inset-x-3`、只靠 z 值（30 vs 40）分先后：
 * 引擎通常一次只阻塞一件事，但旧卡未清理/跨轮次残留时两者会并存，那一刻**提问被审批盖住**。
 * 现在同列纵向排列，两个都在。
 */

import React, { useEffect, useState } from 'react'
import { Sparkles } from 'lucide-react'
import type { QuestionData } from '../types'
import { AskUserPrompt, type AskUserAnswerFn } from './AskUserPrompt'

export interface AskUserFloatingPanelProps {
  /** 当前会话正在等待回答的提问（`null` = 没有，面板不渲染） */
  question?: QuestionData | null
  /** 作答回执入口（对应 `question.answer`） */
  onAnswer?: AskUserAnswerFn
  className?: string
}

export const AskUserFloatingPanel: React.FC<AskUserFloatingPanelProps> = ({
  question,
  onAnswer,
  className,
}) => {
  /// 已提交、但快照还没回来的状态（提交 → 引擎继续 → 下一帧快照实测要几秒）。
  ///
  /// 没有它，用户点完看不到反应会连点——"点了没反应"正是这个仓库反复修过的那类体感
  /// bug（审批浮窗有同样的一条）。提问比审批更容易踩：点选项是"即答"，
  /// 连点两下就会对同一个 callId 发两次作答。
  const [submitted, setSubmitted] = useState(false)
  const callId = question?.callId || ''

  // 换了一条提问（callId 变了）就清掉本地标记，否则下一条提问一上来就是灰的。
  useEffect(() => {
    setSubmitted(false)
  }, [callId])

  if (!onAnswer || !question) return null

  const items = question.questions?.length ?? 1

  const answer: AskUserAnswerFn = (...args) => {
    setSubmitted(true)
    onAnswer(...args)
    // 万一回执丢了（链路失败 / 后端没等到这个 callId），8 秒后把面板放开——
    // 绝不把用户锁在一个点不动的界面上（提问本身还有超时兜底）。
    window.setTimeout(() => setSubmitted(false), 8000)
  }

  return (
    <div
      data-testid="ask-user-floating-panel"
      role="dialog"
      aria-label={items > 1 ? `智能体提问：${items} 个问题待回答` : '智能体提问'}
      className={`pointer-events-auto rounded-xl border border-amber-300/90 dark:border-amber-600/50 bg-white/95 dark:bg-[#1f1a14]/95 shadow-xl backdrop-blur-md overflow-hidden select-none animate-in fade-in slide-in-from-bottom-2 duration-150 ${
        className || ''
      }`}
    >
      {/* 标题行：提问是阻塞项，标题只说一件事——"等你回答" */}
      <div className="flex items-center justify-between px-2.5 py-1.5 bg-amber-500/10 border-b border-amber-300/70 dark:border-amber-600/30">
        <div className="flex items-center space-x-2 min-w-0">
          <Sparkles size={13} className="flex-shrink-0 text-amber-600 dark:text-amber-400" />
          <span className="text-xs font-semibold text-amber-900 dark:text-amber-200">
            智能体提问
          </span>
          <span className="px-1.5 py-0.2 rounded-full text-[10px] font-medium bg-amber-200/80 dark:bg-amber-500/20 text-amber-800 dark:text-amber-300">
            {items > 1 ? `${items} 个问题` : '待你回答'}
          </span>
        </div>
        <span className="hidden sm:inline text-[10px] text-amber-700/80 dark:text-amber-300/80">
          超时未回答会按安全方向自动跳过
        </span>
      </div>

      {submitted ? (
        <div className="px-2.5 py-3 text-xs text-amber-800 dark:text-amber-200">
          已提交，等待智能体继续…
        </div>
      ) : (
        // 高度上限：浮窗再长也不该盖住整屏对话（多题已是一题一屏，这里只兜单题的极端选项数）
        <div className="max-h-[45vh] overflow-y-auto p-2.5">
          <AskUserPrompt question={question} onAnswer={answer} tone="amber" />
        </div>
      )}
    </div>
  )
}
