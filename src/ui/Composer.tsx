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
import { computeThreadStats, type AgentMode, type Item, type Thread } from '../agent/types'
import { computeContextBreakdown, type ContextUsageSummary } from '../agent/stats'
import { ContextUsagePopover } from './ContextUsagePopover'
import type { IconName } from '../icons'

export const MODE_OPTIONS: { value: AgentMode; label: string; icon: IconName; desc: string }[] = [
  { value: 'code', label: 'Code 编码', icon: 'code', desc: '全能敏捷编码与工程构建 (默认)' },
  { value: 'plan', label: 'Plan 规划', icon: 'compass', desc: '只读架构分析与实施计划设计 (只读防写)' },
  { value: 'create', label: 'Create 创造', icon: 'sparkles', desc: '智能体自我进化与工具/技能 CRUD' },
]

import { getModelContextWindow } from '../agent/compact'
export { getModelContextWindow }

export interface ThreadTelemetry {
  turns: number
  steps: number
  tokPerSec: number
  totalTokens: number
  promptTokens: number
  completionTokens: number
  cachedTokens: number
  totalPromptTokens: number
  totalCompletionTokens: number
  totalCachedTokens: number
  cacheHitRatio: number
  durationMs: number
  currentContextTokens: number
  contextLimit: number
  contextRatio: number
  hasUsage: boolean
  contextSummary: ContextUsageSummary
}

/** 格式化耗时展示 */
function formatElapsed(ms?: number): string {
  if (!ms || ms <= 0) return '0s'
  return formatDuration(ms) || '0s'
}

/** 实时计算当前会话的遥测指标：只取最后一次请求返回的指标，不进行跨轮累加 */
export function computeThreadTelemetry(
  thread: Thread,
  isRunning: boolean,
  currentModel?: string,
  configuredLimit?: number,
): ThreadTelemetry {
  const userItems = thread.items.filter((it) => it.kind === 'user')
  const toolItems = thread.items.filter((it) => it.kind === 'tool')
  const assistantItems = thread.items.filter(
    (it): it is Extract<Item, { kind: 'assistant' }> => it.kind === 'assistant',
  )

  const turns = userItems.length
  const steps = toolItems.length

  // 流式运行中的助手消息
  const streamingAssistant = assistantItems.find((it) => it.streaming)

  // 定位最后一次请求的助手消息：若正在流式以流式消息为准，否则倒序取最近一条有统计数据的回复
  let latestAssistant = streamingAssistant
  if (!latestAssistant) {
    for (let i = assistantItems.length - 1; i >= 0; i--) {
      const a = assistantItems[i]
      if (a.usage || a.durationMs) {
        latestAssistant = a
        break
      }
    }
  }

  // 单次请求指标：严格基于最后一次请求返回的真实 Token，不进行跨轮累加
  const promptTokens = latestAssistant?.usage?.promptTokens ?? 0
  const completionTokens = latestAssistant?.usage?.completionTokens ?? 0
  const cachedTokens = latestAssistant?.usage?.cachedTokens ?? 0
  const totalTokens =
    latestAssistant?.usage?.totalTokens ?? (promptTokens + completionTokens)
  const hasUsage = !!latestAssistant?.usage

  // 耗时计算：流式中使用当前流逝时间，已完成使用记录的 durationMs
  let durationMs = 0
  if (streamingAssistant) {
    durationMs = Math.max(100, Date.now() - (streamingAssistant.at || Date.now()))
  } else if (latestAssistant?.durationMs) {
    durationMs = latestAssistant.durationMs
  }

  // 缓存命中率
  const cacheHitRatio =
    promptTokens > 0 ? Math.min(100, Math.round((cachedTokens / promptTokens) * 100)) : 0

  // 计算 tok/s 生成速率（基于最后一次请求的补全 Token 与耗时）
  let tokPerSec = 0
  if (streamingAssistant) {
    if (completionTokens > 0) {
      const elapsedSec = Math.max(0.3, durationMs / 1000)
      tokPerSec = Math.round(completionTokens / elapsedSec)
    }
  } else if (latestAssistant && latestAssistant.durationMs && completionTokens > 0) {
    tokPerSec = Math.round(completionTokens / (latestAssistant.durationMs / 1000))
  }

  // 当前会话上下文占用与比率：基于最后一次请求模型实际读取的 promptTokens
  const currentContextTokens = promptTokens
  const contextLimit = getModelContextWindow(currentModel, configuredLimit)
  const contextRatio =
    contextLimit > 0 ? Math.min(100, Math.round((currentContextTokens / contextLimit) * 100)) : 0

  const contextSummary = computeContextBreakdown({
    items: thread.items,
    realPromptTokens: promptTokens,
    realCompletionTokens: completionTokens,
    realCachedTokens: cachedTokens,
    contextLimit,
  })

  return {
    turns,
    steps,
    tokPerSec,
    totalTokens,
    promptTokens,
    completionTokens,
    cachedTokens,
    totalPromptTokens: promptTokens,
    totalCompletionTokens: completionTokens,
    totalCachedTokens: cachedTokens,
    cacheHitRatio,
    durationMs,
    currentContextTokens,
    contextLimit,
    contextRatio,
    hasUsage,
    contextSummary,
  }
}

/** 遥测信息栏项之间的轻量竖线分隔符 */
function TelemetryDivider() {
  return (
    <text
      style={{
        fontSize: 10,
        color: C.borderStrong,
        opacity: 0.65,
        marginLeft: 2,
        marginRight: 2,
        userSelect: 'none',
      }}
    >
      |
    </text>
  )
}

export function ComposerTelemetryBar({
  store,
  centered,
}: {
  store: AgentStore
  centered?: boolean
}) {
  const [popoverOpen, setPopoverOpen] = useState(false)
  const thread = store.active
  // 空会话初始居中模式时不展示，进入会话或有消息时开始展示
  if (centered && thread.items.length === 0) {
    return null
  }

  const telemetry = computeThreadTelemetry(thread, store.running, store.currentModel, store.contextWindow)
  const {
    turns,
    steps,
    tokPerSec,
    totalTokens,
    promptTokens,
    completionTokens,
    cachedTokens,
    cacheHitRatio,
    durationMs,
    currentContextTokens,
    contextLimit,
    contextRatio,
  } = telemetry

  const contextColor = contextRatio >= 85 ? C.accent : C.secondary

  return (
    <div
      testId="composer-telemetry"
      style={{
        position: 'relative',
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'flex-start',
        flexWrap: 'wrap',
        width: '100%',
        maxWidth: M.composerMax,
        paddingLeft: 8,
        paddingRight: 8,
        paddingTop: 6,
        paddingBottom: 2,
        gap: 8,
        rowGap: 4,
        userSelect: 'none',
      }}
    >
      {/* 1. 轮数、步数与生成速率 */}
      <div
        testId="telemetry-turns-steps"
        aria-label={`已进行 ${turns} 轮对话，执行 ${steps} 步操作，生成速率 ${tokPerSec} tok/s`}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 4,
          cursor: 'default',
        }}
      >
        <Icon name="gauge" size={12} color={C.tertiary} />
        <text style={{ fontSize: 11.5, color: C.secondary, whiteSpace: 'nowrap' }}>
          {`${turns} 轮 ${steps} 步 · ${tokPerSec} tok/s`}
        </text>
      </div>

      <TelemetryDivider />

      {/* 2. 单次总 Token */}
      <div
        testId="telemetry-tokens-cache"
        aria-label={`最后一次请求总 Token 消耗: ${formatNumber(totalTokens)}`}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 4,
          cursor: 'default',
        }}
      >
        <Icon name="database" size={12} color={C.tertiary} />
        <text style={{ fontSize: 11.5, color: C.secondary, whiteSpace: 'nowrap' }}>
          {`${formatTokenShort(totalTokens)} tok`}
        </text>
      </div>

      <TelemetryDivider />

      {/* 3. 系统提示词与上下文输入 */}
      <div
        testId="telemetry-prompt-tokens"
        aria-label={`系统提示词与输入消耗: ${formatNumber(promptTokens)} Token`}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          cursor: 'default',
        }}
      >
        <text style={{ fontSize: 11.5, color: C.secondary, whiteSpace: 'nowrap' }}>
          {`提示词 ${formatTokenShort(promptTokens)}`}
        </text>
      </div>

      <TelemetryDivider />

      {/* 4. 模型回复输出 */}
      <div
        testId="telemetry-completion-tokens"
        aria-label={`模型生成输出消耗: ${formatNumber(completionTokens)} Token`}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          cursor: 'default',
        }}
      >
        <text style={{ fontSize: 11.5, color: C.secondary, whiteSpace: 'nowrap' }}>
          {`输出 ${formatTokenShort(completionTokens)}`}
        </text>
      </div>

      <TelemetryDivider />

      {/* 5. 缓存命中 */}
      <div
        testId="telemetry-cached-tokens"
        aria-label={`缓存命中读取: ${formatNumber(cachedTokens)} Token (命中率 ${cacheHitRatio}%)`}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          cursor: 'default',
        }}
      >
        <text style={{ fontSize: 11.5, color: C.secondary, whiteSpace: 'nowrap' }}>
          {cachedTokens > 0
            ? `缓存 ${formatTokenShort(cachedTokens)} (${cacheHitRatio}%)`
            : '缓存 0'}
        </text>
      </div>

      <TelemetryDivider />

      {/* 6. 单次请求用时 */}
      <div
        testId="telemetry-duration"
        aria-label={`最后一次请求用时: ${durationMs}ms`}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 4,
          cursor: 'default',
        }}
      >
        <Icon name="clock" size={12} color={C.tertiary} />
        <text style={{ fontSize: 11.5, color: C.secondary, whiteSpace: 'nowrap' }}>
          {`用时 ${formatElapsed(durationMs)}`}
        </text>
      </div>

      <TelemetryDivider />

      {/* 7. 上下文窗口占用率与深度洞察触发器 */}
      <div
        testId="telemetry-context-ratio"
        role="button"
        aria-label={`当前上下文占用 ${formatNumber(currentContextTokens)} / ${formatNumber(contextLimit)} (${contextRatio}%)，点击查看细分构成与深度洞察`}
        onClick={() => setPopoverOpen((open) => !open)}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 5,
          cursor: 'pointer',
          paddingLeft: 4,
          paddingRight: 4,
          paddingTop: 1,
          paddingBottom: 1,
          borderRadius: 4,
          backgroundColor: popoverOpen ? C.chip : 'transparent',
          hover: { backgroundColor: C.chipHover },
        }}
      >
        <Icon name="pieChart" size={12} color={contextColor} />
        <text
          style={{
            fontSize: 11.5,
            fontWeight: 500,
            color: contextColor,
            whiteSpace: 'nowrap',
          }}
        >
          {`上下文 ${contextRatio === 0 && currentContextTokens > 0 ? '<1%' : `${contextRatio}%`}`}
        </text>
        {/* 微型进度条 */}
        <div
          style={{
            width: 24,
            height: 4,
            borderRadius: 2,
            backgroundColor: C.overlay,
            overflow: 'hidden',
          }}
        >
          <div
            style={{
              width: `${Math.max(4, Math.min(100, contextRatio))}%`,
              height: '100%',
              backgroundColor: contextColor,
            }}
          />
        </div>
      </div>

      {/* 快捷压缩按钮（当上下文占用达到警戒线 >= 60% 时显式提供直接压缩按钮） */}
      {contextRatio >= 60 ? (
        <>
          <TelemetryDivider />
          <div
            testId="telemetry-quick-compact-btn"
            role="button"
            aria-label="一键压缩上下文与生成会话摘要"
            onClick={() => {
              void store.compactThread(thread.id, { trigger: 'manual' })
            }}
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 4,
              cursor: 'pointer',
              paddingLeft: 6,
              paddingRight: 6,
              paddingTop: 1,
              paddingBottom: 1,
              borderRadius: 4,
              backgroundColor: contextRatio >= 85 ? '#ef444420' : '#10b98118',
              borderWidth: 1,
              borderColor: contextRatio >= 85 ? '#ef444460' : '#10b98140',
              hover: { backgroundColor: contextRatio >= 85 ? '#ef444435' : '#10b98130' },
            }}
          >
            <Icon name="sparkles" size={11} color={contextRatio >= 85 ? '#ef4444' : '#10b981'} />
            <text
              style={{
                fontSize: 11,
                fontWeight: 600,
                color: contextRatio >= 85 ? '#ef4444' : '#10b981',
                whiteSpace: 'nowrap',
              }}
            >
              压缩
            </text>
          </div>
        </>
      ) : null}

      {/* 点击弹出的上下文用量与健康度悬浮面板 */}
      {popoverOpen ? (
        <ContextUsagePopover
          summary={telemetry.contextSummary}
          onClose={() => setPopoverOpen(false)}
          onCompact={() => void store.compactThread(thread.id, { trigger: 'manual' })}
        />
      ) : null}
    </div>
  )
}

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
  const [images, setImages] = useState<string[]>([])
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
  const ready = currentDraft.trim().length > 0 || images.length > 0
  const approval = APPROVAL_OPTIONS.find((option) => option.value === store.approval)!
  const effort = EFFORT_OPTIONS.find((option) => option.value === store.effort)!
  const modeOption = MODE_OPTIONS.find((m) => m.value === (store.mode ?? 'code')) ?? MODE_OPTIONS[0]!
  const modelLabel = store.currentModel ? store.currentModel : '配置模型'
  const imageEntries = store.entries.filter((f) => /\.(png|jpe?g|webp|gif|svg)$/i.test(f))

  const send = (text: string) => {
    const target = text.trim() ? text : currentDraft
    if (!target.trim() && images.length === 0) return
    store.clearPendingDraft()
    store.send(target, images.length > 0 ? images : undefined)
    setDraft('')
    setImages([])
  }

  // 子智能体独立会话：保持窗口对话 UI 风格，但不允许手动输入
  if (store.active.isSubagent) {
    const isRunning = store.running
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
          testId="subagent-readonly-bar"
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            justifyContent: 'space-between',
            width: '100%',
            maxWidth: M.composerMax,
            backgroundColor: C.canvas,
            borderWidth: 1,
            borderColor: C.borderStrong,
            borderRadius: 14,
            paddingLeft: 16,
            paddingRight: 12,
            paddingTop: 10,
            paddingBottom: 10,
            gap: 12,
          }}
        >
          <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 10, flexGrow: 1, minWidth: 0 }}>
            <div
              style={{
                width: 26,
                height: 26,
                borderRadius: 7,
                backgroundColor: C.chip,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                flexShrink: 0,
              }}
            >
              <Icon name="bot" size={14} color={C.link} />
            </div>
            <div style={{ display: 'flex', flexDirection: 'column', flexGrow: 1, minWidth: 0 }}>
              <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                <text style={{ fontSize: 13, fontWeight: 600, color: C.text }}>
                  子智能体专属执行会话
                </text>
                <div
                  style={{
                    paddingLeft: 6,
                    paddingRight: 6,
                    height: 18,
                    borderRadius: 4,
                    backgroundColor: C.chipHover,
                    display: 'flex',
                    alignItems: 'center',
                  }}
                >
                  <text style={{ fontSize: 10, color: C.tertiary, whiteSpace: 'nowrap' }}>
                    独立工作区
                  </text>
                </div>
              </div>
              <text
                style={{
                  fontSize: 11,
                  color: C.faint,
                  whiteSpace: 'nowrap',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                }}
              >
                由主 Agent 自动调度执行，保持会话独立只读，不允许手动输入
              </text>
            </div>
          </div>

          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 8,
              flexShrink: 0,
            }}
          >
            {isRunning ? (
              <div
                testId="stop-subagent"
                role="button"
                aria-label="停止子智能体"
                onClick={() => store.stop()}
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 4,
                  height: 28,
                  paddingLeft: 10,
                  paddingRight: 10,
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

            {store.active.parentId ? (
              <div
                testId="return-parent-thread"
                role="button"
                aria-label="返回主会话"
                onClick={() => {
                  if (store.active.parentId) {
                    store.selectThread(store.active.parentId)
                  }
                }}
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 6,
                  height: 28,
                  paddingLeft: 12,
                  paddingRight: 12,
                  borderRadius: 6,
                  cursor: 'pointer',
                  backgroundColor: C.overlayStrong,
                  hover: { backgroundColor: C.tab },
                }}
              >
                <text style={{ fontSize: 12, fontWeight: 500, color: C.text }}>返回主会话</text>
                <Icon name="arrowRight" size={12} color={C.secondary} />
              </div>
            ) : null}
          </div>
        </div>
        <ComposerTelemetryBar store={store} centered={centered} />
      </div>
    )
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
              : store.mode === 'plan'
              ? '描述要 Agent 完成的任务 (Plan 规划模式)'
              : store.mode === 'create'
              ? '描述要 Agent 完成的任务 (Create 创造模式)'
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
        {images.length > 0 ? (
          <div
            testId="composer-attached-images"
            style={{
              display: 'flex',
              flexDirection: 'row',
              flexWrap: 'wrap',
              gap: 6,
              paddingLeft: 12,
              paddingRight: 12,
              paddingTop: 2,
              paddingBottom: 4,
            }}
          >
            {images.map((img, idx) => (
              <div
                key={idx}
                testId={`composer-image-pill-${idx}`}
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 4,
                  height: 22,
                  paddingLeft: 7,
                  paddingRight: 6,
                  borderRadius: 6,
                  backgroundColor: C.chip,
                  borderWidth: 1,
                  borderColor: C.chipBorder,
                }}
              >
                <Icon name="image" size={11} color={C.link} />
                <text
                  style={{
                    fontSize: 11,
                    color: C.text,
                    maxWidth: 160,
                    whiteSpace: 'nowrap',
                    textOverflow: 'ellipsis',
                  }}
                >
                  {img.split(/[/\\]/).pop() ?? img}
                </text>
                <div
                  role="button"
                  aria-label="移除图片"
                  onClick={() => setImages((prev) => prev.filter((_, i) => i !== idx))}
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    width: 14,
                    height: 14,
                    borderRadius: 3,
                    cursor: 'pointer',
                    hover: { backgroundColor: C.overlayStrong },
                  }}
                >
                  <Icon name="x" size={9} color={C.tertiary} />
                </div>
              </div>
            ))}
          </div>
        ) : null}
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

          <AppendMenu store={store} onPick={(value) => setDraft((text) => `${text}${value} `)} />
          {store.supportsImages ? (
            <Select
              value=""
              onValueChange={(val) => {
                if (val && val !== '__none' && !images.includes(val)) {
                  setImages((prev) => [...prev, val])
                }
              }}
            >
              <div style={{ position: 'relative', display: 'flex' }}>
                <SelectTrigger
                  testId="composer-attach-image"
                  aria-label="添加图片"
                  style={(state) => ({
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    justifyContent: 'center',
                    width: 22,
                    height: 22,
                    borderRadius: 6,
                    cursor: 'pointer',
                    backgroundColor: state.open || images.length > 0 ? C.chip : '#00000000',
                    hover: { backgroundColor: C.chip },
                  })}
                >
                  <Icon name="image" size={13} color={images.length > 0 ? C.link : C.secondary} />
                </SelectTrigger>
                <SelectContent side="top" sideOffset={6} style={{ ...menuLayer(), minWidth: 260 }}>
                  <MenuSurface maxHeight={320}>
                    {imageEntries.length ? (
                      imageEntries.map((entry) => (
                        <SelectItem key={entry} value={entry} style={menuItemStyle}>
                          <MenuRow label={entry} selected={images.includes(entry)} />
                        </SelectItem>
                      ))
                    ) : (
                      <SelectItem value="__none" disabled style={menuItemStyle}>
                        <MenuRow
                          label="工作区未找到图片文件"
                          description="支持 .png/.jpg/.webp/.gif"
                          selected={false}
                        />
                      </SelectItem>
                    )}
                  </MenuSurface>
                </SelectContent>
              </div>
            </Select>
          ) : null}

          <ChipSelect
            testId="mode-select"
            value={store.mode ?? 'code'}
            onChange={(next) => store.setMode(next as AgentMode)}
            items={MODE_OPTIONS}
            icon={modeOption.icon}
            label={modeOption.label}
            menuWidth={250}
          >
            {MODE_OPTIONS.map((m) => (
              <SelectItem key={m.value} testId={`mode-option-${m.value}`} value={m.value} style={menuItemStyle}>
                <MenuRow
                  label={m.label}
                  description={m.desc}
                  selected={(store.mode ?? 'code') === m.value}
                />
              </SelectItem>
            ))}
          </ChipSelect>

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
      <ComposerTelemetryBar store={store} centered={centered} />
    </div>
  )
}
