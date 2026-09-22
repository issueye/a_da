/**
 * The event log behind the composer's 调试 chip.
 *
 * It shows what actually crossed the wire: the endpoint, requests with full payloads,
 * responses with thoughts & tool calls, each tool execution, and every error, newest first.
 */

import React, { useState } from 'react'
import type { AgentStore } from '../agent/store'
import type { DebugEntry } from '../agent/types'
import { Icon, IconButton } from './controls'
import { C, FONT_MONO, M } from '../theme'
import { copyToClipboard } from '../platform/clipboard'
import { formatNumber, formatDuration } from './Transcript'

const KIND_CONFIG: Record<
  DebugEntry['kind'],
  { label: string; color: string; bg: string }
> = {
  request: { label: '请求', color: C.accent, bg: C.chip },
  response: { label: '响应', color: '#10b981', bg: 'rgba(16, 185, 129, 0.12)' },
  tools: { label: '调用', color: '#8b5cf6', bg: 'rgba(139, 92, 246, 0.12)' },
  tool: { label: '结果', color: '#f59e0b', bg: 'rgba(245, 158, 11, 0.12)' },
  error: { label: '错误', color: C.danger, bg: 'rgba(239, 68, 68, 0.12)' },
  info: { label: '信息', color: C.tertiary, bg: C.overlay },
  delta: { label: '增量', color: C.faint, bg: C.overlay },
}

function clock(at: number): string {
  const date = new Date(at)
  const pad = (value: number) => `${value}`.padStart(2, '0')
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

function DebugEntryItem({ entry }: { entry: DebugEntry }) {
  const [expanded, setExpanded] = useState(false)
  const [copied, setCopied] = useState(false)
  const [tab, setTab] = useState<'content' | 'raw'>('content')

  const config = KIND_CONFIG[entry.kind] ?? KIND_CONFIG.info
  const isError = entry.kind === 'error'

  const formattedContent =
    entry.raw ??
    (entry.payload !== undefined ? JSON.stringify(entry.payload, null, 2) : null)

  const hasDetail = !!formattedContent

  // 针对结构化响应提取可读内容
  const payload = entry.payload as any
  const responseContent = payload?.content as string | undefined
  const responseThinking = payload?.thinking as string | undefined
  const responseToolCalls = payload?.toolCalls as any[] | undefined
  const responseUsage = payload?.usage as any

  const hasStructuredResponse =
    entry.kind === 'response' &&
    (responseContent || responseThinking || responseToolCalls?.length || responseUsage)

  // 针对结构化请求提取可读内容
  const requestMessages = payload?.messages as any[] | undefined
  const hasStructuredRequest = entry.kind === 'request' && requestMessages?.length

  const handleCopy = (text: string, e?: any) => {
    e?.stopPropagation?.()
    if (!text) return
    void copyToClipboard(text).then((ok) => {
      if (ok) {
        setCopied(true)
        setTimeout(() => setCopied(false), 1500)
      }
    })
  }

  return (
    <div
      testId={`debug-entry-${entry.id}`}
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 5,
        padding: 8,
        borderRadius: 6,
        backgroundColor: expanded ? C.overlay : 'transparent',
        borderWidth: expanded ? 1 : 0,
        borderColor: C.border,
      }}
    >
      {/* 头部摘要行 */}
      <div
        role="button"
        testId={`debug-entry-header-${entry.id}`}
        onClick={() => hasDetail && setExpanded(!expanded)}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'flex-start',
          gap: 6,
          cursor: hasDetail ? 'pointer' : 'default',
        }}
      >
        <text style={{ fontFamily: FONT_MONO, fontSize: 10, lineHeight: 16, color: C.faint, flexShrink: 0 }}>
          {clock(entry.at)}
        </text>

        {/* 类型标签徽章 */}
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            paddingLeft: 5,
            paddingRight: 5,
            paddingTop: 1,
            paddingBottom: 1,
            borderRadius: 4,
            backgroundColor: config.bg,
            flexShrink: 0,
          }}
        >
          <text
            style={{
              fontSize: 10,
              lineHeight: 13,
              fontWeight: 600,
              color: config.color,
            }}
          >
            {config.label}
          </text>
        </div>

        {/* 摘要文本 */}
        <div style={{ display: 'flex', flexDirection: 'column', flexGrow: 1, minWidth: 0 }}>
          <text
            style={{
              fontSize: 11.5,
              lineHeight: 16,
              fontWeight: expanded ? 600 : 400,
              color: isError ? C.danger : C.text,
            }}
          >
            {entry.text}
          </text>
        </div>

        {/* 展开/收起箭头 */}
        {hasDetail ? (
          <div
            testId={`debug-expand-${entry.id}`}
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              width: 18,
              height: 18,
              borderRadius: 3,
              flexShrink: 0,
              opacity: 0.7,
              hover: { opacity: 1 },
            }}
          >
            <Icon name={expanded ? 'chevronDown' : 'chevronRight'} size={12} color={C.secondary} />
          </div>
        ) : null}
      </div>

      {/* 展开的详情面板（请求载荷/响应结果） */}
      {expanded && formattedContent ? (
        <div
          testId={`debug-detail-${entry.id}`}
          style={{
            display: 'flex',
            flexDirection: 'column',
            marginTop: 4,
            gap: 6,
          }}
        >
          {/* 操作栏与视图切换 */}
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              justifyContent: 'space-between',
              paddingTop: 2,
              paddingBottom: 2,
            }}
          >
            {/* Tab 切换：排版内容 / 原始 JSON */}
            {(hasStructuredResponse || hasStructuredRequest) ? (
              <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                <div
                  role="button"
                  onClick={(e: any) => {
                    e?.stopPropagation?.()
                    setTab('content')
                  }}
                  style={{
                    paddingLeft: 6,
                    paddingRight: 6,
                    paddingTop: 1,
                    paddingBottom: 1,
                    borderRadius: 3,
                    cursor: 'pointer',
                    backgroundColor: tab === 'content' ? C.chip : 'transparent',
                  }}
                >
                  <text
                    style={{
                      fontSize: 10.5,
                      fontWeight: tab === 'content' ? 600 : 400,
                      color: tab === 'content' ? C.text : C.tertiary,
                    }}
                  >
                    {entry.kind === 'response' ? '排版回复' : '消息结构'}
                  </text>
                </div>

                <div
                  role="button"
                  onClick={(e: any) => {
                    e?.stopPropagation?.()
                    setTab('raw')
                  }}
                  style={{
                    paddingLeft: 6,
                    paddingRight: 6,
                    paddingTop: 1,
                    paddingBottom: 1,
                    borderRadius: 3,
                    cursor: 'pointer',
                    backgroundColor: tab === 'raw' ? C.chip : 'transparent',
                  }}
                >
                  <text
                    style={{
                      fontSize: 10.5,
                      fontWeight: tab === 'raw' ? 600 : 400,
                      color: tab === 'raw' ? C.text : C.tertiary,
                    }}
                  >
                    原始 JSON
                  </text>
                </div>
              </div>
            ) : (
              <text style={{ fontSize: 10.5, color: C.tertiary }}>载荷数据</text>
            )}

            {/* 复制按钮 */}
            <div
              testId={`debug-copy-${entry.id}`}
              role="button"
              aria-label="复制完整数据"
              onClick={(e) => handleCopy(tab === 'content' && responseContent ? responseContent : formattedContent, e)}
              style={{
                display: 'flex',
                flexDirection: 'row',
                alignItems: 'center',
                gap: 4,
                paddingLeft: 7,
                paddingRight: 7,
                paddingTop: 2,
                paddingBottom: 2,
                borderRadius: 4,
                cursor: 'pointer',
                backgroundColor: C.chip,
                hover: { backgroundColor: C.chipHover },
              }}
            >
              <Icon name={copied ? 'check' : 'copy'} size={11} color={copied ? '#10b981' : C.secondary} />
              <text style={{ fontSize: 10.5, color: copied ? '#10b981' : C.text }}>
                {copied ? '已复制' : tab === 'content' && responseContent ? '复制正文' : '复制 JSON'}
              </text>
            </div>
          </div>

          {/* 1. 结构化响应排版视图：真实换行，从第一行到最后一行完整可读 */}
          {/* 1. 结构化响应排版视图：真实换行，从第一行到最后一行完整可读 */}
          {tab === 'content' && hasStructuredResponse ? (
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                gap: 8,
                padding: 10,
                borderRadius: 6,
                backgroundColor: C.raised,
                borderWidth: 1,
                borderColor: C.borderStrong,
              }}
            >
              {/* 回复正文（原汁原味真实换行排版） */}
              {responseContent ? (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                  <text style={{ fontSize: 10.5, fontWeight: 600, color: C.accent }}>
                    【回复正文完整内容】
                  </text>
                  <text
                    style={{
                      fontSize: 11,
                      lineHeight: 17,
                      color: C.text,
                    }}
                  >
                    {responseContent}
                  </text>
                </div>
              ) : null}

              {/* 思考过程（思维链） */}
              {responseThinking ? (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 4, paddingTop: 4, borderTopWidth: 1, borderColor: C.border }}>
                  <text style={{ fontSize: 10.5, fontWeight: 600, color: '#8b5cf6' }}>
                    【思考过程 Thinking】
                  </text>
                  <text style={{ fontSize: 11, lineHeight: 16, color: C.secondary }}>
                    {responseThinking}
                  </text>
                </div>
              ) : null}

              {/* 工具调用 */}
              {responseToolCalls && responseToolCalls.length > 0 ? (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 4, paddingTop: 4, borderTopWidth: 1, borderColor: C.border }}>
                  <text style={{ fontSize: 10.5, fontWeight: 600, color: '#f59e0b' }}>
                    【工具调用 Tool Calls】
                  </text>
                  {responseToolCalls.map((call, idx) => (
                    <div key={idx} style={{ display: 'flex', flexDirection: 'column', padding: 6, backgroundColor: C.overlay, borderRadius: 4 }}>
                      <text style={{ fontFamily: FONT_MONO, fontSize: 11, fontWeight: 600, color: C.text }}>
                        {call.name}
                      </text>
                      <text style={{ fontFamily: FONT_MONO, fontSize: 10, color: C.secondary }}>
                        {call.rawArguments || JSON.stringify(call.arguments)}
                      </text>
                    </div>
                  ))}
                </div>
              ) : null}

              {/* Token 与耗时详情 */}
              {responseUsage ? (
                <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', gap: 10, paddingTop: 6, borderTopWidth: 1, borderColor: C.border }}>
                  <text style={{ fontSize: 10.5, color: C.tertiary }}>
                    {`总 Token: ${formatNumber(responseUsage.totalTokens ?? 0)} (提示词: ${formatNumber(responseUsage.promptTokens ?? 0)} · 输出: ${formatNumber(responseUsage.completionTokens ?? 0)}${responseUsage.cachedTokens ? ` · 缓存: ${formatNumber(responseUsage.cachedTokens)}` : ''})`}
                  </text>
                  {payload?.durationMs ? (
                    <text style={{ fontSize: 10.5, color: C.tertiary }}>
                      {`耗时: ${formatDuration(payload.durationMs)}`}
                    </text>
                  ) : null}
                </div>
              ) : null}

              {/* 底部收起条 */}
              <div
                role="button"
                onClick={(e: any) => {
                  e?.stopPropagation?.()
                  setExpanded(false)
                }}
                style={{
                  alignSelf: 'center',
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 4,
                  paddingLeft: 8,
                  paddingRight: 8,
                  paddingTop: 3,
                  paddingBottom: 3,
                  borderRadius: 4,
                  cursor: 'pointer',
                  backgroundColor: C.chip,
                  marginTop: 2,
                }}
              >
                <Icon name="chevronUp" size={11} color={C.tertiary} />
                <text style={{ fontSize: 10, color: C.tertiary }}>收起详情</text>
              </div>
            </div>
          ) : tab === 'content' && hasStructuredRequest ? (
            /* 2. 结构化请求排版视图：按消息角色展示完整发送内容 */
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                gap: 8,
                padding: 10,
                borderRadius: 6,
                backgroundColor: C.raised,
                borderWidth: 1,
                borderColor: C.borderStrong,
              }}
            >
              <text style={{ fontSize: 10.5, fontWeight: 600, color: C.accent }}>
                {`【请求消息列表 · 共 ${requestMessages?.length} 条】`}
              </text>
              {requestMessages?.map((msg, idx) => (
                <div key={idx} style={{ display: 'flex', flexDirection: 'column', gap: 3, padding: 6, backgroundColor: C.overlay, borderRadius: 4 }}>
                  <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                    <text style={{ fontSize: 10, fontWeight: 600, color: msg.role === 'system' ? '#8b5cf6' : msg.role === 'user' ? C.accent : '#10b981' }}>
                      {msg.role.toUpperCase()}
                    </text>
                  </div>
                  <text style={{ fontSize: 10.5, lineHeight: 15, color: C.text }}>
                    {typeof msg.content === 'string' ? msg.content : JSON.stringify(msg.content)}
                  </text>
                </div>
              ))}

              {/* 底部收起条 */}
              <div
                role="button"
                onClick={(e: any) => {
                  e?.stopPropagation?.()
                  setExpanded(false)
                }}
                style={{
                  alignSelf: 'center',
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 4,
                  paddingLeft: 8,
                  paddingRight: 8,
                  paddingTop: 3,
                  paddingBottom: 3,
                  borderRadius: 4,
                  cursor: 'pointer',
                  backgroundColor: C.chip,
                  marginTop: 2,
                }}
              >
                <Icon name="chevronUp" size={11} color={C.tertiary} />
                <text style={{ fontSize: 10, color: C.tertiary }}>收起详情</text>
              </div>
            </div>
          ) : (
            /* 3. 原始未加工 JSON 代码框展示 */
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                gap: 6,
                padding: 8,
                borderRadius: 6,
                backgroundColor: C.raised,
                borderWidth: 1,
                borderColor: C.borderStrong,
              }}
            >
              <text
                style={{
                  fontFamily: FONT_MONO,
                  fontSize: 10.5,
                  lineHeight: 15,
                  color: C.secondary,
                }}
              >
                {formattedContent}
              </text>

              {/* 底部收起条 */}
              <div
                role="button"
                onClick={(e: any) => {
                  e?.stopPropagation?.()
                  setExpanded(false)
                }}
                style={{
                  alignSelf: 'center',
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 4,
                  paddingLeft: 8,
                  paddingRight: 8,
                  paddingTop: 3,
                  paddingBottom: 3,
                  borderRadius: 4,
                  cursor: 'pointer',
                  backgroundColor: C.chip,
                  marginTop: 2,
                }}
              >
                <Icon name="chevronUp" size={11} color={C.tertiary} />
                <text style={{ fontSize: 10, color: C.tertiary }}>收起详情</text>
              </div>
            </div>
          )}
        </div>
      ) : null}
    </div>
  )
}

export function DebugPanel({ store }: { store: AgentStore }) {
  const entries = [...store.log].reverse()

  return (
    <div
      testId="debug-panel"
      style={{
        display: 'flex',
        flexDirection: 'column',
        width: 420,
        height: '100%',
        flexShrink: 0,
        backgroundColor: C.sidebar,
        borderLeftWidth: 1,
        borderColor: C.sidebarBorder,
      }}
    >
      {/* 顶部标题栏 */}
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

        {/* 清空日志按钮 */}
        {entries.length > 0 ? (
          <IconButton
            icon="trash"
            testId="debug-clear"
            label="清空事件日志"
            onClick={() => store.clearLog()}
          />
        ) : null}

        {/* 关闭面板按钮 */}
        <IconButton
          icon="close"
          testId="debug-close"
          label="关闭日志"
          onClick={() => store.toggleDebug()}
        />
      </div>

      {/* 日志条目列表 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          flexGrow: 1,
          minHeight: 0,
          overflowY: 'scroll',
          padding: 10,
          gap: 6,
        }}
      >
        {entries.length === 0 ? (
          <text style={{ fontSize: 11.5, lineHeight: 16, color: C.faint }}>
            还没有事件。发送任务后，模型请求与返回内容、工具结果和错误都会详细记在这里。
          </text>
        ) : null}

        {entries.map((entry) => (
          <DebugEntryItem key={entry.id} entry={entry} />
        ))}
      </div>
    </div>
  )
}
