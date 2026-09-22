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
import { computeThreadStats, type Item, type Thread } from '../agent/types'

/** 获取指定模型的上下文窗口 Token 上限（优先使用用户在设置中配置的上限，其次使用预设，默认 128k） */
export function getModelContextWindow(modelName?: string, configuredLimit?: number): number {
  if (configuredLimit && configuredLimit > 0) {
    return configuredLimit
  }
  if (!modelName) return 128_000
  const m = modelName.toLowerCase()
  if (m.includes('gemini') || m.includes('qwen-long')) return 1_000_000
  if (m.includes('claude')) return 200_000
  if (m.includes('32k')) return 32_000
  if (m.includes('16k')) return 16_000
  if (m.includes('8k')) return 8_000
  if (m.includes('64k')) return 64_000
  if (m.includes('200k')) return 200_000
  if (m.includes('128k')) return 128_000
  return 128_000
}

export interface ThreadTelemetry {
  turns: number
  steps: number
  tokPerSec: number
  totalTokens: number
  totalPromptTokens: number
  totalCompletionTokens: number
  totalCachedTokens: number
  cacheHitRatio: number
  currentContextTokens: number
  contextLimit: number
  contextRatio: number
}

/** 实时计算当前会话的遥测指标（轮数、步数、生成速率、Token消耗、缓存命中率、会话上下文占用率） */
export function computeThreadTelemetry(
  thread: Thread,
  isRunning: boolean,
  currentModel?: string,
  configuredLimit?: number,
): ThreadTelemetry {
  const stats = computeThreadStats(thread)
  const userItems = thread.items.filter((it) => it.kind === 'user')
  const toolItems = thread.items.filter((it) => it.kind === 'tool')
  const assistantItems = thread.items.filter(
    (it): it is Extract<Item, { kind: 'assistant' }> => it.kind === 'assistant',
  )

  const turns = userItems.length
  const steps = toolItems.length

  // 流式运行中的助手消息
  const streamingAssistant = assistantItems.find((it) => it.streaming)

  // 累计 Token：仅使用模型返回的 Token 计数，不进行前端估算
  const streamingCompletion = streamingAssistant?.usage?.completionTokens ?? 0
  const streamingPrompt = streamingAssistant?.usage?.promptTokens ?? 0
  const streamingCached = streamingAssistant?.usage?.cachedTokens ?? 0
  const streamingTotal =
    streamingAssistant?.usage?.totalTokens ?? (streamingPrompt + streamingCompletion)

  const totalPromptTokens = stats.totalPromptTokens + streamingPrompt
  const totalCompletionTokens = stats.totalCompletionTokens + streamingCompletion
  const totalTokens = stats.totalTokens + (streamingAssistant ? streamingTotal : 0)
  const totalCachedTokens = stats.totalCachedTokens + streamingCached

  // 缓存命中率
  const cacheHitRatio =
    totalPromptTokens > 0 ? Math.min(100, Math.round((totalCachedTokens / totalPromptTokens) * 100)) : 0

  // 计算 tok/s 生成速率（基于模型返回的补全 Token 与耗时）
  let tokPerSec = 0
  if (streamingAssistant) {
    if (streamingAssistant.usage?.completionTokens) {
      const elapsedSec = Math.max(0.3, (Date.now() - (streamingAssistant.at || Date.now())) / 1000)
      tokPerSec = Math.round(streamingAssistant.usage.completionTokens / elapsedSec)
    }
  } else {
    // 获取最近一个已完成且有模型 Token 统计的助手回复
    for (let i = assistantItems.length - 1; i >= 0; i--) {
      const a = assistantItems[i]
      if (a.durationMs && a.durationMs > 0 && a.usage?.completionTokens) {
        tokPerSec = Math.round(a.usage.completionTokens / (a.durationMs / 1000))
        break
      }
    }
    // 若单轮无法算出，尝试用总耗时和总补全 Token
    if (tokPerSec === 0 && stats.totalDurationMs > 0 && stats.totalCompletionTokens > 0) {
      tokPerSec = Math.round(stats.totalCompletionTokens / (stats.totalDurationMs / 1000))
    }
  }

  // 当前会话上下文占用与比率：直接使用模型返回的 promptTokens（取最近一轮助手的模型实际输入消耗）
  let currentContextTokens = 0
  for (let i = assistantItems.length - 1; i >= 0; i--) {
    if (assistantItems[i].usage?.promptTokens) {
      currentContextTokens = assistantItems[i].usage!.promptTokens
      break
    }
  }
  const contextLimit = getModelContextWindow(currentModel, configuredLimit)
  const contextRatio =
    contextLimit > 0 ? Math.min(100, Math.round((currentContextTokens / contextLimit) * 100)) : 0

  return {
    turns,
    steps,
    tokPerSec,
    totalTokens,
    totalPromptTokens,
    totalCompletionTokens,
    totalCachedTokens,
    cacheHitRatio,
    currentContextTokens,
    contextLimit,
    contextRatio,
  }
}

export function ComposerTelemetryBar({
  store,
  centered,
}: {
  store: AgentStore
  centered?: boolean
}) {
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
    totalPromptTokens,
    totalCompletionTokens,
    totalCachedTokens,
    cacheHitRatio,
    currentContextTokens,
    contextLimit,
    contextRatio,
  } = telemetry

  const contextColor = contextRatio >= 85 ? C.accent : C.secondary

  return (
    <div
      testId="composer-telemetry"
      style={{
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
        gap: 16,
        rowGap: 4,
        userSelect: 'none',
      }}
    >
      {/* 轮数与步数、每秒 Token */}
      <div
        testId="telemetry-turns-steps"
        aria-label={`已进行 ${turns} 轮对话，执行 ${steps} 步操作，生成速率 ${tokPerSec} tok/s`}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 5,
          cursor: 'default',
        }}
      >
        <Icon name="gauge" size={12} color={C.tertiary} />
        <text style={{ fontSize: 11.5, color: C.secondary, whiteSpace: 'nowrap' }}>
          {`${turns} 轮 ${steps} 步 · ${tokPerSec} tok/s`}
        </text>
      </div>

      {/* Token 统计与缓存命中比率 */}
      <div
        testId="telemetry-tokens-cache"
        aria-label={`累计消耗 ${formatNumber(totalTokens)} Token (输入: ${formatNumber(totalPromptTokens)} · 输出: ${formatNumber(totalCompletionTokens)})，读取缓存 ${formatNumber(totalCachedTokens)} Token (命中率 ${cacheHitRatio}%)`}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 5,
          cursor: 'default',
        }}
      >
        <Icon name="database" size={12} color={C.tertiary} />
        <text style={{ fontSize: 11.5, color: C.secondary, whiteSpace: 'nowrap' }}>
          {`${formatTokenShort(totalTokens)} tok · 缓存命中 ${cacheHitRatio}%`}
        </text>
      </div>

      {/* 当前会话比率 (上下文窗口占用率) */}
      <div
        testId="telemetry-context-ratio"
        aria-label={`当前上下文占用 ${formatNumber(currentContextTokens)} / ${formatNumber(contextLimit)} (${contextRatio}%)`}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 4,
          cursor: 'default',
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
          {`${contextRatio}%`}
        </text>
      </div>
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
            backgroundColor: C.card,
            borderWidth: 1,
            borderColor: C.cardBorder,
            borderRadius: 14,
            paddingLeft: 16,
            paddingRight: 16,
            paddingTop: 12,
            paddingBottom: 12,
            gap: 12,
          }}
        >
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 12,
              minWidth: 0,
              flexGrow: 1,
            }}
          >
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                width: 32,
                height: 32,
                borderRadius: 8,
                backgroundColor: C.overlay,
                flexShrink: 0,
              }}
            >
              <Icon name="bot" size={18} color={C.link} />
            </div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 2, minWidth: 0 }}>
              <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 8 }}>
                <text style={{ fontSize: 13, fontWeight: 600, color: C.text }}>
                  子智能体专属执行会话
                </text>
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    gap: 4,
                    paddingLeft: 6,
                    paddingRight: 6,
                    height: 18,
                    borderRadius: 4,
                    backgroundColor: isRunning ? C.chip : C.overlay,
                  }}
                >
                  <Icon
                    name={isRunning ? 'dot' : 'circleCheck'}
                    size={isRunning ? 6 : 10}
                    color={isRunning ? C.success : C.faint}
                  />
                  <text
                    style={{
                      fontSize: 10.5,
                      lineHeight: 14,
                      color: isRunning ? C.text : C.faint,
                    }}
                  >
                    {isRunning ? '正在异步运行中' : '执行完毕'}
                  </text>
                </div>
              </div>
              <text
                style={{
                  fontSize: 11.5,
                  color: C.secondary,
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
