/**
 * 问答卡：智能体通过 `ask_user` 提问，用户在**这里**作答（不是输入框）。
 *
 * 刻意做成卡片内联交互而不是"把问题塞进输入框让用户打字"：
 * - 会话里同时可能有多个待答问题（多轮提问），内联才分得清在答哪个；
 * - 答案与那次调用绑定，历史里能看到"当时问了什么、答了什么"。
 *
 * 这个组件有**两个宿主**，所以单独放在这里（同一份实现，作答与那次调用的绑定
 * 就只有一个地方，不会两边漂移）：
 * - `Composer`：**待答**时浮动在输入框上方（与排队消息同一位置、同一套视觉），
 *   用户翻历史或虚拟列表把它挪出视野时也还看得见；
 * - `Transcript`：作答之后内联留在会话流里，作为"当时问了什么、答了什么"的记录。
 *
 * 两个宿主不会同时画同一张卡：待答只进浮动面板，答完/中止只进会话流。
 */

import React, { useState } from 'react'
import type { AgentQuestion, Item } from '../agent/types'
import { C, editorTheme } from '../theme'
import { Icon } from './controls'

/**
 * 从工具卡片的 details 里取出问答记录（`ask_user`）。
 *
 * 收到的是工具返回的 `details.question`；字段形状由 `AgentQuestion` 约定。
 * 这里做一次形状校验而不是 `as` 断言：details 也来自历史数据与工具返回值，
 * 畸形数据不该让整张卡片渲染崩掉。
 */
export function parseQuestion(item: Extract<Item, { kind: 'tool' }>): AgentQuestion | null {
  const raw = (item.details as { question?: unknown } | undefined)?.question
  if (!raw || typeof raw !== 'object') return null
  const record = raw as Record<string, unknown>
  if (typeof record.question !== 'string' || !record.question.trim()) return null
  const status = record.status
  if (status !== 'pending' && status !== 'answered' && status !== 'aborted') return null
  return {
    question: record.question,
    choices: Array.isArray(record.choices)
      ? (record.choices as AgentQuestion['choices'])
      : undefined,
    allowText: record.allowText !== false,
    status,
    askedAt: typeof record.askedAt === 'number' ? record.askedAt : item.at,
    answer: record.answer as AgentQuestion['answer'],
  }
}

export function QuestionCard({
  callId,
  question,
  onAnswer,
  variant = 'inline',
}: {
  /** 这次工具调用的 id：作答要按它回传给挂起中的工具。 */
  callId: string
  question: AgentQuestion
  /**
   * 把用户的作答交出去。
   *
   * 刻意**不收 store / client**：这张卡只需要"把答案送出去"这一件事，两个宿主
   * （会话流与输入框上方的浮动面板）各自决定怎么送——收整份客户端接口会把
   * 卡片和宿主绑在一起，也让测试不得不造一个 store。
   */
  onAnswer: (answer: { choice?: string; text?: string }) => void
  /**
   * `inline`＝会话流里的历史记录；`floating`＝输入框上方的浮动面板。
   *
   * 只差容器装饰：浮动面板自己已经是带边框的卡片，内层再套一圈边框、再留一圈
   * 外边距就成了"盒子里还有一个盒子"。内容一字不差——尤其是「等待你的回答」
   * 必须两处都在，它是用户判断"这个界面在等我"的最直接信号。
   */
  variant?: 'inline' | 'floating'
}) {
  const floating = variant === 'floating'
  const [text, setText] = useState('')
  const pending = question.status === 'pending'
  const choices = question.choices ?? []
  const allowText = question.allowText !== false
  // 有选项时也允许补充，但只有选项能单独提交；纯自由问答必须有内容
  const canSubmitText = allowText && text.trim().length > 0

  const answeredChoice = question.answer?.choice
    ? choices.find((choice) => choice.id === question.answer?.choice)
    : undefined

  return (
    <div
      testId={`question-${callId}`}
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 8,
        padding: floating ? 0 : 10,
        margin: floating ? 0 : 8,
        borderRadius: 8,
        backgroundColor: floating ? undefined : pending ? C.accentSoft : C.raised,
        borderWidth: floating ? 0 : 1,
        borderColor: floating ? undefined : pending ? C.accent : C.cardBorder,
      }}
    >
      <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
        <Icon name="sparkles" size={12} color={pending ? C.accent : C.faint} />
        <text style={{ fontSize: 10.5, lineHeight: 14, color: pending ? C.accent : C.faint }}>
          {pending ? '等待你的回答' : question.status === 'answered' ? '已回答' : '未回答'}
        </text>
      </div>

      <text
        style={{
          fontSize: 12.5,
          lineHeight: 18,
          color: C.text,
          whiteSpace: 'normal',
        }}
      >
        {question.question}
      </text>

      {pending ? (
        <>
          {choices.length > 0 ? (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
              {choices.map((choice) => (
                <div
                  key={choice.id}
                  testId={`question-choice-${choice.id}`}
                  role="button"
                  onClick={() => onAnswer({ choice: choice.id })}
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 2,
                    paddingTop: 7,
                    paddingBottom: 7,
                    paddingLeft: 10,
                    paddingRight: 10,
                    borderRadius: 7,
                    cursor: 'pointer',
                    backgroundColor: C.raised,
                    borderWidth: 1,
                    borderColor: C.borderStrong,
                    hover: { backgroundColor: C.chip },
                  }}
                >
                  <text style={{ fontSize: 12, lineHeight: 16, color: C.text }}>{choice.label}</text>
                  {choice.description ? (
                    <text style={{ fontSize: 11, lineHeight: 15, color: C.faint }}>
                      {choice.description}
                    </text>
                  ) : null}
                </div>
              ))}
            </div>
          ) : null}

          {allowText ? (
            <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
              <div
                style={{
                  display: 'flex',
                  flexGrow: 1,
                  minWidth: 0,
                  alignItems: 'center',
                  height: 30,
                  paddingLeft: 8,
                  paddingRight: 8,
                  borderRadius: 7,
                  backgroundColor: C.raised,
                  borderWidth: 1,
                  borderColor: C.borderStrong,
                }}
              >
                <input
                  testId={`question-input-${callId}`}
                  value={text}
                  placeholder={choices.length > 0 ? '或补充说明…' : '输入你的回答…'}
                  theme={editorTheme()}
                  style={{
                    flexGrow: 1,
                    minWidth: 0,
                    fontSize: 12,
                    color: C.text,
                    backgroundColor: '#00000000',
                    borderWidth: 0,
                  }}
                  onChange={(event) => setText(event.value ?? '')}
                  onSubmit={() => {
                    if (canSubmitText) onAnswer({ text: text.trim() })
                  }}
                />
              </div>
              <div
                testId={`question-submit-${callId}`}
                role="button"
                aria-label="提交回答"
                onClick={() => {
                  if (canSubmitText) onAnswer({ text: text.trim() })
                }}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  height: 30,
                  paddingLeft: 12,
                  paddingRight: 12,
                  borderRadius: 7,
                  cursor: 'pointer',
                  backgroundColor: canSubmitText ? C.inverse : C.chip,
                  hover: { opacity: 0.9 },
                }}
              >
                <text
                  style={{
                    fontSize: 12,
                    lineHeight: 16,
                    color: canSubmitText ? C.onInverse : C.faint,
                  }}
                >
                  提交
                </text>
              </div>
            </div>
          ) : null}
        </>
      ) : (
        // 已作答/已中止：把结果留在卡片上，历史里能看到"当时答了什么"
        <text style={{ fontSize: 12, lineHeight: 17, color: C.secondary, whiteSpace: 'normal' }}>
          {question.status === 'aborted'
            ? '运行已中止，未作答。'
            : answeredChoice
              ? `选择了「${answeredChoice.label}」${question.answer?.text ? `，并补充：${question.answer.text}` : ''}`
              : question.answer?.text
                ? `回答：${question.answer.text}`
                : '已提交。'}
        </text>
      )}
    </div>
  )
}
