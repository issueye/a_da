/**
 * The event log behind the composer's 调试 chip.
 *
 * It shows what actually crossed the wire: the endpoint, each tool call, and
 * every error, newest first. Nothing renders here that the agent loop did not
 * record itself.
 */

import React from 'react'
import type { AgentStore } from '../agent/store'
import type { DebugEntry } from '../agent/types'
import { IconButton } from './controls'
import { C, FONT_MONO, M } from '../theme'

const KIND: Record<DebugEntry['kind'], string> = {
  request: '请求',
  delta: '增量',
  tools: '调用',
  tool: '结果',
  error: '错误',
  info: '信息',
}

function clock(at: number): string {
  const date = new Date(at)
  const pad = (value: number) => `${value}`.padStart(2, '0')
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

export function DebugPanel({ store }: { store: AgentStore }) {
  const entries = [...store.log].reverse()
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        width: 336,
        height: '100%',
        flexShrink: 0,
        backgroundColor: C.sidebar,
        borderLeftWidth: 1,
        borderColor: C.sidebarBorder,
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          height: M.titleBar,
          flexShrink: 0,
          paddingLeft: 12,
          paddingRight: 6,
          borderBottomWidth: 1,
          borderColor: C.sidebarBorder,
        }}
      >
        <text style={{ fontSize: 12, lineHeight: 16, fontWeight: 600, color: C.text }}>
          事件日志
        </text>
        <text style={{ fontSize: 11, lineHeight: 15, color: C.faint, paddingLeft: 6 }}>
          {entries.length}
        </text>
        <div style={{ flexGrow: 1 }} />
        <IconButton icon="close" testId="debug-close" label="关闭日志" onClick={() => store.toggleDebug()} />
      </div>
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          flexGrow: 1,
          minHeight: 0,
          overflowY: 'scroll',
          padding: 12,
          gap: 8,
        }}
      >
        {entries.length === 0 ? (
          <text style={{ fontSize: 11.5, lineHeight: 16, color: C.faint }}>
            还没有事件。发送任务后，模型请求、工具结果和错误都会记在这里。
          </text>
        ) : null}
        {entries.map((entry) => {
          const error = entry.kind === 'error'
          return (
            <div key={entry.id} style={{ display: 'flex', flexDirection: 'column', gap: 3 }}>
              <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 7 }}>
                <text style={{ fontFamily: FONT_MONO, fontSize: 10.5, lineHeight: 14, color: C.faint }}>
                  {clock(entry.at)}
                </text>
                <text
                  style={{
                    fontSize: 11,
                    lineHeight: 15,
                    fontWeight: 600,
                    color: error ? C.danger : C.tertiary,
                  }}
                >
                  {KIND[entry.kind]}
                </text>
              </div>
              <text
                style={{
                  fontSize: 11.5,
                  lineHeight: 17,
                  color: error ? C.danger : C.secondary,
                }}
              >
                {entry.text}
              </text>
            </div>
          )
        })}
      </div>
    </div>
  )
}
