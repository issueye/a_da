/**
 * 上下文深度洞察悬浮卡片 (Tauri UI 版本)
 * 参考 `src/ui/ContextUsagePopover.tsx` 与原版 GPUIX 设计
 * 提供上下文多段彩色进度条、各构成维度细分、执行遥测明细、缓存收益与健康度建议
 */

import React, { useEffect, useRef } from 'react'
import {
  Brain,
  X,
  Zap,
  Shield,
  AlertTriangle,
  Sparkles,
  Gauge,
  Database,
  Clock,
  Activity,
  Layers,
} from 'lucide-react'
import type { ContextUsageSummary } from '../types'
import { formatTokenShort } from './Transcript'

export interface TelemetryData {
  turns: number
  steps: number
  tokPerSec: number
  totalTokens: number
  promptTokens: number
  completionTokens: number
  cachedTokens: number
  cacheHitRatio: number
  durationMs: number
}

export interface ContextUsagePopoverProps {
  summary: ContextUsageSummary
  telemetry: TelemetryData
  onClose: () => void
  onCompact?: () => void
}

export const ContextUsagePopover: React.FC<ContextUsagePopoverProps> = ({
  summary,
  telemetry,
  onClose,
  onCompact,
}) => {
  const percentNum = Math.round(summary.percent * 100)
  const isHigh = percentNum > 75
  const isWarning = percentNum > 85
  const popoverRef = useRef<HTMLDivElement>(null)

  // 点击外部关闭弹窗
  useEffect(() => {
    const handleMouseDown = (e: MouseEvent) => {
      if (popoverRef.current && !popoverRef.current.contains(e.target as Node)) {
        onClose()
      }
    }
    // 延迟添加监听器以避免触发当前的点击冒泡
    const timer = setTimeout(() => {
      document.addEventListener('mousedown', handleMouseDown)
    }, 0)
    return () => {
      clearTimeout(timer)
      document.removeEventListener('mousedown', handleMouseDown)
    }
  }, [onClose])

  return (
    <div
      ref={popoverRef}
      data-testid="context-usage-popover"
      className="absolute bottom-full right-0 mb-2 w-[360px] max-w-[90vw] bg-white dark:bg-[#1a1a1e] border border-zinc-200 dark:border-[#2d2d32] rounded-xl shadow-2xl p-3.5 flex flex-col space-y-2.5 z-50 select-none animate-in fade-in slide-in-from-bottom-2 duration-150 text-zinc-800 dark:text-zinc-200 text-xs"
      onClick={(e) => e.stopPropagation()}
    >
      {/* 1. 头部标题与关闭按钮 */}
      <div className="flex items-center justify-between">
        <div className="flex items-center space-x-2">
          <Brain
            size={14}
            className={isWarning ? 'text-rose-500' : isHigh ? 'text-amber-500' : 'text-blue-500 dark:text-blue-400'}
          />
          <span className="font-semibold text-zinc-900 dark:text-zinc-100 text-xs">
            上下文用量与健康度
          </span>
        </div>

        <div className="flex items-center space-x-2">
          <span className="font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
            {summary.formattedSummary}
          </span>
          <button
            type="button"
            onClick={onClose}
            className="p-1 rounded-md text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200 hover:bg-zinc-100 dark:hover:bg-zinc-800/80 transition-colors cursor-pointer"
            title="关闭浮窗"
          >
            <X size={12} />
          </button>
        </div>
      </div>

      {/* 2. 详细执行指标卡片（呈现移入浮窗的步数、速率、提示词、模型输出） */}
      <div className="grid grid-cols-4 gap-1.5 p-2 rounded-lg bg-zinc-50 dark:bg-zinc-800/50 border border-zinc-100 dark:border-zinc-800">
        <div className="flex flex-col items-center justify-center py-0.5">
          <span className="text-[10px] text-zinc-400 dark:text-zinc-500">执行步数</span>
          <span className="font-mono font-semibold text-zinc-700 dark:text-zinc-200 text-[11.5px] mt-0.5">
            {telemetry.steps} 步
          </span>
        </div>

        <div className="flex flex-col items-center justify-center py-0.5 border-l border-zinc-200/60 dark:border-zinc-700/60">
          <span className="text-[10px] text-zinc-400 dark:text-zinc-500">生成速率</span>
          <span className="font-mono font-semibold text-zinc-700 dark:text-zinc-200 text-[11.5px] mt-0.5">
            {telemetry.tokPerSec > 0 ? `${telemetry.tokPerSec} tok/s` : '-'}
          </span>
        </div>

        <div className="flex flex-col items-center justify-center py-0.5 border-l border-zinc-200/60 dark:border-zinc-700/60">
          <span className="text-[10px] text-zinc-400 dark:text-zinc-500">提示词</span>
          <span className="font-mono font-semibold text-zinc-700 dark:text-zinc-200 text-[11.5px] mt-0.5">
            {formatTokenShort(telemetry.promptTokens)}
          </span>
        </div>

        <div className="flex flex-col items-center justify-center py-0.5 border-l border-zinc-200/60 dark:border-zinc-700/60">
          <span className="text-[10px] text-zinc-400 dark:text-zinc-500">模型输出</span>
          <span className="font-mono font-semibold text-zinc-700 dark:text-zinc-200 text-[11.5px] mt-0.5">
            {formatTokenShort(telemetry.completionTokens)}
          </span>
        </div>
      </div>

      {/* 3. 多段式彩色进度条 */}
      <div className="w-full h-1.5 rounded-full bg-zinc-100 dark:bg-zinc-800 overflow-hidden flex flex-row">
        {summary.breakdown.map((item) => {
          const widthPercent = Math.max(1, Math.round(item.percent * 100))
          return (
            <div
              key={item.source}
              style={{
                width: `${widthPercent}%`,
                backgroundColor: item.color,
              }}
              title={`${item.label}: ${item.estimatedTokens.toLocaleString()} tok (${Math.round(item.percent * 100)}%)`}
              className="h-full transition-all duration-300"
            />
          )
        })}
      </div>

      {/* 4. 构成维度明细列表 */}
      <div className="flex flex-col space-y-1.5 py-0.5">
        {summary.breakdown.map((item) => (
          <div
            key={item.source}
            className="flex items-center justify-between text-[11.5px]"
          >
            <div className="flex items-center space-x-2 min-w-0">
              <div
                className="w-2 h-2 rounded-full flex-shrink-0"
                style={{ backgroundColor: item.color }}
              />
              <span className="text-zinc-600 dark:text-zinc-400 truncate">
                {item.label}
              </span>
            </div>

            <div className="flex items-center space-x-2 font-mono text-[11px] flex-shrink-0">
              <span className="text-zinc-700 dark:text-zinc-300">
                {item.estimatedTokens.toLocaleString()} tok
              </span>
              <span className="text-zinc-400 dark:text-zinc-500 w-8 text-right">
                {Math.round(item.percent * 100)}%
              </span>
            </div>
          </div>
        ))}
      </div>

      {/* 5. 缓存命中收益（若有命中） */}
      {summary.cachedTokens > 0 && (
        <div className="flex items-center justify-between pt-2 border-t border-zinc-100 dark:border-zinc-800 text-[11.5px]">
          <div className="flex items-center space-x-1.5 text-emerald-600 dark:text-emerald-400 font-medium">
            <Zap size={12} className="text-emerald-500" />
            <span>缓存命中收益</span>
          </div>
          <span className="font-mono text-[11px] text-emerald-600 dark:text-emerald-400">
            {summary.cachedTokens.toLocaleString()} tok ({Math.round((summary.cacheHitRate ?? 0) * 100)}%)
          </span>
        </div>
      )}

      {/* 6. 底部健康度提示与建议 */}
      <div className="flex items-center space-x-1.5 pt-2 border-t border-zinc-100 dark:border-zinc-800 text-[11px]">
        {isWarning ? (
          <AlertTriangle size={12} className="text-rose-500 flex-shrink-0" />
        ) : isHigh ? (
          <AlertTriangle size={12} className="text-amber-500 flex-shrink-0" />
        ) : (
          <Shield size={12} className="text-emerald-500 flex-shrink-0" />
        )}
        <span
          className={
            isWarning
              ? 'text-rose-500 font-medium'
              : isHigh
              ? 'text-amber-500'
              : 'text-zinc-500 dark:text-zinc-400'
          }
        >
          {isWarning
            ? '当前上下文占用已接近红线，建议立即压缩上下文以防超限'
            : isHigh
            ? '当前上下文占用较高，可按需执行压缩以提速降本'
            : `上下文容量充裕（剩余 ${100 - percentNum}% 空间）`}
        </span>
      </div>

      {/* 7. 一键压缩操作按钮 */}
      {onCompact && (
        <button
          type="button"
          onClick={() => {
            onClose()
            onCompact()
          }}
          className={`w-full mt-1 flex items-center justify-center space-x-1.5 py-1.5 px-3 rounded-lg text-xs font-semibold transition-all cursor-pointer shadow-xs ${
            isHigh
              ? 'bg-emerald-500/15 hover:bg-emerald-500/25 text-emerald-600 dark:text-emerald-400 border border-emerald-500/30'
              : 'bg-zinc-100 dark:bg-zinc-800 hover:bg-zinc-200 dark:hover:bg-zinc-700 text-zinc-700 dark:text-zinc-200 border border-zinc-200 dark:border-zinc-700'
          }`}
        >
          <Sparkles size={12} className="text-emerald-500" />
          <span>一键压缩上下文与生成摘要 (/compact)</span>
        </button>
      )}
    </div>
  )
}
