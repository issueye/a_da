import React, { useState } from 'react'
import { Check, ListChecks } from 'lucide-react'
import type { QuestionData, QuestionItemData } from '../types'
import type { QuestionSubAnswerInput } from '../utils/question-data'

export type { QuestionSubAnswerInput }

export type AskUserAnswerFn = (
  callId: string,
  choice?: string,
  text?: string,
  choices?: string[],
  answers?: QuestionSubAnswerInput[]
) => void

/**
 * 提问作答条（ask_user）——**支持多选**，也支持**一次问多题**。
 *
 * 三处渲染（工具卡里的作答条、独立提问卡、输入框上方的浮动提问面板）共用这一份实现，
 * 免得"多选/多题"只在其中一处生效（问答交互各写一遍是历史上踩过的坑）。
 *
 * 行为：
 * - **单题**：点一下即答（不牺牲一步操作）；题目声明或用户手动切多选时改为勾选后提交；
 * - **多题**：逐题作答（单选点一下换选项、多选勾选），底部**一次提交**，逐题答复一起回传；
 * - **每个选项独占一行**（不做横向平铺）：说明里通常写着"代价/风险"，一行一条才读得清，
 *   平铺时说明一折行、旁边还挤着别的选项，反而看不出在选哪一条。
 */
export const AskUserPrompt: React.FC<{
  question: QuestionData
  onAnswer: AskUserAnswerFn
  tone?: 'blue' | 'amber'
}> = ({ question, onAnswer, tone = 'blue' }) => {
  // 归一化之后 `questions` 恒非空；这里再兜一层，组件单独用时也能工作
  const items: QuestionItemData[] =
    Array.isArray(question.questions) && question.questions.length > 0
      ? question.questions
      : [
          {
            id: 'q1',
            question: question.question,
            options: question.options || [],
            isMultiSelect: Boolean(question.isMultiSelect),
            allowText: true,
          },
        ]
  const multiQuestion = items.length > 1

  // ── 单题路径的状态（保持"点一下即答"的老交互）──
  const [multi, setMulti] = useState<boolean>(Boolean(items[0]?.isMultiSelect))
  const [selected, setSelected] = useState<string[]>([])
  const [text, setText] = useState('')

  // ── 多题路径的状态：每题一份草稿，最后一次性提交 ──
  const [draft, setDraft] = useState<Record<string, { selected: string[]; text: string }>>({})
  const draftOf = (id: string) => draft[id] || { selected: [], text: '' }
  const patchDraft = (id: string, patch: Partial<{ selected: string[]; text: string }>) =>
    setDraft((prev) => ({ ...prev, [id]: { ...(prev[id] || { selected: [], text: '' }), ...patch } }))

  const toggleIn = (list: string[], value: string, isMulti: boolean) =>
    isMulti
      ? list.includes(value)
        ? list.filter((v) => v !== value)
        : [...list, value]
      : list.includes(value)
        ? []
        : [value]

  const trimmedSingle = text.trim()

  const submitSingle = () => {
    if (!multi) {
      // 单选：没勾选项时必须有文本（点选项走的是即时答复，不经过这里）
      if (!trimmedSingle) return
      onAnswer(question.callId, undefined, trimmedSingle)
    } else {
      if (selected.length === 0 && !trimmedSingle) return
      onAnswer(question.callId, undefined, trimmedSingle || undefined, selected)
    }
    setSelected([])
    setText('')
  }

  const canSubmitSingle = multi
    ? selected.length > 0 || Boolean(trimmedSingle)
    : Boolean(trimmedSingle)

  const submitMulti = () => {
    const answers: QuestionSubAnswerInput[] = items.map((it) => {
      const a = draftOf(it.id || '')
      const t = a.text.trim()
      return {
        id: it.id,
        choice: !it.isMultiSelect && a.selected.length === 1 ? a.selected[0] : undefined,
        choices: it.isMultiSelect && a.selected.length > 0 ? a.selected : undefined,
        text: t || undefined,
      }
    })
    onAnswer(question.callId, undefined, undefined, undefined, answers)
    setDraft({})
  }

  const answeredCount = multiQuestion
    ? items.filter((it) => {
        const a = draftOf(it.id || '')
        return a.selected.length > 0 || a.text.trim()
      }).length
    : 0
  // 至少答一题即可提交：逼用户把每一题都填满会把人卡在面板上，
  // 没答的那些会在回执里如实标成"未作答"（模型看得到）。
  const canSubmitMulti = answeredCount > 0

  // 选项行是**两行块**（id+文案 / 说明），多选开关仍是单行小 chip——
  // 两者的对齐方式不同，所以把"外观"与"布局"拆开写，避免同一个 class 里
  // `items-start` 与 `items-center` 打架（Tailwind 的胜负看规则顺序，不看书写顺序）。
  const chipLook =
    'px-2.5 py-1.5 rounded-lg text-xs font-medium transition-colors shadow-xs border text-left'
  // 每个选项**独占一行**：`w-full` + 纵向排列（见外层 `flex flex-col`）
  const chipOption = `${chipLook} flex w-full items-start space-x-1.5`
  // `self-start`：纵向容器默认 `align-items: stretch`，不顶上就会被拉成整行宽
  const chipToggle = `${chipLook} flex items-center space-x-1 self-start`
  const chipIdle =
    tone === 'amber'
      ? 'bg-white dark:bg-amber-900/40 hover:bg-amber-100 dark:hover:bg-amber-800/50 border-amber-300/80 dark:border-amber-700/50 text-amber-900 dark:text-amber-100'
      : 'bg-white dark:bg-blue-900/40 hover:bg-blue-50 dark:hover:bg-blue-800/50 border-blue-200 dark:border-blue-700/50 text-blue-700 dark:text-blue-200'
  const chipActive =
    tone === 'amber'
      ? 'bg-amber-500 hover:bg-amber-400 border-amber-500 text-white'
      : 'bg-blue-600 hover:bg-blue-500 border-blue-600 text-white'

  const inputClass = `flex-1 bg-white dark:bg-black/30 border rounded-lg px-2.5 py-1 text-zinc-900 dark:text-zinc-100 text-xs outline-none focus:border-blue-500 shadow-xs ${
    tone === 'amber'
      ? 'border-amber-200 dark:border-amber-800/60'
      : 'border-blue-200 dark:border-blue-900/50'
  }`

  /** 一个选项按钮（行）。`active` 表示已选中。 */
  const renderOption = (opt: any, active: boolean, onClick: () => void, withCheckbox: boolean) => (
    <button
      key={opt.value}
      type="button"
      title={opt.description || opt.label}
      onClick={onClick}
      className={`${chipOption} ${active ? chipActive : chipIdle}`}
    >
      {withCheckbox && (
        <span
          className={`mt-[2px] w-3 h-3 shrink-0 rounded-[4px] border flex items-center justify-center ${
            active ? 'border-white/80 bg-white/20' : 'border-current opacity-50'
          }`}
        >
          {active && <Check size={9} />}
        </span>
      )}
      {/* 选项 id（如 `c1`）必须显示出来：题干/参数里就是用它编号的，
          用户要能一眼对上"我在选哪一条"，也能照着 id 在输入框里回话。 */}
      {opt.id && (
        <span
          className={`shrink-0 font-mono text-[10px] leading-none px-1 py-0.5 rounded ${
            active ? 'bg-white/20 text-white' : 'bg-black/10 dark:bg-white/15 opacity-80'
          }`}
        >
          {opt.id}
        </span>
      )}
      {/* 文案一行、说明折在下面：说明里通常写着"代价/风险"，是选哪条的关键依据，
          藏在 hover tooltip 里等于没给。 */}
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="break-words">{opt.label}</span>
        {opt.description && (
          <span
            className={`mt-0.5 text-[10px] font-normal leading-snug whitespace-normal break-words ${
              active ? 'text-white/85' : 'opacity-70'
            }`}
          >
            {opt.description}
          </span>
        )}
      </span>
    </button>
  )

  // ── 多题：逐题作答、一次提交 ──
  if (multiQuestion) {
    return (
      <div className="space-y-3">
        {items.map((it, idx) => {
          const key = it.id || `q${idx + 1}`
          const a = draftOf(key)
          const isMulti = Boolean(it.isMultiSelect)
          return (
            <div key={key} className="space-y-1.5">
              <div className="flex items-start space-x-1.5">
                {it.id && (
                  <span className="mt-[1px] shrink-0 font-mono text-[10px] leading-none px-1 py-0.5 rounded bg-black/10 dark:bg-white/15 opacity-80">
                    {it.id}
                  </span>
                )}
                <span className="text-xs font-medium text-zinc-800 dark:text-zinc-200 leading-snug">
                  {idx + 1}. {it.question}
                </span>
              </div>

              {(it.options || []).length > 0 && (
                <div className="flex flex-col gap-1.5">
                  {(it.options || []).map((opt: any) =>
                    renderOption(
                      opt,
                      a.selected.includes(opt.value),
                      () => patchDraft(key, { selected: toggleIn(a.selected, opt.value, isMulti) }),
                      isMulti
                    )
                  )}
                </div>
              )}

              {it.allowText !== false && (
                <input
                  type="text"
                  value={a.text}
                  onChange={(e) => patchDraft(key, { text: e.target.value })}
                  placeholder={isMulti ? '该题补充说明（可与勾选项一起提交）...' : '该题的自定义答复或说明...'}
                  className={inputClass}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') submitMulti()
                  }}
                />
              )}
            </div>
          )
        })}

        <div className="flex items-center space-x-1.5">
          <button
            type="button"
            onClick={submitMulti}
            disabled={!canSubmitMulti}
            className={`px-3 py-1 rounded-lg text-xs font-medium transition-colors shadow-xs ${
              canSubmitMulti
                ? 'bg-blue-600 hover:bg-blue-500 text-white'
                : 'bg-zinc-200 dark:bg-zinc-800 text-zinc-400 dark:text-zinc-500 cursor-not-allowed'
            }`}
          >
            {answeredCount > 0 ? `提交（已答 ${answeredCount}/${items.length}）` : '提交'}
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

  // ── 单题：保持"点一下即答" ──
  const options = items[0]?.options || []
  const hasOptions = options.length > 0

  return (
    <div className="space-y-2">
      {hasOptions && (
        <div className="flex flex-col gap-1.5">
          {options.map((opt: any) => {
            const active = multi && selected.includes(opt.value)
            return renderOption(
              opt,
              active,
              () => (multi ? setSelected((prev) => toggleIn(prev, opt.value, true)) : onAnswer(question.callId, opt.value)),
              multi
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
            className={`${chipToggle} ${
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
          className={inputClass}
          onKeyDown={(e) => {
            if (e.key === 'Enter') submitSingle()
          }}
        />
        <button
          type="button"
          onClick={submitSingle}
          disabled={!canSubmitSingle}
          className={`px-3 py-1 rounded-lg text-xs font-medium transition-colors shadow-xs ${
            canSubmitSingle
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
