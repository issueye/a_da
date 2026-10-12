import React, { useRef, useEffect } from 'react'
import { Plus, X, MessageSquare, Bot, CheckCircle2, XCircle, Clock, Circle, Loader2 } from 'lucide-react'
import type { Thread } from '../types'
import { resolveThreadStatus } from '../utils/thread-status'

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

  // 根据 openTabIds 提取对应的 Thread 对象（取不到的 id 直接跳过，见下方说明）
  const threadMap = new Map(threads.map((t) => [t.id, t]))
  const openTabs: Thread[] = []

  for (const id of openTabIds) {
    const t = threadMap.get(id)
    if (t) {
      openTabs.push(t)
    }
    // 🔴 取不到会话对象就**不画页签**。
    //
    // 早先这里会兜底造一个 `title:'子智能体会话...'`、`isSubagent:true` 的假页签，于是任何
    // "在 `threads` 里找不到的 id"都会在界面上凭空长成一个"子智能体"页签——实测到的来源有：
    // 乐观创建期被原地改名的 `thread_tmp_*`、点开子卡片时带过来的失效 id、以及会话消失后
    // 残留的 id。子会话本来就是 `threads` 的一员（侧边栏父子树、`resolveThreadStatus` 都靠它），
    // 真的在的话这里必然取得到；取不到就是没有东西可显示，不该编。
  }

  // 如果 openTabs 为空但 activeThreadId 存在，至少显示当前激活的会话
  if (openTabs.length === 0 && activeThreadId) {
    const active = threadMap.get(activeThreadId)
    if (active) openTabs.push(active)
  }

  // 激活会话变动时，平滑滚动至当前激活的标签页
  useEffect(() => {
    if (scrollRef.current && activeThreadId) {
      const activeEl = scrollRef.current.querySelector<HTMLElement>(`[data-tab-id="${activeThreadId}"]`)
      if (activeEl) {
        activeEl.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'nearest' })
      }
    }
  }, [activeThreadId, openTabs.length])

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
          const isSubagent = Boolean(
            thread.isSubagent ||
            thread.parentId ||
            thread.title?.includes('子代理') ||
            thread.title?.includes('子智能体') ||
            (thread.agentId === 'ada-coding' && thread.parentId)
          )
          const closable = openTabs.length > 1
          // PM 是**模式**（同一 ada-coding 实例的模式预设）；agentId 兼容老会话
          const isPm = thread.mode === 'pm' || thread.agentId === 'ada-pm' || thread.agentId === 'pm-assistant'
          const statusInfo = resolveThreadStatus(thread, threadMap, runningThreadIds)

          return (
            <div
              key={thread.id}
              data-tab-id={thread.id}
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
                {isSubagent ? (
                  <Bot
                    size={12.5}
                    className={
                      isSelected
                        ? 'text-purple-600 dark:text-purple-400 flex-shrink-0'
                        : 'text-purple-400 dark:text-purple-500 flex-shrink-0'
                    }
                  />
                ) : isPm ? (
                  <Bot
                    size={12}
                    className={
                      isSelected
                        ? 'text-amber-500 flex-shrink-0'
                        : 'text-amber-500/70 dark:text-amber-400/60 flex-shrink-0'
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

                {/* 状态图标 */}
                {statusInfo.status === 'running' || running ? (
                  <Loader2 size={11} className="animate-spin text-blue-500 flex-shrink-0" />
                ) : statusInfo.status === 'failed' ? (
                  <XCircle size={11} className="text-rose-500 flex-shrink-0" />
                ) : statusInfo.status === 'completed' ? (
                  <CheckCircle2 size={11} className="text-emerald-500 dark:text-emerald-400 flex-shrink-0" />
                ) : statusInfo.status === 'waiting' ? (
                  <Clock size={11} className="text-amber-500 flex-shrink-0" />
                ) : (
                  <Circle size={5} className="text-zinc-300 dark:text-zinc-600 flex-shrink-0" />
                )}

                <span className="truncate text-xs">
                  {thread.title || (isSubagent ? '子代理会话' : (isPm ? 'PM 会话' : '新建对话'))}
                </span>

                {isPm && !isSubagent && (
                  <span
                    className="px-1 py-0.2 rounded text-[8px] bg-amber-500/15 text-amber-600 dark:text-amber-400 font-semibold flex-shrink-0 leading-tight"
                    title="PM 模式（规划与委派）"
                  >
                    PM
                  </span>
                )}

                {isSubagent && (
                  <span className="px-1 py-0.2 rounded text-[8px] bg-purple-500/15 text-purple-600 dark:text-purple-400 font-semibold flex-shrink-0 leading-tight">
                    子智能体
                  </span>
                )}
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
