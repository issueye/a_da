import { Code2, Compass, ListTodo, Sparkles } from 'lucide-react'
import type { AgentMode } from '../types'

/**
 * 协作模式的**元数据单一真源**。
 *
 * `short` 给滑块用（四段并排，标签必须短）；`label`/`desc` 给提示与文档用。
 * 两个字段放一处，是因为"滑块上写什么"与"提示里说什么"必须是同一份事实。
 */
export const MODE_OPTIONS: {
  value: AgentMode
  label: string
  short: string
  icon: any
  desc: string
}[] = [
  { value: 'code', label: 'Code 编码', short: '编码', icon: Code2, desc: '全能敏捷编码与工程构建（默认）' },
  { value: 'plan', label: 'Plan 规划', short: '规划', icon: Compass, desc: '只读架构分析与实施计划设计（防写）' },
  { value: 'create', label: 'Create 创造', short: '创造', icon: Sparkles, desc: '智能体自我进化与工具 / 技能 CRUD' },
  { value: 'pm', label: 'PM 项目管理', short: 'PM', icon: ListTodo, desc: '目标拆解与分派，经网关驱动 Coding Agent 落地' },
]

/**
 * 模式滑块（四段式，带滑动指示物）。
 *
 * # 为什么是滑块而不是下拉菜单
 *
 * 模式只有四个、且互斥——滑块把"当前在哪一档、还有哪几档"一次摊平，点一下就到位；
 * 下拉菜单要先展开再选，对"只在开头设一次"的东西是多余的一次交互。
 *
 * # 为什么有 `readOnly`
 *
 * 🔴 **模式只在新建会话时确定**：会话一旦开始（有消息），改模式就等于把同一段对话
 * 交给两个不同的系统提示词/工具集去解释，前后行为对不上，而账还落在一条会话里。
 * 所以已开始的会话里这个控件**只显示、不可点**，并在提示里说清为什么——
 * 直接隐藏会让人以为"这个功能没了"。
 */
export const ModeSlider: React.FC<{
  value: AgentMode
  onChange?: (mode: AgentMode) => void
  readOnly?: boolean
  className?: string
}> = ({ value, onChange, readOnly = false, className = '' }) => {
  const index = Math.max(
    0,
    MODE_OPTIONS.findIndex((m) => m.value === value),
  )
  const current = MODE_OPTIONS[index]
  const step = 100 / MODE_OPTIONS.length

  const title = readOnly
    ? `本会话模式：${current.label}（在新建会话时确定，会话开始后不可切换）`
    : `当前：${current.label}。点击选择；${current.desc}`

  return (
    <div
      className={`relative flex p-0.5 rounded-xl border select-none ${
        readOnly
          ? 'bg-zinc-100/70 dark:bg-zinc-800/50 border-zinc-200/60 dark:border-zinc-700/40'
          : 'bg-zinc-100 dark:bg-zinc-800/80 border-zinc-200/80 dark:border-zinc-700/60 shadow-xs'
      } ${className}`}
      title={title}
      aria-label={title}
      role="radiogroup"
    >
      {/* 滑动指示物：位置由索引算出，过渡交给 CSS——不重排、不闪 */}
      <div
        className={`absolute top-0.5 bottom-0.5 left-0.5 rounded-lg transition-all duration-200 ease-out ${
          readOnly ? 'bg-zinc-300/70 dark:bg-zinc-700/70' : 'bg-blue-600 text-white shadow-blue-600/20'
        }`}
        style={{ width: `calc(${step}% - 2px)`, transform: `translateX(${index * 100}%)` }}
      />
      {MODE_OPTIONS.map((m) => {
        const Icon = m.icon
        const selected = m.value === value
        return (
          <button
            key={m.value}
            type="button"
            role="radio"
            aria-checked={selected}
            disabled={readOnly}
            onClick={() => !readOnly && onChange?.(m.value)}
            className={`relative z-10 flex-1 flex items-center justify-center space-x-1 py-1.5 rounded-lg text-xs font-medium transition-colors ${
              readOnly ? 'cursor-default' : 'cursor-pointer'
            } ${
              selected
                ? readOnly
                  ? 'text-zinc-700 dark:text-zinc-200 font-semibold'
                  : 'text-white font-semibold'
                : 'text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200'
            }`}
          >
            <Icon size={13} className={selected && !readOnly ? 'text-white' : 'text-blue-500'} />
            <span>{m.short}</span>
          </button>
        )
      })}
    </div>
  )
}
