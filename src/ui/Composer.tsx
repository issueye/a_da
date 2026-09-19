/**
 * The task box.
 *
 * Enter sends, Shift+Enter inserts a line, and a send that arrives while a turn
 * is running is queued rather than dropped. The chips under the box are the
 * three settings the mock shows: approval, event log and reasoning effort.
 */

import React, { useState } from 'react'
import { Select, SelectContent, SelectItem, SelectTrigger } from '@gpuix/react'
import {
  APPROVAL_OPTIONS,
  EFFORT_OPTIONS,
  type AgentStore,
  type ApprovalMode,
  type Effort,
} from '../agent/store'
import { ChipButton, ChipSelect, Icon, MENU_LAYER, MenuRow, MenuSurface, menuItemStyle } from './controls'
import { C, EDITOR_THEME, M } from '../theme'

const DEBUG_OPTIONS = [
  { value: 'off', label: '关闭' },
  { value: 'on', label: '显示事件日志' },
]

function AppendMenu({ store, onPick }: { store: AgentStore; onPick: (value: string) => void }) {
  const entries = store.entries
  return (
    <Select value="" onValueChange={onPick}>
      <div style={{ position: 'relative', display: 'flex' }}>
        <SelectTrigger
          testId="composer-add"
          style={(state) => ({
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            justifyContent: 'center',
            width: 22,
            height: 22,
            borderRadius: 6,
            cursor: 'pointer',
            backgroundColor: state.open ? C.chip : '#00000000',
            hover: { backgroundColor: C.chip },
          })}
        >
          <Icon name="plus" size={14} color={C.secondary} />
        </SelectTrigger>
        <SelectContent side="top" sideOffset={6} style={{ ...MENU_LAYER, minWidth: 250 }}>
          <MenuSurface maxHeight={320}>
            {entries.length ? (
              entries.map((entry) => (
                <SelectItem key={entry} value={entry} style={menuItemStyle}>
                  <MenuRow label={entry} selected={false} />
                </SelectItem>
              ))
            ) : (
              <SelectItem value="__none" disabled style={menuItemStyle}>
                <MenuRow label="工作区索引中…" selected={false} />
              </SelectItem>
            )}
          </MenuSurface>
        </SelectContent>
      </div>
    </Select>
  )
}

export function Composer({ store }: { store: AgentStore }) {
  const [draft, setDraft] = useState('')
  const running = store.running
  const ready = draft.trim().length > 0
  const approval = APPROVAL_OPTIONS.find((option) => option.value === store.approval)!
  const effort = EFFORT_OPTIONS.find((option) => option.value === store.effort)!

  const send = (text: string) => {
    if (!text.trim()) return
    store.send(text)
    setDraft('')
  }

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        flexShrink: 0,
        paddingLeft: M.contentPadding,
        paddingRight: M.contentPadding,
        paddingTop: 8,
        paddingBottom: 16,
        userSelect: 'none',
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          width: '100%',
          maxWidth: M.composerMax,
          backgroundColor: C.canvas,
          borderWidth: 1,
          borderColor: C.borderStrong,
          borderRadius: 12,
          paddingTop: 8,
          paddingBottom: 8,
        }}
      >
        <textarea
          testId="composer"
          value={draft}
          placeholder={running ? '继续输入以排队后续修改' : '描述要 Agent 完成的任务'}
          minRows={1}
          maxRows={7}
          theme={EDITOR_THEME}
          style={{
            width: '100%',
            minWidth: 0,
            fontSize: 13.5,
            lineHeight: 20,
            color: C.text,
            backgroundColor: '#00000000',
            borderWidth: 0,
            paddingLeft: 11,
            paddingRight: 11,
          }}
          onChange={(event) => setDraft(event.value ?? '')}
          onSubmit={(event) => send(event.value ?? draft)}
        />
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 2,
            marginTop: 6,
            paddingLeft: 8,
            paddingRight: 8,
          }}
        >
          <AppendMenu store={store} onPick={(value) => setDraft((text) => `${text}${value} `)} />
          <ChipSelect
            testId="approval"
            value={store.approval}
            onChange={(next) => store.setApproval(next as ApprovalMode)}
            items={APPROVAL_OPTIONS}
            icon="shield"
            label={approval.label}
            menuWidth={230}
          >
            <SelectItem testId="approval-auto" value="auto" style={menuItemStyle}>
              <MenuRow
                label="自动批准"
                description="读写文件和执行命令都不再询问"
                selected={store.approval === 'auto'}
              />
            </SelectItem>
            <SelectItem testId="approval-ask" value="ask" style={menuItemStyle}>
              <MenuRow
                label="每次询问"
                description="每次工具调用都等你确认"
                selected={store.approval === 'ask'}
              />
            </SelectItem>
            <SelectItem testId="approval-readonly" value="readonly" style={menuItemStyle}>
              <MenuRow
                label="只读"
                description="只允许读取，写入与命令需批准"
                selected={store.approval === 'readonly'}
              />
            </SelectItem>
          </ChipSelect>

          <div style={{ flexGrow: 1 }} />

          {running ? (
            <ChipButton testId="stop" icon="square" label="停止" onClick={() => store.stop()} />
          ) : null}
          <ChipButton
            testId="composer-refresh"
            icon="refresh"
            label="刷新"
            onClick={() => void store.refresh()}
          />
          <ChipSelect
            testId="debug"
            value={store.debugOpen ? 'on' : 'off'}
            onChange={(next) => {
              if ((next === 'on') !== store.debugOpen) store.toggleDebug()
            }}
            items={DEBUG_OPTIONS}
            icon="bug"
            label="调试"
          >
            <SelectItem testId="debug-off" value="off" style={menuItemStyle}>
              <MenuRow label="关闭" description="隐藏右侧的事件日志" selected={!store.debugOpen} />
            </SelectItem>
            <SelectItem testId="debug-on" value="on" style={menuItemStyle}>
              <MenuRow
                label="显示事件日志"
                description="模型请求、工具结果和错误"
                selected={store.debugOpen}
              />
            </SelectItem>
          </ChipSelect>
          <ChipSelect
            testId="effort"
            value={store.effort}
            onChange={(next) => store.setEffort(next as Effort)}
            items={EFFORT_OPTIONS}
            icon="brain"
            label={effort.label}
            menuWidth={170}
          >
            {EFFORT_OPTIONS.map((option) => (
              <SelectItem key={option.value} testId={`effort-${option.value}`} value={option.value} style={menuItemStyle}>
                <MenuRow
                  label={option.label}
                  description={option.value === 'max' ? '默认，最慢也最稳' : undefined}
                  selected={store.effort === option.value}
                />
              </SelectItem>
            ))}
          </ChipSelect>

          <div
            testId="send"
            role="button"
            aria-label={running ? '排队这条指令' : '发送'}
            onClick={() => send(draft)}
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              justifyContent: 'center',
              width: 28,
              height: 28,
              marginLeft: 4,
              borderRadius: 8,
              flexShrink: 0,
              cursor: ready ? 'pointer' : 'default',
              backgroundColor: C.inverse,
              hover: { opacity: ready ? 0.88 : 1 },
            }}
          >
            <Icon name="send" size={13} color={C.onInverse} />
          </div>
        </div>
      </div>
    </div>
  )
}
