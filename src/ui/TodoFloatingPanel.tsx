/**
 * 任务规划步骤独立收缩悬浮框组件 (TodoFloatingPanel)
 *
 * 悬浮在会话区右上角，不在主消息流中穿插，保持会话区整洁。
 * 具备双态交互：
 * - 展开态：清晰展示每个步骤状态（已完成、进行中、待处理）、进度徽章、复制清单、一键收起；
 * - 收缩态：极小占用空间的悬浮胶囊药丸，展示当前任务进度与正在执行的步骤摘要，点击即展开。
 */

import React, { useState } from 'react'
import type { AgentStore } from '../agent/store'
import type { Item } from '../agent/types'
import { copyToClipboard } from '../platform/clipboard'
import { C, FONT_MONO } from '../theme'
import { Icon } from './controls'

export interface TodoStep {
  id?: string
  title: string
  status: 'pending' | 'in_progress' | 'completed'
}

/** 从工具卡片中解析待办步骤 */
export function parseTodosFromItem(item: Extract<Item, { kind: 'tool' }>): TodoStep[] | null {
  if (item.name !== 'todo') return null
  if (Array.isArray(item.args?.todos)) {
    return item.args.todos as TodoStep[]
  }
  if (item.output) {
    try {
      const parsed = JSON.parse(item.output)
      if (Array.isArray(parsed?.todos)) return parsed.todos
    } catch {}
  }
  return null
}

/** 获取会话列表中最新的一份待办规划 */
export function getLatestTodoItem(items: Item[]): {
  item: Extract<Item, { kind: 'tool' }>
  todos: TodoStep[]
  notes?: string
} | null {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i]
    if (it.kind === 'tool' && it.name === 'todo') {
      const todos = parseTodosFromItem(it)
      if (todos && todos.length > 0) {
        let notes: string | undefined
        if (typeof it.args?.notes === 'string') {
          notes = it.args.notes
        } else if (it.output) {
          try {
            const parsed = JSON.parse(it.output)
            if (typeof parsed?.notes === 'string') notes = parsed.notes
          } catch {}
        }
        return { item: it, todos, notes }
      }
    }
  }
  return null
}

export function TodoFloatingPanel({ store }: { store: AgentStore }) {
  const [collapsed, setCollapsed] = useState(false)
  const [copied, setCopied] = useState(false)

  const latest = getLatestTodoItem(store.active.items)
  if (!latest) return null

  const { todos, notes } = latest
  const completedCount = todos.filter((t) => t.status === 'completed').length
  const allDone = completedCount === todos.length && todos.length > 0
  const activeStep =
    todos.find((t) => t.status === 'in_progress') ?? todos.find((t) => t.status !== 'completed')

  const handleCopy = (e: any) => {
    e?.stopPropagation?.()
    const markdown = [
      `### 任务规划步骤 (${completedCount}/${todos.length})`,
      ...(notes ? [`> 备注：${notes}`] : []),
      ...todos.map((t) => `- [${t.status === 'completed' ? 'x' : ' '}] ${t.title}`),
    ].join('\n')

    void copyToClipboard(markdown).then((ok) => {
      if (ok) {
        setCopied(true)
        setTimeout(() => setCopied(false), 1500)
      }
    })
  }

  // 1. 收缩状态：悬浮胶囊药丸 (Pill)
  if (collapsed) {
    return (
      <div
        testId="todo-floating-pill"
        role="button"
        aria-label="展开任务规划"
        onClick={() => setCollapsed(false)}
        style={{
          position: 'absolute',
          top: 12,
          right: 28,
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 8,
          height: 32,
          paddingLeft: 10,
          paddingRight: 10,
          borderRadius: 16,
          cursor: 'pointer',
          backgroundColor: C.raised,
          borderWidth: 1,
          borderColor: C.borderStrong,
          boxShadow: {
            offsetX: 0,
            offsetY: 2,
            blurRadius: 10,
            spreadRadius: 0,
            color: C.shadow,
          },
          hover: {
            backgroundColor: C.chip,
          },
        }}
      >
        <Icon name="listTodo" size={13} color={allDone ? C.success : C.link} />
        <text style={{ fontSize: 12, fontWeight: 600, color: C.text }}>任务规划</text>
        <div
          onClick={() => setCollapsed(false)}
          style={{
            paddingLeft: 6,
            paddingRight: 6,
            height: 18,
            borderRadius: 9,
            backgroundColor: allDone ? C.chipHover : C.overlay,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          <text
            style={{
              fontSize: 10.5,
              fontFamily: FONT_MONO,
              fontWeight: 600,
              color: allDone ? C.success : C.link,
            }}
          >
            {`${completedCount}/${todos.length}`}
          </text>
        </div>
        {activeStep && !allDone ? (
          <text
            style={{
              fontSize: 11,
              color: C.secondary,
              maxWidth: 160,
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis',
            }}
          >
            {activeStep.title}
          </text>
        ) : null}
        <Icon name="chevronDown" size={12} color={C.tertiary} />
      </div>
    )
  }

  // 2. 展开状态：悬浮卡片 (Floating Card)
  return (
    <div
      testId="todo-floating-card"
      style={{
        position: 'absolute',
        top: 12,
        right: 28,
        width: 380,
        maxWidth: 460,
        display: 'flex',
        flexDirection: 'column',
        backgroundColor: C.raised,
        borderWidth: 1,
        borderColor: C.borderStrong,
        borderRadius: 12,
        padding: 12,
        boxShadow: {
          offsetX: 0,
          offsetY: 4,
          blurRadius: 18,
          spreadRadius: 0,
          color: C.shadow,
        },
      }}
    >
      {/* 顶部标题行 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'space-between',
          paddingBottom: 8,
          borderBottomWidth: 1,
          borderColor: C.cardBorder,
        }}
      >
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 7 }}>
          <Icon name="listTodo" size={14} color={allDone ? C.success : C.link} />
          <text style={{ fontSize: 12.5, fontWeight: 600, color: C.text }}>任务规划步骤</text>
          <div
            style={{
              paddingLeft: 6,
              paddingRight: 6,
              height: 18,
              borderRadius: 9,
              backgroundColor: allDone ? C.chipHover : C.overlay,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
          >
            <text
              style={{
                fontSize: 10.5,
                fontFamily: FONT_MONO,
                color: allDone ? C.success : C.tertiary,
              }}
            >
              {`${completedCount} / ${todos.length} 已完成`}
            </text>
          </div>
        </div>

        {/* 顶部操作区：复制与收起 */}
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <div
            role="button"
            aria-label={copied ? '已复制' : '复制清单'}
            onClick={handleCopy}
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 4,
              paddingLeft: 6,
              paddingRight: 6,
              height: 22,
              borderRadius: 4,
              cursor: 'pointer',
              backgroundColor: copied ? C.chipHover : C.overlay,
              hover: { backgroundColor: C.chipHover },
            }}
          >
            <Icon
              name={copied ? 'check' : 'copy'}
              size={11}
              color={copied ? C.success : C.tertiary}
            />
            <text style={{ fontSize: 11, color: copied ? C.success : C.tertiary }}>
              {copied ? '已复制' : '复制'}
            </text>
          </div>

          <div
            testId="collapse-todo-panel"
            role="button"
            aria-label="收起任务规划"
            onClick={() => setCollapsed(true)}
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 2,
              paddingLeft: 6,
              paddingRight: 6,
              height: 22,
              borderRadius: 4,
              cursor: 'pointer',
              backgroundColor: C.overlay,
              hover: { backgroundColor: C.chipHover },
            }}
          >
            <Icon name="chevronUp" size={11} color={C.tertiary} />
            <text style={{ fontSize: 11, color: C.tertiary }}>收起</text>
          </div>
        </div>
      </div>

      {/* 补充备注信息（如有） */}
      {notes ? (
        <div
          style={{
            marginTop: 6,
            paddingTop: 4,
            paddingBottom: 4,
            paddingLeft: 8,
            paddingRight: 8,
            backgroundColor: C.overlay,
            borderRadius: 6,
          }}
        >
          <text style={{ fontSize: 11, lineHeight: 16, color: C.secondary }}>
            {`备注：${notes}`}
          </text>
        </div>
      ) : null}

      {/* 任务步骤列表 */}
      <div
        testId="todo-steps-list"
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 4,
          marginTop: 8,
          maxHeight: 320,
          overflowY: 'scroll',
          paddingRight: 4,
        }}
      >
        {todos.map((step, idx) => {
          const isDone = step.status === 'completed'
          const isRunning = step.status === 'in_progress'

          return (
            <div
              key={step.id || idx}
              style={{
                display: 'flex',
                flexDirection: 'row',
                alignItems: 'center',
                gap: 8,
                paddingTop: 5,
                paddingBottom: 5,
                paddingLeft: 6,
                paddingRight: 6,
                borderRadius: 6,
                backgroundColor: isRunning ? C.overlay : undefined,
              }}
            >
              <Icon
                name={isDone ? 'circleCheck' : isRunning ? 'arrowRight' : 'circle'}
                size={13}
                color={isDone ? C.success : isRunning ? C.link : C.faint}
              />
              <text
                style={{
                  fontSize: 12,
                  lineHeight: 17,
                  color: isDone ? C.faint : isRunning ? C.text : C.secondary,
                  fontWeight: isRunning ? 600 : 400,
                  textDecoration: isDone ? 'line-through' : undefined,
                  flexShrink: 1,
                }}
              >
                {step.title}
              </text>
            </div>
          )
        })}
      </div>
    </div>
  )
}
