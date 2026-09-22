/**
 * 上下文深度洞察悬浮卡片
 * 参考 ZCode `ContextContent` 与 `ChatContextUsage`
 * 提供上下文多段进度条、各构成维度细分、缓存收益与健康度建议
 */

import React from 'react'
import type { ContextUsageSummary } from '../agent/stats/types'
import { C, FONT_MONO } from '../theme'
import { Icon } from './controls'

export function ContextUsagePopover({
  summary,
  onClose,
}: {
  summary: ContextUsageSummary
  onClose: () => void
}) {
  const percentNum = Math.round(summary.percent * 100)
  const isHigh = percentNum > 75
  const isWarning = percentNum > 85

  return (
    <div
      testId="context-usage-popover"
      style={{
        position: 'absolute',
        bottom: 34,
        right: 12,
        width: 320,
        backgroundColor: C.raised,
        borderWidth: 1,
        borderColor: C.borderStrong,
        borderRadius: 10,
        boxShadow: {
          offsetX: 0,
          offsetY: 8,
          blurRadius: 24,
          spreadRadius: 0,
          color: 'rgba(0, 0, 0, 0.28)',
        },
        padding: 12,
        display: 'flex',
        flexDirection: 'column',
        gap: 10,
      }}
      onClick={(e: any) => e?.stopPropagation?.()}
    >
      {/* 头部标题与关闭按钮 */}
      <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between' }}>
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <Icon name="brain" size={13} color={isWarning ? C.danger : isHigh ? '#f59e0b' : C.accent} />
          <text style={{ fontSize: 11.5, fontWeight: 600, color: C.text }}>
            上下文用量与健康度
          </text>
        </div>

        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <text style={{ fontFamily: FONT_MONO, fontSize: 10.5, color: C.secondary }}>
            {summary.formattedSummary}
          </text>
          <div
            role="button"
            onClick={onClose}
            style={{
              cursor: 'pointer',
              opacity: 0.6,
              hover: { opacity: 1 },
              padding: 2,
            }}
          >
            <Icon name="close" size={11} color={C.tertiary} />
          </div>
        </div>
      </div>

      {/* 多段式彩色进度条 */}
      <div
        style={{
          width: '100%',
          height: 6,
          borderRadius: 3,
          backgroundColor: C.overlay,
          display: 'flex',
          flexDirection: 'row',
          overflow: 'hidden',
        }}
      >
        {summary.breakdown.map((item) => {
          const widthPercent = Math.max(1, Math.round(item.percent * 100))
          return (
            <div
              key={item.source}
              style={{
                width: `${widthPercent}%`,
                height: '100%',
                backgroundColor: item.color,
              }}
            />
          )
        })}
      </div>

      {/* 构成明细 Breakdown 列表 */}
      <div style={{ display: 'flex', flexDirection: 'column', gap: 5, paddingTop: 2 }}>
        {summary.breakdown.map((item) => (
          <div
            key={item.source}
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              justifyContent: 'space-between',
              fontSize: 10.5,
            }}
          >
            <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
              <div
                style={{
                  width: 7,
                  height: 7,
                  borderRadius: 2,
                  backgroundColor: item.color,
                }}
              />
              <text style={{ color: C.text }}>{item.label}</text>
            </div>

            <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 8 }}>
              <text style={{ fontFamily: FONT_MONO, color: C.secondary }}>
                {`${item.estimatedTokens.toLocaleString()} tok`}
              </text>
              <text style={{ fontFamily: FONT_MONO, color: C.tertiary, width: 32, textAlign: 'right' }}>
                {`${Math.round(item.percent * 100)}%`}
              </text>
            </div>
          </div>
        ))}
      </div>

      {/* 缓存收益统计行 */}
      {summary.cachedTokens > 0 ? (
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            justifyContent: 'space-between',
            paddingTop: 6,
            borderTopWidth: 1,
            borderColor: C.border,
          }}
        >
          <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 4 }}>
            <text style={{ fontSize: 10.5, color: '#10b981', fontWeight: 500 }}>
              ⚡ 缓存命中收益
            </text>
          </div>
          <text style={{ fontFamily: FONT_MONO, fontSize: 10.5, color: '#10b981' }}>
            {`${summary.cachedTokens.toLocaleString()} tok (${Math.round((summary.cacheHitRate ?? 0) * 100)}%)`}
          </text>
        </div>
      ) : null}

      {/* 底部健康度提示与建议 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 5,
          paddingTop: 6,
          borderTopWidth: 1,
          borderColor: C.border,
        }}
      >
        <Icon
          name={isWarning ? 'alertTriangle' : isHigh ? 'alertTriangle' : 'shield'}
          size={11}
          color={isWarning ? C.danger : isHigh ? '#f59e0b' : '#10b981'}
        />
        <text style={{ fontSize: 10, color: isWarning ? C.danger : isHigh ? '#f59e0b' : C.tertiary }}>
          {isWarning
            ? '当前上下文已接近上限，建议新开会话以保障模型推理质量'
            : isHigh
              ? '当前上下文占用较高，可按需精简历史或通过新会话整理'
              : `上下文容量充裕（剩余 ${(100 - percentNum)}% 空间）`}
        </text>
      </div>
    </div>
  )
}
