/**
 * The conversation column.
 *
 * Rows are the same items the store appends during a turn: the user's tasks,
 * streamed model replies rendered as markdown, one card per tool call, and the
 * approval gate that holds a write until the user answers it.
 */

import React, { useState } from 'react'
import { describeTool } from '../agent/tools'
import type { AgentStore } from '../agent/store'
import type { Item, ToolStatus } from '../agent/types'
import { Icon } from './controls'
import type { IconName } from '../icons'
import { C, DOC_THEME, FONT_MONO, M } from '../theme'
import { Welcome } from './Welcome'

const TOOL_LABEL: Record<string, string> = {
  list_files: '列出文件',
  read_file: '读取文件',
  search_files: '搜索代码',
  write_file: '写入文件',
  edit_file: '修改文件',
  run_command: '执行命令',
}

const TOOL_ICON: Record<string, IconName> = {
  list_files: 'folder',
  read_file: 'file',
  search_files: 'search',
  write_file: 'file',
  edit_file: 'file',
  run_command: 'terminal',
}

const STATUS: Record<ToolStatus, { label: string; color: string }> = {
  awaiting: { label: '等待批准', color: C.accent },
  running: { label: '执行中', color: C.tertiary },
  done: { label: '完成', color: C.success },
  error: { label: '失败', color: C.danger },
  denied: { label: '已拒绝', color: C.faint },
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

function AssistantRow({ item }: { item: Extract<Item, { kind: 'assistant' }> }) {
  if (!item.text.trim() && !item.streaming) return null
  return (
    <div style={{ display: 'flex', flexDirection: 'column', width: '100%' }}>
      {item.text ? <markdown source={item.text} theme={DOC_THEME} /> : null}
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
    </div>
  )
}

function ToolCard({ item, store }: { item: Extract<Item, { kind: 'tool' }>; store: AgentStore }) {
  const status = STATUS[item.status]
  return (
    <div
      testId={`tool-${item.id}`}
      style={{
        display: 'flex',
        flexDirection: 'column',
        width: '100%',
        borderWidth: 1,
        borderColor: C.cardBorder,
        borderRadius: 10,
        backgroundColor: C.tool,
        overflow: 'hidden',
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 8,
          paddingTop: 7,
          paddingBottom: 7,
          paddingLeft: 10,
          paddingRight: 10,
          backgroundColor: C.card,
          borderBottomWidth: item.patch || item.output ? 1 : 0,
          borderColor: C.cardBorder,
        }}
      >
        <Icon name={TOOL_ICON[item.name] ?? 'terminal'} size={12} color={C.tertiary} />
        <text style={{ fontSize: 11.5, lineHeight: 16, fontWeight: 600, color: C.secondary }}>
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
            flexShrink: 1,
          }}
        >
          {describeTool(item.name, item.args)}
        </text>
        <div style={{ flexGrow: 1 }} />
        <text style={{ fontSize: 11, lineHeight: 15, color: status.color }}>{status.label}</text>
      </div>

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
              backgroundColor: '#FFFFFF',
              borderWidth: 1,
              borderColor: C.borderStrong,
              hover: { backgroundColor: C.chip },
            }}
          >
            <text style={{ fontSize: 12, lineHeight: 16, color: C.secondary }}>拒绝</text>
          </div>
        </div>
      ) : null}

      {item.patch ? (
        <diff patch={item.patch} wordDiff maxLines={22} theme={DOC_THEME} />
      ) : null}

      {item.output && !item.patch ? (
        <div style={{ paddingTop: 9, paddingBottom: 9, paddingLeft: 10, paddingRight: 10 }}>
          <MonoBlock text={item.output} tone={item.status === 'error' ? C.danger : C.secondary} />
        </div>
      ) : null}
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

function ItemRow({ item, store }: { item: Item; store: AgentStore }) {
  if (item.kind === 'user') return <UserRow item={item} />
  if (item.kind === 'assistant') return <AssistantRow item={item} />
  if (item.kind === 'tool') return <ToolCard item={item} store={store} />
  return <NoticeRow item={item} />
}

export function Transcript({ store }: { store: AgentStore }) {
  const items = store.active.items
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
      }}
    >
      {items.length === 0 ? (
        <Welcome />
      ) : (
        <virtual-list
          alignment="bottom"
          followTail
          overdraw={420}
          estimatedItemHeight={150}
          style={{ flexGrow: 1, minHeight: 0, width: '100%' }}
        >
          {items.map((item) => (
            <div
              key={item.id}
              style={{
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'center',
                width: '100%',
                paddingBottom: 18,
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
                <ItemRow item={item} store={store} />
              </div>
            </div>
          ))}
        </virtual-list>
      )}
    </div>
  )
}
