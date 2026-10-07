import React, { useState } from 'react'
import { Clock, Zap, Edit3, X, Trash2, ChevronDown, ChevronUp, Image as ImageIcon } from 'lucide-react'
import type { QueuedItem } from '../types'

export interface QueuedMessagesFloatingPanelProps {
  queue: QueuedItem[]
  onPromote: (index: number) => void
  onEditItem?: (text: string, images?: string[]) => void
  onRemove: (index: number) => void
  onClear: () => void
}

/**
 * 会话排队发送浮动队列面板 (QueuedMessagesFloatingPanel)
 * 置顶在 Composer 输入框上方，展示排队等待发出的指令，支持插队、编辑与移除。
 */
export const QueuedMessagesFloatingPanel: React.FC<QueuedMessagesFloatingPanelProps> = ({
  queue,
  onPromote,
  onEditItem,
  onRemove,
  onClear,
}) => {
  const [collapsed, setCollapsed] = useState(false)
  const [showClearConfirm, setShowClearConfirm] = useState(false)

  if (!queue || queue.length === 0) return null

  return (
    <div className="w-full mb-1.5 p-2 bg-white/95 dark:bg-[#1a1a1e]/95 border border-blue-200/80 dark:border-blue-900/40 rounded-xl shadow-md backdrop-blur-md flex flex-col space-y-1.5 select-none animate-in fade-in slide-in-from-bottom-1 duration-150">
      {/* 顶部标题栏 */}
      <div className="flex items-center justify-between px-1">
        <div className="flex items-center space-x-2 min-w-0">
          <Clock size={13} className="text-blue-500 flex-shrink-0" />
          <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100 truncate">
            排队发送队列
          </span>
          <span className="px-1.5 py-0.2 rounded-full text-[10px] font-mono font-medium bg-blue-100/80 dark:bg-blue-950/60 text-blue-700 dark:text-blue-300">
            {queue.length} 条待发送
          </span>
          <span className="hidden sm:inline text-[11px] text-zinc-400 dark:text-zinc-500 truncate">
            (当前任务完成后按序发送，也可点击立即发送插队)
          </span>
        </div>

        <div className="flex items-center space-x-1 flex-shrink-0">
          {queue.length > 1 && (
            <>
              {showClearConfirm ? (
                <div className="flex items-center space-x-1 bg-rose-50 dark:bg-rose-950/40 px-1.5 py-0.5 rounded-md border border-rose-200 dark:border-rose-900/60 text-[10.5px]">
                  <span className="text-rose-600 dark:text-rose-400">确认清空全部?</span>
                  <button
                    onClick={() => {
                      onClear()
                      setShowClearConfirm(false)
                    }}
                    className="text-rose-700 dark:text-rose-300 font-semibold hover:underline cursor-pointer"
                  >
                    确定
                  </button>
                  <button
                    onClick={() => setShowClearConfirm(false)}
                    className="text-zinc-500 hover:text-zinc-700 dark:text-zinc-400 cursor-pointer ml-1"
                  >
                    取消
                  </button>
                </div>
              ) : (
                <button
                  type="button"
                  onClick={() => setShowClearConfirm(true)}
                  className="flex items-center space-x-1 px-1.5 py-0.5 text-[11px] text-zinc-500 dark:text-zinc-400 hover:text-rose-600 dark:hover:text-rose-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 rounded transition-colors cursor-pointer"
                  title="清空全部排队消息"
                >
                  <Trash2 size={11} />
                  <span>全部清空</span>
                </button>
              )}
            </>
          )}

          <button
            type="button"
            onClick={() => setCollapsed(!collapsed)}
            className="p-1 text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200 hover:bg-zinc-100 dark:hover:bg-zinc-800 rounded transition-colors cursor-pointer"
            title={collapsed ? '展开队列' : '收起队列'}
          >
            {collapsed ? <ChevronDown size={13} /> : <ChevronUp size={13} />}
          </button>
        </div>
      </div>

      {/* 排队条目列表 */}
      {!collapsed && (
        <div className="space-y-1 pt-0.5 max-h-48 overflow-y-auto pr-0.5">
          {queue.map((item, idx) => (
            <div
              key={item.id || idx}
              className="flex items-center justify-between p-1.5 rounded-lg bg-zinc-50 dark:bg-zinc-900/50 border border-zinc-200/60 dark:border-zinc-800 gap-2 text-xs"
            >
              {/* 左侧：序号与指令内容摘要 */}
              <div className="flex items-center space-x-2 min-w-0 flex-1">
                <span className="w-5 h-5 flex items-center justify-center rounded bg-zinc-200/70 dark:bg-zinc-800 text-[10px] font-mono text-zinc-500 dark:text-zinc-400 flex-shrink-0">
                  #{idx + 1}
                </span>

                <span className="text-zinc-800 dark:text-zinc-200 truncate flex-1 leading-snug">
                  {item.text || '(图片附件指令)'}
                </span>

                {item.images && item.images.length > 0 && (
                  <span className="flex items-center space-x-0.5 px-1.5 py-0.2 rounded text-[10px] bg-blue-50 dark:bg-blue-950/40 text-blue-600 dark:text-blue-400 border border-blue-200/50 dark:border-blue-900/50 flex-shrink-0">
                    <ImageIcon size={10} />
                    <span>{item.images.length} 图</span>
                  </span>
                )}
              </div>

              {/* 右侧：立即发送插队、取出编辑、移出队列 */}
              <div className="flex items-center space-x-1 flex-shrink-0">
                <button
                  type="button"
                  onClick={() => onPromote(idx)}
                  className="flex items-center space-x-1 px-2 py-0.5 rounded-md bg-blue-50 dark:bg-blue-950/50 hover:bg-blue-100 dark:hover:bg-blue-900/60 text-blue-600 dark:text-blue-400 border border-blue-300/60 dark:border-blue-700/50 text-[11px] font-medium transition-colors cursor-pointer shadow-2xs"
                  title="立即插队发送该消息"
                >
                  <Zap size={11} className="text-blue-500" />
                  <span>立即发送</span>
                </button>

                {onEditItem && (
                  <button
                    type="button"
                    onClick={() => {
                      onEditItem(item.text, item.images)
                      onRemove(idx)
                    }}
                    className="p-1 text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200/60 dark:hover:bg-zinc-800 rounded transition-colors cursor-pointer"
                    title="取出到输入框编辑"
                  >
                    <Edit3 size={12} />
                  </button>
                )}

                <button
                  type="button"
                  onClick={() => onRemove(idx)}
                  className="p-1 text-zinc-400 hover:text-rose-500 hover:bg-zinc-200/60 dark:hover:bg-zinc-800 rounded transition-colors cursor-pointer"
                  title="移出队列"
                >
                  <X size={12} />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
