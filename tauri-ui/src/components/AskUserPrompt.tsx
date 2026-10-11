import React, { useState } from 'react'
import { Check, ListChecks } from 'lucide-react'
import type { QuestionData } from '../types'

export type AskUserAnswerFn = (
  callId: string,
  choice?: string,
  text?: string,
  choices?: string[]
) => void

/**
 * 提问作答条（ask_user）——**支持多选**。
 *
 * 三处渲染（工具卡里的作答条、独立提问卡、输入框上方的浮动提问面板）共用这一份实现，
 * 免得"多选"只在其中一处生效（问答交互各写一遍是历史上踩过的坑）。
 *
 * 行为：
 * - 题目声明了 `multi_select`（或用户点「多选」切换）→ 选项变成**勾选**，`提交` 回传全部选中项；
 * - 否则保持原来的"点一下即答"（单选不牺牲一步操作）；
 * - 文本补充与选项可同时提交（多选时 `choices + text` 一起回传）。
 */
export const AskUserPrompt: React.FC<{
  question: QuestionData
  onAnswer: AskUserAnswerFn
  tone?: 'blue' | 'amber'
}> = ({ question, onAnswer, tone = 'blue' }) => {
  const declaredMulti = Boolean(
    (question as any).isMultiSelect ??
      (question as any).multiSelect ??
      (question as any).multi_select
  )
  const [multi, setMulti] = useState<boolean>(declaredMulti)
  const [selected, setSelected] = useState<string[]>([])
  const [text, setText] = useState('')

  const options = question.options || []
  const hasOptions = options.length > 0
  const trimmed = text.trim()

  const toggle = (value: string) =>
    setSelected((prev) => (prev.includes(value) ? prev.filter((v) => v !== value) : [...prev, value]))

  const submit = () => {
    if (multi) {
      if (selected.length === 0 && !trimmed) return
      onAnswer(question.callId, undefined, trimmed || undefined, selected)
    } else {
      if (!trimmed) return
      onAnswer(question.callId, undefined, trimmed)
    }
    setSelected([])
    setText('')
  }

  const canSubmit = multi ? selected.length > 0 || Boolean(trimmed) : Boolean(trimmed)

  const chipBase =
    'px-2.5 py-1 rounded-lg text-xs font-medium transition-colors shadow-xs border flex items-center space-x-1'
  const chipIdle =
    tone === 'amber'
      ? 'bg-white dark:bg-amber-900/40 hover:bg-amber-100 dark:hover:bg-amber-800/50 border-amber-300/80 dark:border-amber-700/50 text-amber-900 dark:text-amber-100'
      : 'bg-white dark:bg-blue-900/40 hover:bg-blue-50 dark:hover:bg-blue-800/50 border-blue-200 dark:border-blue-700/50 text-blue-700 dark:text-blue-200'
  const chipActive =
    tone === 'amber'
      ? 'bg-amber-500 hover:bg-amber-400 border-amber-500 text-white'
      : 'bg-blue-600 hover:bg-blue-500 border-blue-600 text-white'

  return (
    <div className="space-y-2">
      {hasOptions && (
        <div className="flex flex-wrap items-center gap-1.5">
          {options.map((opt: any) => {
            const active = multi && selected.includes(opt.value)
            return (
              <button
                key={opt.value}
                type="button"
                title={opt.description || opt.label}
                onClick={() => (multi ? toggle(opt.value) : onAnswer(question.callId, opt.value))}
                className={`${chipBase} ${active ? chipActive : chipIdle}`}
              >
                {multi && (
                  <span
                    className={`w-3 h-3 rounded-[4px] border flex items-center justify-center ${
                      active ? 'border-white/80 bg-white/20' : 'border-current opacity-50'
                    }`}
                  >
                    {active && <Check size={9} />}
                  </span>
                )}
                <span>{opt.label}</span>
              </button>
            )
          })}

          {/* 多选开关：题目没声明多选时，用户也能自己切成勾选模式 */}
          <button
            type="button"
            onClick={() => {
              setMulti((v) => !v)
              setSelected([])
            }}
            title={multi ? '切回单选（点一下即答）' : '切换为多选（可勾选多个选项后一起提交）'}
            className={`${chipBase} ${
              multi
                ? 'bg-zinc-800 dark:bg-zinc-700 text-white border-zinc-700 dark:border-zinc-600'
                : 'bg-transparent border-dashed border-zinc-300 dark:border-zinc-600 text-zinc-500 dark:text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200'
            }`}
          >
            <ListChecks size={11} />
            <span>{multi ? '多选中' : '多选'}</span>
          </button>
        </div>
      )}

      <div className="flex items-center space-x-1.5">
        <input
          type="text"
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={multi ? '补充说明（可与勾选项一起提交）...' : '输入自定义答复或说明...'}
          className={`flex-1 bg-white dark:bg-black/30 border rounded-lg px-2.5 py-1 text-zinc-900 dark:text-zinc-100 text-xs outline-none focus:border-blue-500 shadow-xs ${
            tone === 'amber'
              ? 'border-amber-200 dark:border-amber-800/60'
              : 'border-blue-200 dark:border-blue-900/50'
          }`}
          onKeyDown={(e) => {
            if (e.key === 'Enter') submit()
          }}
        />
        <button
          type="button"
          onClick={submit}
          disabled={!canSubmit}
          className={`px-3 py-1 rounded-lg text-xs font-medium transition-colors shadow-xs ${
            canSubmit
              ? 'bg-blue-600 hover:bg-blue-500 text-white'
              : 'bg-zinc-200 dark:bg-zinc-800 text-zinc-400 dark:text-zinc-500 cursor-not-allowed'
          }`}
        >
          {multi && selected.length > 0 ? `提交（已选 ${selected.length}）` : '提交'}
        </button>
        <button
          type="button"
          onClick={() => onAnswer(question.callId, undefined, '用户确认继续')}
          className="px-2.5 py-1 rounded-lg bg-zinc-200/70 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 hover:bg-zinc-300 dark:hover:bg-zinc-700 text-xs transition-colors"
        >
          跳过
        </button>
      </div>
    </div>
  )
}
