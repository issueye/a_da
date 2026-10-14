/**
 * 提问数据的归一化（纯函数，无 React / 无宿主依赖）。
 *
 * `ask_user` 有两条写法：**单题**（顶层 `question`/`choices`）与**多题**
 * （`questions: [{id, question, choices, multi_select}]`）。界面必须把两种都摊成
 * 同一个结构再渲染，否则"多题"只会在其中一处生效（问答交互各写一遍是历史上踩过的坑）。
 *
 * `Composer`（输入框上方的浮动提问面板）与 `Transcript`（对话流里的提问卡）共用这一份，
 * 所以它必须容忍模型给的任意形状：选项可能是字符串、也可能是 `{id,label,description}`，
 * 多选标记可能是 `multiSelect` / `multi_select` / `isMultiSelect`。
 */

import type { QuestionData, QuestionItemData, QuestionOption } from '../types'

/** 单题答复（多题路径按 `id` 对应回题目；缺省时后端按**顺序**对应）。 */
export interface QuestionSubAnswerInput {
  id?: string
  choice?: string
  choices?: string[]
  text?: string
}

function normOption(raw: any): QuestionOption {
  const id =
    typeof raw?.id === 'string' && raw.id.trim() ? raw.id.trim() : undefined
  const value =
    (typeof raw?.value === 'string' && raw.value) ||
    id ||
    (typeof raw?.label === 'string' && raw.label) ||
    String(raw)
  const label =
    (typeof raw?.label === 'string' && raw.label) ||
    (typeof raw?.title === 'string' && raw.title) ||
    value
  const description =
    typeof raw?.description === 'string' && raw.description ? raw.description : undefined
  return { id, value, label, description }
}

/** 选项列表：`options`（线协议驼峰）与 `choices`（工具事件）两种键都收。 */
function normOptions(raw: any): QuestionOption[] {
  const list = Array.isArray(raw?.options)
    ? raw.options
    : Array.isArray(raw?.choices)
      ? raw.choices
      : []
  return list.map((c: any) => (typeof c === 'string' ? { value: c, label: c } : normOption(c)))
}

function normOne(raw: any, fallbackId: string): QuestionItemData {
  const text = typeof raw?.question === 'string' ? raw.question.trim() : ''
  const id = typeof raw?.id === 'string' && raw.id.trim() ? raw.id.trim() : fallbackId
  const allowText = raw?.allowText ?? raw?.allow_text
  return {
    id,
    question: text || '请回复智能体的提问',
    options: normOptions(raw),
    isMultiSelect: Boolean(raw?.isMultiSelect ?? raw?.multiSelect ?? raw?.multi_select),
    // 默认允许文本补充（与工具侧 `allow_text` 默认 true 对齐）
    allowText: allowText === undefined ? true : Boolean(allowText),
  }
}

/**
 * 把一条提问（单题或多题）归一化成 `QuestionData`。
 *
 * 归一化之后 `questions` **恒非空**：单题也会被包成一项，渲染侧只需要认一种结构。
 * 顶层 `question`/`options`/`isMultiSelect` 填首题——不认识 `questions` 的旧渲染
 * 至少还能显示第一题。
 */
export function normalizeQuestionData(raw: any, callId: string): QuestionData {
  const rawList =
    Array.isArray(raw?.questions) && raw.questions.length > 0 ? raw.questions : [raw]
  const questions = rawList.map((r: any, i: number) => normOne(r, `q${i + 1}`))
  const first = questions[0]
  return {
    ...raw,
    callId,
    question: first?.question || '',
    options: first?.options || [],
    isMultiSelect: Boolean(first?.isMultiSelect),
    questions,
  }
}
