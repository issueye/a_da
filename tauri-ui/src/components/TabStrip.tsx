import React, { useRef } from 'react'
import { Plus, X, MessageSquare, Bot } from 'lucide-react'
import type { Thread } from '../types'

export interface TabStripProps {
  threads: Thread[]
  openTabIds: string[]
  activeThreadId: string
  runningThreadIds?: string[]
  isRunning?: boolean
  onSelectTab: (threadId: string) => void
  onCloseTab: (threadId: string) => void
  onNewTab: () => void
}

/**
 * 会话多标签页栏组件 (TabStrip)
 * 摆在主内容区顶部，支持多会话快速切换、关闭标签与快捷新建。
 */
export const TabStrip: React.FC<TabStripProps> = ({
  threads,
  openTabIds,
  activeThreadId,
  runningThreadIds = [],
  isRunning = false,
  onSelectTab,
  onCloseTab,
  onNewTab,
}) => {
  const scrollRef = useRef<HTMLDivElement>(null)

  // 根据 openTabIds 提取对应的 Thread 对象（若有不存在的 id 则兜底过滤）
  const threadMap = new Map(threads.map((t) => [t.id, t]))
  const openTabs: Thread[] = []

  for (const id of openTabIds) {
    const t = threadMap.get(id)
    if (t) {
      openTabs.push(t)
    }
  }

  // 如果 openTabs 为空但 activeThreadId 存在，至少显示当前激活的会话
  if (openTabs.length === 0 && activeThreadId) {
    const active = threadMap.get(activeThreadId)
    if (active) openTabs.push(active)
  }

  const isThreadRunning = (threadId: string) => {
    if (runningThreadIds.includes(threadId)) return true
    if (threadId === activeThreadId && isRunning) return true
    return false
  }

  return (
    <div className="flex items-center h-9 px-2 bg-zinc-100/80 dark:bg-[#141416] border-b border-zinc-200 dark:border-[#27272a] select-none flex-shrink-0 gap-1 overflow-hidden transition-colors">
      {/* 横向滚动的标签项列表 */}
      <div
        ref={scrollRef}
        className="flex items-center space-x-1 flex-1 min-w-0 overflow-x-auto scrollbar-none py-1"
      >
        {openTabs.map((thread) => {
          const isSelected = thread.id === activeThreadId
          const running = isThreadRunning(thread.id)
          const isSubagent = Boolean(thread.isSubagent)
          const closable = openTabs.length > 1

          return (
            <div
              key={thread.id}
              onClick={() => onSelectTab(thread.id)}
              className={`group flex items-center justify-between h-7 px-2.5 rounded-lg text-xs cursor-pointer transition-all min-w-[120px] max-w-[180px] flex-shrink-0 border ${
                isSelected
                  ? 'bg-white dark:bg-[#1e1e24] text-zinc-900 dark:text-zinc-100 border-zinc-200 dark:border-zinc-700/80 font-medium shadow-xs'
                  : 'bg-transparent text-zinc-600 dark:text-zinc-400 border-transparent hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 hover:text-zinc-900 dark:hover:text-zinc-200'
              }`}
              title={thread.title || (isSubagent ? '子代理会话' : '新对话')}
            >
              {/* 左侧：图标与标题 */}
              <div className="flex items-center space-x-1.5 min-w-0 flex-1 mr-1">
                {running ? (
                  <span className="relative flex h-2 w-2 flex-shrink-0">
                    <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-blue-400 opacity-75" />
                    <span className="relative inline-flex rounded-full h-2 w-2 bg-blue-500" />
                  </span>
                ) : isSubagent ? (
                  <Bot
                    size={13}
                    className={
                      isSelected
                        ? 'text-purple-600 dark:text-purple-400 flex-shrink-0'
                        : 'text-purple-400 dark:text-purple-500 flex-shrink-0'
                    }
                  />
                ) : (
                  <MessageSquare
                    size={12}
                    className={
                      isSelected
                        ? 'text-blue-500 flex-shrink-0'
                        : 'text-zinc-400 dark:text-zinc-500 flex-shrink-0'
                    }
                  />
                )}

                <span className="truncate text-xs">
                  {thread.title || (isSubagent ? '子代理会话' : '新对话')}
                </span>
              </div>

              {/* 右侧：关闭标签按钮 */}
              {closable && (
                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation()
                    onCloseTab(thread.id)
                  }}
                  className="opacity-0 group-hover:opacity-100 p-0.5 rounded text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-opacity flex-shrink-0 cursor-pointer"
                  title="关闭标签页"
                >
                  <X size={11} />
                </button>
              )}
            </div>
          )
        })}
      </div>

      {/* 新建标签按钮 */}
      <button
        type="button"
        onClick={onNewTab}
        className="flex items-center justify-center w-7 h-7 rounded-lg text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800/80 transition-colors flex-shrink-0 cursor-pointer"
        title="新建对话标签页"
      >
        <Plus size={14} />
      </button>
    </div>
  )
}
