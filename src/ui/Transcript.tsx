/**
 * The conversation column.
 *
 * Rows are the same items the store appends during a turn: the user's tasks,
 * streamed model replies rendered as markdown, one card per tool call, and the
 * approval gate that holds a write until the user answers it.
 */

import React, { useEffect, useRef, useState } from 'react'
import { useGpuix, type PublicInstance } from '@gpuix/react'
import { describeTool } from '../agent/tools'
import { patchStats } from '../agent/patch'
import type { AgentStore } from '../agent/store'
import type { Item, ToolStatus } from '../agent/types'
import { Icon } from './controls'
import type { IconName } from '../icons'
import { C, docTheme, FONT_MONO, M } from '../theme'
import { Welcome } from './Welcome'
import { copyToClipboard } from '../platform/clipboard'
import { TodoFloatingPanel } from './TodoFloatingPanel'

const TOOL_LABEL: Record<string, string> = {
  list_files: '列出文件',
  read_file: '读取文件',
  search_files: '搜索代码',
  write_file: '写入文件',
  edit_file: '修改文件',
  run_command: '执行命令',
  todo: '任务规划',
}

const TOOL_ICON: Record<string, IconName> = {
  list_files: 'folder',
  read_file: 'file',
  search_files: 'search',
  write_file: 'file',
  edit_file: 'file',
  run_command: 'terminal',
  todo: 'listTodo',
}

/**
 * The status word and tint for a tool card.
 *
 * A function, not a constant: the colours have to be read at render time so a
 * theme switch repaints them. A module-level record would capture the palette
 * that was installed when this file was first imported.
 */
function statusOf(status: ToolStatus): { label: string; color: string } {
  switch (status) {
    case 'awaiting':
      return { label: '等待批准', color: C.accent }
    case 'running':
      return { label: '执行中', color: C.tertiary }
    // 跑完是常态，不再写一个「完成」占位置：行不再动就是结束了。
    case 'done':
      return { label: '', color: C.faint }
    case 'error':
      return { label: '失败', color: C.danger }
    case 'denied':
      return { label: '已拒绝', color: C.faint }
  }
}

/** 这些工具的摘要就是一条路径，可以拆成「文件名 + 目录」两段来排。 */
const PATH_TOOLS = new Set(['read_file', 'write_file', 'edit_file', 'list_files'])

/**
 * 一行的目标：正文是文件名，目录是暗色小字。
 *
 * 摘要本身来自 `describeTool`（唯一的事实来源，它也被调试日志用着），这里只做
 * 排版上的拆分。
 */
function toolTarget(name: string, args: Record<string, unknown>): { target: string; dir: string } {
  const summary = describeTool(name, args)
  if (!PATH_TOOLS.has(name)) return { target: summary, dir: '' }
  const cut = summary.lastIndexOf('/')
  if (cut < 0) return { target: summary, dir: '' }
  return { target: summary.slice(cut + 1), dir: summary.slice(0, cut + 1) }
}

/** Mono output that keeps its own newlines and can be opened in full. */
function MonoBlock({ text, tone }: { text: string; tone?: string }) {
  const [open, setOpen] = useState(false)
  const lines = text.split('\n')
  const limit = open ? lines.length : Math.min(lines.length, 14)
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 1 }}>
      {lines.slice(0, limit).map((line, index) => (
        <text
          key={index}
          style={{
            fontFamily: FONT_MONO,
            fontSize: 11.5,
            lineHeight: 17,
            color: tone ?? C.secondary,
            whiteSpace: 'nowrap',
            // 一行很长时给省略号，而不是被盒子边缘硬切成两半。
            textOverflow: 'ellipsis',
            minWidth: 0,
          }}
        >
          {line || ' '}
        </text>
      ))}
      {lines.length > limit ? (
        <div
          onClick={() => setOpen(true)}
          style={{ cursor: 'pointer', paddingTop: 4, width: '100%' }}
        >
          <text style={{ fontSize: 11.5, lineHeight: 17, color: C.link, whiteSpace: 'nowrap' }}>
            展开其余 {lines.length - limit} 行
          </text>
        </div>
      ) : null}
    </div>
  )
}

/** The user's own turn, right-aligned like a chat bubble. */
function UserRow({ item }: { item: Extract<Item, { kind: 'user' }> }) {
  const lines = item.text.split('\n')
  return (
    <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', width: '100%', gap: 4 }}>
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          maxWidth: 560,
          paddingTop: 8,
          paddingBottom: 8,
          paddingLeft: 11,
          paddingRight: 11,
          backgroundColor: C.user,
          borderWidth: 1,
          borderColor: C.cardBorder,
          borderRadius: 10,
        }}
      >
        {lines.map((line, index) => (
          <text key={index} style={{ fontSize: 13, lineHeight: 19, color: C.text }}>
            {line || ' '}
          </text>
        ))}
      </div>
      {item.queued ? (
        <text style={{ fontSize: 11, lineHeight: 15, color: C.faint }}>排队中</text>
      ) : null}
    </div>
  )
}

/** 格式化耗时 */
export function formatDuration(ms?: number): string {
  if (ms === undefined || ms <= 0) return ''
  if (ms < 1000) return `${(ms / 1000).toFixed(1)}s`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  const minutes = Math.floor(ms / 60_000)
  const seconds = Math.round((ms % 60_000) / 1000)
  return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`
}

/** 格式化数字千分位 */
export function formatNumber(n: number): string {
  return n.toLocaleString('en-US')
}

/** 格式化 Token 简写 */
export function formatTokenShort(n: number): string {
  if (n < 1000) return String(n)
  if (n < 10_000) return `${(n / 1000).toFixed(1)}k`
  return `${Math.round(n / 1000)}k`
}

function AssistantRow({ item }: { item: Extract<Item, { kind: 'assistant' }> }) {
  if (!item.text.trim() && !item.streaming) return null

  const hasStats = !item.streaming && (item.durationMs !== undefined || item.usage !== undefined)
  const durationText = formatDuration(item.durationMs)
  const usage = item.usage

  return (
    <div style={{ display: 'flex', flexDirection: 'column', width: '100%' }}>
      {item.text ? <markdown source={item.text} theme={docTheme()} /> : null}
      {item.streaming ? (
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 6,
            paddingTop: item.text ? 8 : 0,
          }}
        >
          <Icon name="dot" size={9} color={C.tertiary} />
          <text style={{ fontSize: 11.5, lineHeight: 16, color: C.tertiary }}>正在生成…</text>
        </div>
      ) : null}

      {hasStats ? (
        <div
          testId="assistant-meta-bar"
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            justifyContent: 'space-between',
            paddingTop: 6,
            marginTop: 4,
            borderTopWidth: 1,
            borderColor: C.cardBorder,
          }}
        >
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 12,
              flexWrap: 'wrap',
            }}
          >
            {/* 耗时微徽标 */}
            {durationText ? (
              <div
                testId="message-duration"
                aria-label={`本次对话花费时间：${durationText}`}
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 3.5,
                }}
              >
                <Icon name="clock" size={11} color={C.faint} />
                <text style={{ fontSize: 11, color: C.tertiary }}>{durationText}</text>
              </div>
            ) : null}

            {/* Token 统计徽标 */}
            {usage && usage.totalTokens > 0 ? (
              <div
                testId="message-tokens"
                aria-label={`总计: ${formatNumber(usage.totalTokens)} tokens (输入: ${formatNumber(usage.promptTokens)} · 输出: ${formatNumber(usage.completionTokens)}${usage.thinkingTokens ? ` · 思考: ${formatNumber(usage.thinkingTokens)}` : ''})`}
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 3.5,
                }}
              >
                <Icon name="sparkles" size={11} color={C.faint} />
                <text style={{ fontSize: 11, color: C.tertiary }}>
                  {`${formatNumber(usage.totalTokens)} tokens`}
                </text>
                <text style={{ fontSize: 10.5, color: C.faint, marginLeft: 1 }}>
                  {`(${formatNumber(usage.promptTokens)} ↑ / ${formatNumber(usage.completionTokens)} ↓)`}
                </text>
              </div>
            ) : null}
          </div>

          {/* 快捷复制回复 */}
          {item.text ? <CopyButton text={item.text} label="复制回复" /> : null}
        </div>
      ) : item.text && !item.streaming ? (
        <div style={{ display: 'flex', justifyContent: 'flex-end', paddingTop: 4 }}>
          <CopyButton text={item.text} label="复制回复" />
        </div>
      ) : null}
    </div>
  )
}

/** 小型快捷复制按钮 */
function CopyButton({ text, label = '复制' }: { text: string; label?: string }) {
  const [copied, setCopied] = useState(false)
  const handleCopy = (e: any) => {
    e?.stopPropagation?.()
    void copyToClipboard(text).then((ok) => {
      if (ok) {
        setCopied(true)
        setTimeout(() => setCopied(false), 1500)
      }
    })
  }
  return (
    <div
      role="button"
      aria-label={copied ? '已复制' : label}
      onClick={handleCopy}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        gap: 3,
        paddingLeft: 6,
        paddingRight: 6,
        height: 20,
        borderRadius: 4,
        cursor: 'pointer',
        backgroundColor: copied ? C.chipHover : C.overlay,
        hover: { backgroundColor: C.chipHover },
      }}
    >
      <Icon name={copied ? 'check' : 'copy'} size={10} color={copied ? C.success : C.tertiary} />
      <text style={{ fontSize: 10.5, lineHeight: 14, color: copied ? C.success : C.tertiary }}>
        {copied ? '已复制' : label}
      </text>
    </div>
  )
}

interface TodoStep {
  id?: string
  title: string
  status: 'pending' | 'in_progress' | 'completed'
}

function parseTodos(item: Extract<Item, { kind: 'tool' }>): TodoStep[] | null {
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

function TodoContent({ item }: { item: Extract<Item, { kind: 'tool' }> }) {
  const todos = parseTodos(item)
  if (!todos || !todos.length) {
    return item.output ? (
      <div style={{ padding: 10 }}>
        <MonoBlock text={item.output} />
      </div>
    ) : null
  }
  const completedCount = todos.filter((t) => t.status === 'completed').length

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 4,
        paddingTop: 8,
        paddingBottom: 8,
        paddingLeft: 12,
        paddingRight: 12,
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'space-between',
          paddingBottom: 6,
          borderBottomWidth: 1,
          borderColor: C.cardBorder,
        }}
      >
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <Icon name="listTodo" size={13} color={C.link} />
          <text style={{ fontSize: 12, fontWeight: 600, color: C.text }}>任务规划步骤</text>
        </div>
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <text style={{ fontFamily: FONT_MONO, fontSize: 11, color: C.tertiary }}>
            {`${completedCount} / ${todos.length} 已完成`}
          </text>
          {item.output ? <CopyButton text={item.output} label="复制清单" /> : null}
        </div>
      </div>
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
              paddingTop: 4,
              paddingBottom: 4,
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
  )
}

function ToolCard({ item, store }: { item: Extract<Item, { kind: 'tool' }>; store: AgentStore }) {
  const status = statusOf(item.status)
  /** 折叠 / 展开。默认一律收起：跑完的、失败的、被拒的都只占一行。 */
  const [open, setOpen] = useState(false)
  const stats = item.patch ? patchStats(item.patch) : null
  const isTodo = item.name === 'todo'
  const todos = isTodo ? parseTodos(item) : null

  let target = ''
  let dir = ''
  if (isTodo && todos) {
    const completedCount = todos.filter((t) => t.status === 'completed').length
    const activeStep = todos.find((t) => t.status === 'in_progress') ?? todos.find((t) => t.status !== 'completed')
    target = activeStep?.title ?? '待办列表'
    dir = `${completedCount}/${todos.length} 已完成`
  } else {
    const parsed = toolTarget(item.name, item.args)
    target = parsed.target
    dir = parsed.dir
  }

  const detail = Boolean(item.patch) || Boolean(item.output) || isTodo

  return (
    <div
      testId={`tool-${item.id}`}
      style={{ display: 'flex', flexDirection: 'column', width: '100%' }}
    >
      {/*
        折叠起来就是一行：三角、图标、工具名、目标、改动量、状态。展开的那块才
        进带边框的盒子——一整轮下来，几十次调用应该读成一列行，而不是一摞卡片。
      */}
      <div
        testId={`tool-head-${item.id}`}
        role="button"
        aria-label={TOOL_LABEL[item.name] ?? item.name}
        onClick={detail ? () => setOpen((value) => !value) : undefined}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 7,
          height: M.row,
          paddingLeft: 2,
          paddingRight: 8,
          borderRadius: 6,
          flexShrink: 0,
          cursor: detail ? 'pointer' : 'default',
          hover: detail ? { backgroundColor: C.overlay } : undefined,
        }}
      >
        <Icon
          name={open ? 'chevronDown' : 'chevronRight'}
          size={11}
          color={detail ? C.faint : '#00000000'}
        />
        <Icon name={TOOL_ICON[item.name] ?? 'terminal'} size={12} color={item.status === 'running' ? C.link : C.tertiary} />
        <text style={{ fontSize: 12, lineHeight: 16, color: item.status === 'running' ? C.link : C.secondary, fontWeight: item.status === 'running' ? 500 : 400, flexShrink: 0 }}>
          {TOOL_LABEL[item.name] ?? item.name}
        </text>
        <text
          style={{
            fontFamily: FONT_MONO,
            fontSize: 11.5,
            lineHeight: 16,
            color: C.text,
            whiteSpace: 'nowrap',
            textOverflow: 'ellipsis',
            // 弹性项默认按内容宽度撑着（min-width: auto），不给 0 就不会收缩，
            // 也就永远轮不到省略号出手——只会被父级硬裁掉。
            minWidth: 0,
            flexShrink: 1,
          }}
        >
          {target}
        </text>
        {dir ? (
          <text
            style={{
              fontFamily: FONT_MONO,
              fontSize: 11,
              lineHeight: 15,
              color: C.faint,
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis',
              minWidth: 0,
              flexShrink: 1,
            }}
          >
            {dir}
          </text>
        ) : null}
        <div style={{ flexGrow: 1 }} />
        {stats && stats.added > 0 ? (
          <div style={{ display: 'flex', alignItems: 'center', height: 18, paddingLeft: 5, paddingRight: 5, borderRadius: 4, backgroundColor: C.overlay, flexShrink: 0 }}>
            <text
              style={{
                fontSize: 11,
                lineHeight: 15,
                fontWeight: 500,
                color: C.success,
                whiteSpace: 'nowrap',
              }}
            >
              {`+${stats.added}`}
            </text>
          </div>
        ) : null}
        {stats && stats.removed > 0 ? (
          <div style={{ display: 'flex', alignItems: 'center', height: 18, paddingLeft: 5, paddingRight: 5, borderRadius: 4, backgroundColor: C.overlay, flexShrink: 0 }}>
            <text
              style={{
                fontSize: 11,
                lineHeight: 15,
                fontWeight: 500,
                color: C.danger,
                whiteSpace: 'nowrap',
              }}
            >
              {`−${stats.removed}`}
            </text>
          </div>
        ) : null}
        {status.label ? (
          <text
            style={{
              fontSize: 11,
              lineHeight: 15,
              fontWeight: item.status === 'running' ? 500 : 400,
              color: status.color,
              whiteSpace: 'nowrap',
              flexShrink: 0,
            }}
          >
            {status.label}
          </text>
        ) : null}
      </div>

      {/*
        盒子只在真有大开内容时才渲染。收起时也渲染的话，一个带边框、里面什么都没有
        的容器会塌成一条 1px 的线——看起来就像每行都带下划线。
      */}
      {open || item.status === 'awaiting' ? (
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            marginTop: 2,
            borderWidth: 1,
            borderColor: C.cardBorder,
            borderRadius: 10,
            backgroundColor: C.tool,
            overflow: 'hidden',
          }}
        >
          {item.status === 'awaiting' ? (
            <div
              style={{
                display: 'flex',
                flexDirection: 'row',
                alignItems: 'center',
                gap: 8,
                paddingTop: 9,
                paddingBottom: 9,
                paddingLeft: 10,
                paddingRight: 10,
                backgroundColor: C.accentSoft,
              }}
            >
              <text style={{ fontSize: 12, lineHeight: 17, color: C.accent, flexShrink: 1 }}>
                这次调用会修改工作区，是否执行？
              </text>
              <div style={{ flexGrow: 1 }} />
              <div
                testId="approve"
                role="button"
                aria-label="批准"
                onClick={() => store.decide(item.id, true)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  height: 24,
                  paddingLeft: 10,
                  paddingRight: 10,
                  borderRadius: 6,
                  cursor: 'pointer',
                  backgroundColor: C.inverse,
                  hover: { opacity: 0.9 },
                }}
              >
                <text style={{ fontSize: 12, lineHeight: 16, color: C.onInverse }}>批准</text>
              </div>
              <div
                testId="deny"
                role="button"
                aria-label="拒绝"
                onClick={() => store.decide(item.id, false)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  height: 24,
                  paddingLeft: 10,
                  paddingRight: 10,
                  borderRadius: 6,
                  cursor: 'pointer',
                  backgroundColor: C.raised,
                  borderWidth: 1,
                  borderColor: C.borderStrong,
                  hover: { backgroundColor: C.chip },
                }}
              >
                <text style={{ fontSize: 12, lineHeight: 16, color: C.secondary }}>拒绝</text>
              </div>
            </div>
          ) : null}

          {open && isTodo ? <TodoContent item={item} /> : null}

          {open && item.patch ? (
            <div style={{ display: 'flex', flexDirection: 'column' }}>
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  paddingTop: 6,
                  paddingBottom: 6,
                  paddingLeft: 10,
                  paddingRight: 10,
                  backgroundColor: C.raised,
                  borderBottomWidth: 1,
                  borderColor: C.cardBorder,
                }}
              >
                <text style={{ fontSize: 11, fontFamily: FONT_MONO, color: C.secondary }}>
                  {target}
                </text>
                <CopyButton text={item.patch} label="复制 Diff" />
              </div>
              <diff patch={item.patch} wordDiff maxLines={24} theme={docTheme()} />
            </div>
          ) : null}

          {open && item.name === 'run_command' && !isTodo ? (
            <div style={{ display: 'flex', flexDirection: 'column' }}>
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  paddingTop: 6,
                  paddingBottom: 6,
                  paddingLeft: 10,
                  paddingRight: 10,
                  backgroundColor: C.raised,
                  borderBottomWidth: 1,
                  borderColor: C.cardBorder,
                }}
              >
                <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6, minWidth: 0, flexShrink: 1 }}>
                  <Icon name="terminal" size={11} color={C.tertiary} />
                  <text
                    style={{
                      fontFamily: FONT_MONO,
                      fontSize: 11,
                      lineHeight: 15,
                      color: C.text,
                      whiteSpace: 'nowrap',
                      textOverflow: 'ellipsis',
                      minWidth: 0,
                    }}
                  >
                    {`$ ${String(item.args.command ?? '')}`}
                  </text>
                </div>
                <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6, flexShrink: 0 }}>
                  <CopyButton text={String(item.args.command ?? '')} label="复制命令" />
                  {item.output ? <CopyButton text={item.output} label="复制输出" /> : null}
                </div>
              </div>
              {item.output ? (
                <div style={{ padding: 10 }}>
                  <MonoBlock text={item.output} tone={item.status === 'error' ? C.danger : C.secondary} />
                </div>
              ) : null}
            </div>
          ) : null}

          {open && item.output && !item.patch && !isTodo && item.name !== 'run_command' ? (
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                gap: 6,
                paddingTop: 8,
                paddingBottom: 8,
                paddingLeft: 10,
                paddingRight: 10,
              }}
            >
              <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between' }}>
                <text style={{ fontSize: 11, color: C.tertiary }}>输出内容</text>
                <CopyButton text={item.output} label="复制" />
              </div>
              <MonoBlock text={item.output} tone={item.status === 'error' ? C.danger : C.secondary} />
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

/**
 * 模型的思考链，默认只占一行。
 *
 * 收起时是「思考 · 持续 N 秒」，并实时展现最新思考切片预览；
 * 展开则是左侧带导线的推理原文卡片。
 */
function ThinkingRow({ item }: { item: Extract<Item, { kind: 'thinking' }> }) {
  const [open, setOpen] = useState(false)
  const isStreaming = item.endedAt === undefined
  const seconds = isStreaming ? null : Math.max(1, Math.round((item.endedAt! - item.at) / 1000))

  // 提取思考输出的最新非空单行作为折叠态的动态预览
  const streamingPreview = (() => {
    if (!isStreaming) return null
    const lines = item.text.split('\n').map((l) => l.trim()).filter(Boolean)
    if (!lines.length) return null
    const last = lines[lines.length - 1]!
    return last.length > 48 ? `${last.slice(0, 48)}…` : last
  })()

  return (
    <div style={{ display: 'flex', flexDirection: 'column', width: '100%' }}>
      <div
        testId={`thinking-head-${item.id}`}
        role="button"
        aria-label="思考过程"
        onClick={() => setOpen((value) => !value)}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 7,
          height: M.row,
          paddingLeft: 2,
          paddingRight: 8,
          borderRadius: 6,
          flexShrink: 0,
          cursor: 'pointer',
          hover: { backgroundColor: C.overlay },
        }}
      >
        <Icon name={open ? 'chevronDown' : 'chevronRight'} size={11} color={C.faint} />
        <Icon name="brain" size={12} color={isStreaming ? C.link : C.tertiary} />
        <text style={{ fontSize: 12, lineHeight: 16, color: isStreaming ? C.link : C.secondary, fontWeight: isStreaming ? 600 : 400 }}>
          思考
        </text>
        <text style={{ fontSize: 11, lineHeight: 15, color: C.faint, flexShrink: 0 }}>
          {isStreaming ? '· 思考中…' : `· 持续 ${seconds} 秒`}
        </text>
        {!open && streamingPreview ? (
          <text
            style={{
              fontSize: 11,
              lineHeight: 15,
              color: C.tertiary,
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis',
              minWidth: 0,
              flexShrink: 1,
            }}
          >
            {`· "${streamingPreview}"`}
          </text>
        ) : null}
        <div style={{ flexGrow: 1 }} />
      </div>

      {open ? <ThinkingBody text={item.text} isStreaming={isStreaming} /> : null}
    </div>
  )
}

/** 推理原文：散文排版，左侧结构化导线缩进，带快捷复制。 */
function ThinkingBody({ text, isStreaming }: { text: string; isStreaming?: boolean }) {
  const [full, setFull] = useState(false)
  const lines = text.split('\n')
  const limit = full ? lines.length : Math.min(lines.length, 16)
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        marginLeft: 8,
        paddingLeft: 12,
        borderLeftWidth: 2,
        borderColor: isStreaming ? C.link : C.borderStrong,
        marginTop: 3,
        marginBottom: 6,
        width: '100%',
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 3,
          paddingTop: 8,
          paddingBottom: 8,
          paddingLeft: 12,
          paddingRight: 12,
          borderWidth: 1,
          borderColor: C.cardBorder,
          borderRadius: 8,
          backgroundColor: C.card,
        }}
      >
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            justifyContent: 'space-between',
            paddingBottom: 4,
            borderBottomWidth: 1,
            borderColor: C.overlay,
          }}
        >
          <text style={{ fontSize: 11, fontWeight: 500, color: C.tertiary }}>
            {isStreaming ? '正在推理…' : '推理分析'}
          </text>
          <CopyButton text={text} label="复制思考" />
        </div>
        {lines.slice(0, limit).map((line, index) => (
          <text key={index} style={{ fontSize: 12, lineHeight: 18, color: C.secondary }}>
            {line || ' '}
          </text>
        ))}
        {lines.length > limit ? (
          <div
            onClick={() => setFull(true)}
            style={{ cursor: 'pointer', paddingTop: 6, width: '100%' }}
          >
            <text style={{ fontSize: 11.5, lineHeight: 17, color: C.link }}>
              展开其余 {lines.length - limit} 行
            </text>
          </div>
        ) : null}
      </div>
    </div>
  )
}

function NoticeRow({ item }: { item: Extract<Item, { kind: 'notice' }> }) {
  const error = item.level === 'error'
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'flex-start',
        gap: 7,
        width: '100%',
        paddingTop: 8,
        paddingBottom: 8,
        paddingLeft: 10,
        paddingRight: 10,
        borderRadius: 8,
        backgroundColor: error ? C.accentSoft : C.card,
      }}
    >
      <Icon name={error ? 'x' : 'check'} size={12} color={error ? C.accent : C.tertiary} />
      <text
        style={{
          fontSize: 12,
          lineHeight: 18,
          color: error ? C.accent : C.secondary,
          flexShrink: 1,
        }}
      >
        {item.text}
      </text>
    </div>
  )
}

/**
 * 每个条目下面留多少空。
 *
 * 一行一条的时间线要读得紧凑：工具行和思考行几乎贴在一起，人说的话才留白——
 * 一屏能看下几十次调用，而不是被空隙吃掉一半。
 */
const GAP_BELOW: Record<Item['kind'], number> = {
  user: 18,
  assistant: 18,
  notice: 12,
  tool: 3,
  thinking: 3,
}

function ItemRow({ item, store }: { item: Item; store: AgentStore }) {
  if (item.kind === 'user') return <UserRow item={item} />
  if (item.kind === 'thinking') return <ThinkingRow item={item} />
  if (item.kind === 'assistant') return <AssistantRow item={item} />
  if (item.kind === 'tool') {
    if (item.name === 'todo') return null
    return <ToolCard item={item} store={store} />
  }
  return <NoticeRow item={item} />
}

export type ProcessItem = Item

export interface UserBlock {
  kind: 'user'
  id: string
  item: Extract<Item, { kind: 'user' }>
}

export interface AssistantBlock {
  kind: 'assistant'
  id: string
  item: Extract<Item, { kind: 'assistant' }>
}

export interface ThinkingBlock {
  kind: 'thinking'
  id: string
  item: Extract<Item, { kind: 'thinking' }>
}

export interface ProcessBlock {
  kind: 'process'
  id: string
  items: ProcessItem[]
  isCompleted: boolean
}

export type TranscriptBlock = UserBlock | AssistantBlock | ThinkingBlock | ProcessBlock

/**
 * 将会话消息线性序列整理为结构化的块：
 * 按用户交互回合（Turn）划分：
 * - 用户提问独立成块（UserBlock）；
 * - 回合最终的回复/报告独立成块平铺展示（AssistantBlock）；
 * - 如果回合仅有思考而无工具调用（问答场景），将思考独立为 ThinkingBlock 突出展示；
 * - 如果包含工具调用，将思考、工具调用与通知收纳到同一个过程块（ProcessBlock）中，
 *   在完成后默认折叠，且醒目显示思考耗时与工具统计。
 */
export function buildTranscriptBlocks(items: Item[], isRunning: boolean): TranscriptBlock[] {
  const blocks: TranscriptBlock[] = []

  // 按 user 将 items 切割为回合
  let currentTurnItems: Item[] = []
  const turns: Item[][] = []

  for (const item of items) {
    if (item.kind === 'user') {
      if (currentTurnItems.length > 0) {
        turns.push(currentTurnItems)
      }
      currentTurnItems = [item]
    } else {
      currentTurnItems.push(item)
    }
  }
  if (currentTurnItems.length > 0) {
    turns.push(currentTurnItems)
  }

  for (let turnIdx = 0; turnIdx < turns.length; turnIdx++) {
    const turn = turns[turnIdx]
    const isLastTurn = turnIdx === turns.length - 1

    let userItem: Extract<Item, { kind: 'user' }> | null = null
    const nonUserItems: Item[] = []

    for (const it of turn) {
      if (it.kind === 'user' && !userItem) {
        userItem = it
      } else {
        nonUserItems.push(it)
      }
    }

    if (userItem) {
      blocks.push({
        kind: 'user',
        id: userItem.id,
        item: userItem,
      })
    }

    // 寻找该回合的最终报告项（最后一个 assistant 项）
    let reportIndex = -1
    for (let i = nonUserItems.length - 1; i >= 0; i--) {
      if (nonUserItems[i].kind === 'assistant') {
        reportIndex = i
        break
      }
    }

    const reportItem =
      reportIndex >= 0 ? (nonUserItems[reportIndex] as Extract<Item, { kind: 'assistant' }>) : null
    const processItems =
      reportIndex >= 0
        ? nonUserItems.filter((_, idx) => idx !== reportIndex)
        : nonUserItems

    if (processItems.length > 0) {
      const hasActive = processItems.some((it) => {
        if (it.kind === 'tool') {
          return it.status === 'running' || it.status === 'awaiting'
        }
        if (it.kind === 'thinking') {
          return it.endedAt === undefined
        }
        return false
      })

      // 该回合是否已完成：
      // 1. 没有未完成的 tool 或 thinking；
      // 2. 并且满足以下之一：历史轮次必完成、会话已结束运行、或当前轮次已进入报告输出阶段。
      const isCompleted =
        !hasActive && (!isLastTurn || !isRunning || reportItem !== null)

      blocks.push({
        kind: 'process',
        id: `process-${processItems[0].id}`,
        items: processItems,
        isCompleted,
      })
    }

    if (reportItem) {
      blocks.push({
        kind: 'assistant',
        id: reportItem.id,
        item: reportItem,
      })
    }
  }

  return blocks
}

/**
 * 过程卡片：将中间的思考、工具调用与通知聚合收纳。
 *
 * 完成之后默认收缩，点击折叠条可展开查看具体的思考与工具详情；
 * 正在运行或等待审批时保持展开，便于用户实时查看进度并操作批准/拒绝。
 */
function ProcessGroupCard({
  block,
  store,
  isOpen,
  onToggle,
}: {
  block: ProcessBlock
  store: AgentStore
  isOpen: boolean
  onToggle: () => void
}) {
  const tools = block.items.filter((it): it is Extract<Item, { kind: 'tool' }> => it.kind === 'tool')
  const thinkings = block.items.filter((it): it is Extract<Item, { kind: 'thinking' }> => it.kind === 'thinking')

  // 统计修改代码行数
  let totalAdded = 0
  let totalRemoved = 0
  for (const t of tools) {
    if (t.patch) {
      const stats = patchStats(t.patch)
      totalAdded += stats.added
      totalRemoved += stats.removed
    }
  }

  // 统计思考总时长与流式状态
  let totalThinkingSeconds = 0
  let isThinkingStreaming = false
  for (const th of thinkings) {
    if (th.endedAt !== undefined) {
      totalThinkingSeconds += Math.max(1, Math.round((th.endedAt - th.at) / 1000))
    } else {
      isThinkingStreaming = true
    }
  }

  // 概括步骤组成
  const summaryParts: string[] = []
  if (tools.length > 0) {
    summaryParts.push(`${tools.length} 项工具操作`)
  }
  const summaryText = summaryParts.join(' · ')

  // 状态显示
  const hasAwaiting = tools.some((t) => t.status === 'awaiting')
  const hasDenied = tools.some((t) => t.status === 'denied')
  const hasError =
    tools.some((t) => t.status === 'error') ||
    block.items.some((it) => it.kind === 'notice' && it.level === 'error')
  const isRunning = !block.isCompleted

  const title = '执行过程'

  return (
    <div
      testId={`process-group-${block.id}`}
      style={{ display: 'flex', flexDirection: 'column', width: '100%' }}
    >
      {/* 过程汇总折叠条 */}
      <div
        testId={`process-head-${block.id}`}
        role="button"
        aria-label={title}
        onClick={onToggle}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 7,
          height: 32,
          paddingLeft: 8,
          paddingRight: 10,
          borderRadius: 8,
          cursor: 'pointer',
          backgroundColor: C.raised,
          borderWidth: 1,
          borderColor: hasAwaiting ? C.accent : C.cardBorder,
          hover: { backgroundColor: C.overlay },
        }}
      >
        <Icon
          name={isOpen ? 'chevronDown' : 'chevronRight'}
          size={11}
          color={C.faint}
        />
        <Icon
          name={isRunning ? 'sparkles' : thinkings.length > 0 ? 'brain' : hasError ? 'x' : 'check'}
          size={12}
          color={hasAwaiting ? C.accent : isRunning ? C.link : hasError ? C.danger : C.tertiary}
        />
        <text
          style={{
            fontSize: 12,
            lineHeight: 16,
            fontWeight: 500,
            color: hasAwaiting ? C.accent : isRunning ? C.link : C.text,
            flexShrink: 0,
          }}
        >
          {title}
        </text>

        {/* 步骤计数徽章 */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            height: 18,
            paddingLeft: 6,
            paddingRight: 6,
            borderRadius: 4,
            backgroundColor: C.overlay,
            flexShrink: 0,
          }}
        >
          <text style={{ fontSize: 10.5, lineHeight: 14, color: C.secondary, whiteSpace: 'nowrap' }}>
            {`${block.items.length} 个步骤`}
          </text>
        </div>

        {/* 醒目的思考耗时徽章 */}
        {thinkings.length > 0 ? (
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 4,
              height: 18,
              paddingLeft: 6,
              paddingRight: 6,
              borderRadius: 4,
              backgroundColor: C.overlay,
              flexShrink: 0,
            }}
          >
            <Icon name="brain" size={10} color={isThinkingStreaming ? C.link : C.tertiary} />
            <text
              style={{
                fontSize: 10.5,
                lineHeight: 14,
                color: isThinkingStreaming ? C.link : C.secondary,
                whiteSpace: 'nowrap',
              }}
            >
              {isThinkingStreaming ? '推理中…' : `${totalThinkingSeconds}s`}
            </text>
          </div>
        ) : null}

        {/* 步骤成分摘要 */}
        {summaryText ? (
          <text
            style={{
              fontSize: 11,
              lineHeight: 15,
              color: C.faint,
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis',
              minWidth: 0,
              flexShrink: 1,
            }}
          >
            {summaryText}
          </text>
        ) : null}

        <div style={{ flexGrow: 1 }} />

        {/* 改动统计：+A -B */}
        {totalAdded > 0 ? (
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              height: 18,
              paddingLeft: 5,
              paddingRight: 5,
              borderRadius: 4,
              backgroundColor: C.overlay,
              flexShrink: 0,
            }}
          >
            <text style={{ fontSize: 10.5, lineHeight: 14, fontWeight: 500, color: C.success, whiteSpace: 'nowrap' }}>
              {`+${totalAdded}`}
            </text>
          </div>
        ) : null}
        {totalRemoved > 0 ? (
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              height: 18,
              paddingLeft: 5,
              paddingRight: 5,
              borderRadius: 4,
              backgroundColor: C.overlay,
              flexShrink: 0,
            }}
          >
            <text style={{ fontSize: 10.5, lineHeight: 14, fontWeight: 500, color: C.danger, whiteSpace: 'nowrap' }}>
              {`−${totalRemoved}`}
            </text>
          </div>
        ) : null}

        {/* 状态文字或展开收起提示 */}
        {hasAwaiting ? (
          <text style={{ fontSize: 11, lineHeight: 15, fontWeight: 600, color: C.accent, whiteSpace: 'nowrap', flexShrink: 0 }}>
            等待批准
          </text>
        ) : isRunning ? (
          <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 4, flexShrink: 0 }}>
            <Icon name="dot" size={8} color={C.link} />
            <text style={{ fontSize: 11, lineHeight: 15, color: C.link, whiteSpace: 'nowrap' }}>
              执行中…
            </text>
          </div>
        ) : hasError ? (
          <text style={{ fontSize: 10.5, lineHeight: 14, color: C.danger, whiteSpace: 'nowrap', flexShrink: 0 }}>
            {isOpen ? '收起' : '失败 · 展开'}
          </text>
        ) : hasDenied ? (
          <text style={{ fontSize: 10.5, lineHeight: 14, color: C.faint, whiteSpace: 'nowrap', flexShrink: 0 }}>
            {isOpen ? '收起' : '已拒绝 · 展开'}
          </text>
        ) : (
          <text style={{ fontSize: 10.5, lineHeight: 14, color: C.faint, whiteSpace: 'nowrap', flexShrink: 0 }}>
            {isOpen ? '收起' : '已完成 · 展开'}
          </text>
        )}
      </div>

      {/* 展开内容 */}
      {isOpen ? (
        <div
          testId={`process-body-${block.id}`}
          style={{
            display: 'flex',
            flexDirection: 'column',
            width: '100%',
            marginTop: 4,
            paddingLeft: 8,
            borderLeftWidth: 2,
            borderColor: C.borderStrong,
            gap: 3,
          }}
        >
          {block.items.map((item) => (
            <div
              key={item.id}
              style={{
                display: 'flex',
                flexDirection: 'column',
                width: '100%',
                paddingBottom: item.kind === 'notice' ? 6 : 3,
              }}
            >
              <ItemRow item={item} store={store} />
            </div>
          ))}
        </div>
      ) : null}
    </div>
  )
}

export function Transcript({ store }: { store: AgentStore }) {
  const items = store.active.items
  // 任务规划步骤不在会话区中展示，由独立的收缩悬浮框呈现
  const displayItems = items.filter((item) => !(item.kind === 'tool' && item.name === 'todo'))
  const isThreadRunning = store.isThreadRunning(store.activeId)
  const blocks = buildTranscriptBlocks(displayItems, isThreadRunning)

  const { renderer } = useGpuix()
  const listRef = useRef<PublicInstance>(null)
  const [atBottom, setAtBottom] = useState(true)
  const [tailKey, setTailKey] = useState(0)
  // 记录已完成状态下，用户主动展开的块（未记录的默认收起）
  const [userExpandedCompletedBlocks, setUserExpandedCompletedBlocks] = useState<Record<string, boolean>>({})
  // 记录运行中状态下，用户主动折叠的块（未记录的默认展开）
  const [userCollapsedRunningBlocks, setUserCollapsedRunningBlocks] = useState<Record<string, boolean>>({})

  useEffect(() => {
    setAtBottom(true)
    setUserExpandedCompletedBlocks({})
    setUserCollapsedRunningBlocks({})
  }, [store.activeId])

  const toggleBlock = (block: ProcessBlock, currentOpen: boolean) => {
    if (block.isCompleted) {
      setUserExpandedCompletedBlocks((prev) => ({
        ...prev,
        [block.id]: !currentOpen,
      }))
    } else {
      setUserCollapsedRunningBlocks((prev) => ({
        ...prev,
        [block.id]: currentOpen,
      }))
    }
  }

  const scrollToBottom = () => {
    if (blocks.length > 0) {
      if (listRef.current && renderer?.scrollToItem) {
        renderer.scrollToItem(listRef.current.id, blocks.length - 1)
      }
      setAtBottom(true)
      setTailKey((k) => k + 1)
    }
  }

  const handleVisibleRange = (event: { endIndex?: number; visibleEnd?: number }) => {
    const end = event.endIndex ?? event.visibleEnd
    if (typeof end === 'number' && blocks.length > 0) {
      setAtBottom(end >= blocks.length)
    }
  }

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        flexGrow: 1,
        minHeight: 0,
        width: '100%',
        paddingLeft: M.contentPadding,
        paddingRight: M.contentPadding,
        position: 'relative',
      }}
    >
      {blocks.length === 0 ? (
        <Welcome />
      ) : (
        <virtual-list
          key={tailKey}
          ref={listRef}
          testId="transcript-list"
          alignment="bottom"
          followTail
          overdraw={420}
          estimatedItemHeight={150}
          onVisibleRange={handleVisibleRange}
          style={{ flexGrow: 1, minHeight: 0, width: '100%' }}
        >
          {blocks.map((block) => {
            const paddingBottom =
              block.kind === 'user' ? 18 : block.kind === 'assistant' ? 18 : block.kind === 'thinking' ? 6 : 12
            const isOpen =
              block.kind === 'process'
                ? block.isCompleted
                  ? Boolean(userExpandedCompletedBlocks[block.id])
                  : !userCollapsedRunningBlocks[block.id]
                : false

            return (
              <div
                key={block.id}
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  alignItems: 'center',
                  width: '100%',
                  paddingBottom,
                }}
              >
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    width: '100%',
                    maxWidth: M.transcriptMax,
                  }}
                >
                  {block.kind === 'user' ? (
                    <UserRow item={block.item} />
                  ) : block.kind === 'assistant' ? (
                    <AssistantRow item={block.item} />
                  ) : block.kind === 'thinking' ? (
                    <ThinkingRow item={block.item} />
                  ) : (
                    <ProcessGroupCard
                      block={block}
                      store={store}
                      isOpen={isOpen}
                      onToggle={() => toggleBlock(block, isOpen)}
                    />
                  )}
                </div>
              </div>
            )
          })}
        </virtual-list>
      )}

      {/* 任务规划步骤独立收缩悬浮框 */}
      <TodoFloatingPanel store={store} />

      {!atBottom && blocks.length > 0 ? (
        <div
          testId="scroll-to-bottom"
          role="button"
          aria-label="回到底部"
          onClick={scrollToBottom}
          style={{
            position: 'absolute',
            bottom: 12,
            right: 28,
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 5,
            height: 28,
            paddingLeft: 9,
            paddingRight: 11,
            borderRadius: 14,
            cursor: 'pointer',
            backgroundColor: C.raised,
            borderWidth: 1,
            borderColor: C.borderStrong,
            boxShadow: {
              offsetX: 0,
              offsetY: 4,
              blurRadius: 12,
              spreadRadius: 0,
              color: C.shadow,
            },
            hover: {
              backgroundColor: C.chip,
            },
          }}
        >
          <Icon name="arrowDown" size={12} color={C.secondary} />
          <text style={{ fontSize: 11.5, lineHeight: 15, fontWeight: 500, color: C.secondary }}>
            回到底部
          </text>
        </div>
      ) : null}
    </div>
  )
}
