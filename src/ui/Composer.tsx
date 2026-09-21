/**
 * The task box.
 *
 * Enter sends, Shift+Enter inserts a line, and a send that arrives while a turn
 * is running is queued rather than dropped. The chips under the box are the
 * three settings the mock shows: approval, event log and reasoning effort.
 */

import React, { useEffect, useState } from 'react'
import { Select, SelectContent, SelectItem, SelectTrigger } from '@gpuix/react'
import {
  APPROVAL_OPTIONS,
  EFFORT_OPTIONS,
  type AgentStore,
  type ApprovalMode,
  type Effort,
} from '../agent/store'
import { ChipButton, ChipSelect, Icon, menuLayer, MenuRow, MenuSurface, menuItemStyle } from './controls'
import { C, editorTheme, M } from '../theme'
import { formatDuration, formatNumber, formatTokenShort } from './Transcript'

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
        <SelectContent side="top" sideOffset={6} style={{ ...menuLayer(), minWidth: 250 }}>
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

export function Composer({ store, centered }: { store: AgentStore; centered?: boolean }) {
  const [draft, setDraft] = useState('')
  const [focused, setFocused] = useState(false)

  // 当外部有注入待发送/草稿时（例如提示词一键应用），优先显示与消费
  const currentDraft = draft || store.pendingDraft || ''

  useEffect(() => {
    if (store.pendingDraft !== null) {
      const text = store.pendingDraft
      setDraft(text)
      const timer = setTimeout(() => {
        if (store.pendingDraft === text) {
          store.clearPendingDraft()
        }
      }, 50)
      return () => clearTimeout(timer)
    }
  }, [store.pendingDraft])

  const running = store.running
  const ready = currentDraft.trim().length > 0
  const approval = APPROVAL_OPTIONS.find((option) => option.value === store.approval)!
  const effort = EFFORT_OPTIONS.find((option) => option.value === store.effort)!
  const modelLabel = store.currentModel ? store.currentModel : '配置模型'

  const send = (text: string) => {
    const target = text.trim() ? text : currentDraft
    if (!target.trim()) return
    store.clearPendingDraft()
    store.send(target)
    setDraft('')
  }

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        flexShrink: 0,
        width: '100%',
        paddingLeft: centered ? 0 : M.contentPadding,
        paddingRight: centered ? 0 : M.contentPadding,
        paddingTop: centered ? 0 : 8,
        paddingBottom: centered ? 0 : 16,
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
          borderColor: focused ? C.link : C.borderStrong,
          borderRadius: 14,
          paddingTop: centered ? 12 : 8,
          paddingBottom: centered ? 10 : 8,
          boxShadow: focused
            ? {
                offsetX: 0,
                offsetY: 3,
                blurRadius: 12,
                spreadRadius: 0,
                color: C.shadow,
              }
            : centered
            ? {
                offsetX: 0,
                offsetY: 2,
                blurRadius: 10,
                spreadRadius: 0,
                color: C.shadow,
              }
            : undefined,
        }}
      >
        <textarea
          testId="composer"
          value={currentDraft}
          placeholder={
            running
              ? '继续输入以排队后续修改'
              : centered
              ? '描述要 Agent 完成的任务 (Ask anything, @ to mention, / for actions)'
              : '描述要 Agent 完成的任务'
          }
          minRows={centered ? 2 : 1}
          maxRows={7}
          theme={editorTheme()}
          style={{
            width: '100%',
            minWidth: 0,
            fontSize: 13.5,
            lineHeight: 20,
            color: C.text,
            backgroundColor: '#00000000',
            borderWidth: 0,
            paddingLeft: 12,
            paddingRight: 12,
          }}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          onChange={(event) => {
            if (store.pendingDraft !== null) store.clearPendingDraft()
            setDraft(event.value ?? '')
          }}
          onSubmit={(event) => send(event.value?.trim() ? event.value : currentDraft)}
        />
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 3,
            marginTop: 6,
            paddingLeft: 8,
            paddingRight: 8,
          }}
        >
          {/* 左侧控制区：模型标识、文件引入、权限模式、思考深度 */}
          <div
            testId="composer-model"
            role="button"
            aria-label={`模型：${modelLabel}，点击配置`}
            onClick={() => store.setSettings(true)}
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 5,
              height: 22,
              paddingLeft: 7,
              paddingRight: 8,
              borderRadius: 6,
              cursor: 'pointer',
              backgroundColor: C.chip,
              borderWidth: 1,
              borderColor: C.chipBorder,
              hover: { backgroundColor: C.chipHover },
            }}
          >
            <Icon name="sparkles" size={12} color={store.currentModel ? C.link : C.tertiary} />
            <text
              style={{
                fontSize: 11.5,
                fontWeight: 500,
                color: store.currentModel ? C.text : C.secondary,
                whiteSpace: 'nowrap',
                textOverflow: 'ellipsis',
                maxWidth: 130,
              }}
            >
              {modelLabel}
            </text>
          </div>

          {store.activeThreadStats.totalTokens > 0 ? (
            <div
              testId="composer-thread-tokens"
              aria-label={`会话累计 Token：${formatNumber(store.activeThreadStats.totalTokens)} (输入: ${formatNumber(store.activeThreadStats.totalPromptTokens)} · 输出: ${formatNumber(store.activeThreadStats.totalCompletionTokens)}) · 总耗时: ${formatDuration(store.activeThreadStats.totalDurationMs)} · 共 ${store.activeThreadStats.turnsCount} 轮`}
              style={{
                display: 'flex',
                flexDirection: 'row',
                alignItems: 'center',
                gap: 4,
                height: 22,
                paddingLeft: 7,
                paddingRight: 8,
                borderRadius: 6,
                backgroundColor: C.chip,
                borderWidth: 1,
                borderColor: C.chipBorder,
              }}
            >
              <Icon name="coins" size={11} color={C.tertiary} />
              <text
                style={{
                  fontSize: 11,
                  color: C.secondary,
                  whiteSpace: 'nowrap',
                }}
              >
                {`${formatTokenShort(store.activeThreadStats.totalTokens)} tokens`}
              </text>
            </div>
          ) : null}

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

          {/* 弹性占位分割 */}
          <div style={{ flexGrow: 1 }} />

          {/* 右侧操作区：运行中停止按键、刷新、调试日志与主发送按键 */}
          {running ? (
            <div
              testId="stop"
              role="button"
              aria-label="停止"
              onClick={() => store.stop()}
              style={{
                display: 'flex',
                flexDirection: 'row',
                alignItems: 'center',
                gap: 4,
                height: 24,
                paddingLeft: 8,
                paddingRight: 9,
                borderRadius: 6,
                cursor: 'pointer',
                backgroundColor: C.accentSoft,
                borderWidth: 1,
                borderColor: C.accent,
                hover: { opacity: 0.85 },
              }}
            >
              <Icon name="square" size={10} color={C.accent} />
              <text style={{ fontSize: 11.5, fontWeight: 600, color: C.accent }}>停止</text>
            </div>
          ) : null}

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

          <div
            testId="send"
            role="button"
            aria-label={running ? '排队这条指令' : '发送'}
            onClick={() => send(currentDraft)}
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              justifyContent: 'center',
              width: 28,
              height: 28,
              marginLeft: 2,
              borderRadius: 8,
              flexShrink: 0,
              cursor: ready ? 'pointer' : 'default',
              backgroundColor: ready ? C.inverse : C.overlay,
              hover: { opacity: ready ? 0.85 : 1 },
            }}
          >
            <Icon name="arrowUp" size={14} color={ready ? C.onInverse : C.faint} />
          </div>
        </div>
      </div>
    </div>
  )
}
