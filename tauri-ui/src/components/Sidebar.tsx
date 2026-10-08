import React, { useState, useMemo, memo } from 'react'
import {
  Plus,
  MessageSquare,
  Trash2,
  FolderGit2,
  FolderOpen,
  Search,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  Bot,
  Download,
} from 'lucide-react'
import type { Thread } from '../types'
import { agentClient } from '../client/ws-client'
import { notify } from './ToastHost'

interface SidebarProps {
  threads: Thread[]
  activeThreadId: string
  activeWorkspace: string
  runningThreadIds?: string[]
  onSelectThread: (threadId: string) => void
  onCreateThread: (workspace?: string) => void
  onDeleteThread: (threadId: string) => void
  onRemoveWorkspace?: (workspace: string) => void
  onOpenWorkspacePicker?: () => void
}

/** 路径规范化函数，抹平 Windows 斜杠与大小写差异，防止重复分组与闪烁 */
function normalizePathKey(p?: string): string {
  if (!p) return ''
  return p.trim().replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()
}

const SidebarComponent: React.FC<SidebarProps> = ({
  threads,
  activeThreadId,
  activeWorkspace,
  runningThreadIds = [],
  onSelectThread,
  onCreateThread,
  onDeleteThread,
  onRemoveWorkspace,
  onOpenWorkspacePicker,
}) => {
  const [collapsed, setCollapsed] = useState(false)
  const [search, setSearch] = useState('')
  // 工作区手风琴状态：默认全收起 (null)，每次仅允许展开一个节点，展开另一个时其余自动收起
  const [expandedWorkspaceKey, setExpandedWorkspaceKey] = useState<string | null>(null)
  // 子代理子树折叠状态：默认收起，点击或处于激活态时展开
  const [expandedParents, setExpandedParents] = useState<Record<string, boolean>>({})
  // 会话标题重命名状态
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editTitle, setEditTitle] = useState('')

  const toggleParent = (parentId: string) => {
    setExpandedParents((prev) => ({
      ...prev,
      [parentId]: !prev[parentId],
    }))
  }

  const toggleWorkspace = (key: string) => {
    setExpandedWorkspaceKey((prev) => (prev === key ? null : key))
  }

  const handleExportThread = (e: React.MouseEvent, t: Thread) => {
    e.stopPropagation()
    try {
      const md = agentClient.exportThreadToMarkdown(t)
      if (navigator?.clipboard?.writeText) {
        navigator.clipboard.writeText(md).catch(() => {})
      }
      const blob = new Blob([md], { type: 'text/markdown;charset=utf-8' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      const safeTitle = (t.title || '会话导出').replace(/[\\/:*?"<>|]/g, '_')
      a.download = `${safeTitle}-${t.id.slice(0, 8)}.md`
      document.body.appendChild(a)
      a.click()
      document.body.removeChild(a)
      URL.revokeObjectURL(url)
      notify({
        level: 'success',
        message: '导出会话成功',
        detail: `已导出「${t.title || '未命名会话'}」，内容已复制到剪贴板`,
      })
    } catch (err: any) {
      notify({
        level: 'error',
        message: '导出失败',
        detail: err?.message || String(err),
      })
    }
  }

  // 将会话按工作区树形归纳分组，并在工作区内按父子关系构建树形会话列表
  const workspaceGroups = useMemo(() => {
    // key: 规范化路径, value: { rawWorkspace, dirName, threads }
    const groupMap = new Map<
      string,
      {
        rawWorkspace: string
        dirName: string
        threads: Thread[]
      }
    >()

    // 归组所有会话（会话严格绑定其所属工程目录）
    for (const t of threads) {
      const rawWs = t.workspace && t.workspace.trim() ? t.workspace : activeWorkspace || '默认工作区'
      const key = normalizePathKey(rawWs) || 'default'

      if (!groupMap.has(key)) {
        const dir = rawWs.split(/[\\/]/).filter(Boolean).pop() || rawWs
        groupMap.set(key, {
          rawWorkspace: rawWs,
          dirName: dir,
          threads: [],
        })
      }

      groupMap.get(key)!.threads.push(t)
    }

    // 仅在无任何会话时，兜底显示 activeWorkspace 作为新建入口
    if (groupMap.size === 0) {
      const normActive = normalizePathKey(activeWorkspace)
      if (normActive) {
        const dir = activeWorkspace.split(/[\\/]/).filter(Boolean).pop() || activeWorkspace
        groupMap.set(normActive, {
          rawWorkspace: activeWorkspace,
          dirName: dir,
          threads: [],
        })
      }
    }

    return Array.from(groupMap.entries()).map(([key, group]) => {
      // 保持会话列表顺序稳定：以 createdAt 降序作为主序，id 作为平局决胜
      const sorted = [...group.threads].sort((a, b) => {
        const timeA = a.createdAt || a.updatedAt || 0
        const timeB = b.createdAt || b.updatedAt || 0
        if (timeB !== timeA) return timeB - timeA
        return a.id.localeCompare(b.id)
      })

      // 建立 ID 索引，并将子代理关联到父会话
      const threadMap = new Map<string, Thread>()
      for (const t of sorted) {
        threadMap.set(t.id, t)
      }

      const subagentMap = new Map<string, Thread[]>()
      for (const t of sorted) {
        if (t.isSubagent && t.parentId && threadMap.has(t.parentId)) {
          const list = subagentMap.get(t.parentId) || []
          list.push(t)
          subagentMap.set(t.parentId, list)
        }
      }

      // 构建树节点列表：只有顶级主会话（或找不到父会话的子代理会话）作为根节点
      const rawNodes = sorted
        .filter((t) => !(t.isSubagent && t.parentId && threadMap.has(t.parentId)))
        .map((mainThread) => ({
          thread: mainThread,
          subagents: subagentMap.get(mainThread.id) || [],
        }))

      // 搜索过滤：若父会话匹配或其名下任意子代理匹配则保留
      const filteredNodes = search.trim()
        ? rawNodes
            .map((node) => {
              const term = search.trim().toLowerCase()
              const parentMatch = (node.thread.title || '新对话').toLowerCase().includes(term)
              const matchedSubagents = node.subagents.filter((sub) =>
                (sub.title || '子代理会话').toLowerCase().includes(term)
              )
              if (parentMatch) {
                return node
              }
              if (matchedSubagents.length > 0) {
                return { ...node, subagents: matchedSubagents }
              }
              return null
            })
            .filter((n): n is { thread: Thread; subagents: Thread[] } => n !== null)
        : rawNodes

      return {
        key,
        workspace: group.rawWorkspace,
        dirName: group.dirName,
        nodes: filteredNodes,
        totalCount: sorted.length,
      }
    })
  }, [threads, activeWorkspace, search])

  // 当前激活会话所属的上级工作区，作为新建对话的首选工作区
  const currentThreadWorkspace =
    threads.find((t) => t.id === activeThreadId)?.workspace || activeWorkspace

  // 极简折叠模式（只留窄边栏）
  if (collapsed) {
    return (
      <aside className="w-12 bg-zinc-50 dark:bg-[#121214] border-r border-zinc-200 dark:border-[#27272a] flex flex-col items-center py-3 flex-shrink-0 select-none transition-colors duration-100">
        <button
          onClick={() => setCollapsed(false)}
          className="p-2 text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded-lg transition-colors mb-3 cursor-pointer"
          title="展开工作区树"
        >
          <ChevronRight size={16} />
        </button>
        <button
          onClick={() => onCreateThread(currentThreadWorkspace)}
          className="p-2 bg-blue-600 hover:bg-blue-500 text-white rounded-lg transition-colors shadow-xs mb-4 cursor-pointer"
          title="在所属工作区新建对话"
        >
          <Plus size={16} />
        </button>
      </aside>
    )
  }

  return (
    <aside className="w-64 bg-zinc-50 dark:bg-[#121214] border-r border-zinc-200 dark:border-[#27272a] flex flex-col h-full flex-shrink-0 select-none transition-colors duration-100">
      {/* 顶部：新建、打开工作区与折叠 */}
      <div className="p-3 border-b border-zinc-200 dark:border-[#27272a]/60 flex items-center justify-between gap-1.5">
        <button
          onClick={() => onCreateThread(currentThreadWorkspace)}
          className="flex-1 flex items-center justify-center space-x-1.5 py-1.5 px-2.5 bg-blue-600 hover:bg-blue-500 active:bg-blue-700 text-white text-xs font-medium rounded-lg transition-colors shadow-xs cursor-pointer"
          title="在所属工作区新建对话"
        >
          <Plus size={14} />
          <span>新建对话</span>
        </button>

        {onOpenWorkspacePicker && (
          <button
            onClick={onOpenWorkspacePicker}
            className="p-1.5 text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded-md transition-colors cursor-pointer"
            title="选择并打开工作区目录"
          >
            <FolderOpen size={15} />
          </button>
        )}

        <button
          onClick={() => setCollapsed(true)}
          className="p-1.5 text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded-md transition-colors cursor-pointer"
          title="折叠侧边栏"
        >
          <ChevronLeft size={16} />
        </button>
      </div>

      {/* 搜索框 */}
      <div className="px-3 pt-2 pb-1">
        <div className="relative flex items-center">
          <Search size={12} className="absolute left-2.5 text-zinc-400 dark:text-zinc-500 pointer-events-none" />
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="搜索工作区或会话..."
            className="w-full bg-white dark:bg-[#1e1e22] text-zinc-900 dark:text-zinc-200 placeholder-zinc-400 dark:placeholder-zinc-500 text-xs rounded-md pl-7 pr-2.5 py-1.5 outline-none border border-zinc-200 dark:border-transparent focus:border-blue-500/50 shadow-xs dark:shadow-none transition-colors"
          />
        </div>
      </div>

      {/* 工作区 + 会话 树形展示列表：添加 [scrollbar-gutter:stable] 防抖 */}
      <div className="flex-1 overflow-y-auto px-2 py-1.5 space-y-1.5 [scrollbar-gutter:stable]">
        {workspaceGroups.length === 0 ? (
          <div className="text-center py-8 text-xs text-zinc-400 dark:text-zinc-600">
            暂无工作区记录
          </div>
        ) : (
          workspaceGroups.map((group) => {
            // 工作区手风琴折叠判断：搜索时展开匹配节点；平时仅展开 expandedWorkspaceKey 对应的唯一节点，其余默认全收起
            const isFolded = search.trim() ? false : expandedWorkspaceKey !== group.key

            if (search.trim() && group.nodes.length === 0) {
              return null
            }

            return (
              <div key={group.key} className="space-y-0.5">
                {/* 树节点：工作区分组头部 */}
                <div
                  onClick={() => toggleWorkspace(group.key)}
                  className="group flex items-center justify-between px-2 py-1 rounded-md text-xs cursor-pointer hover:bg-zinc-200/50 dark:hover:bg-zinc-800/40 transition-colors"
                  title={group.workspace}
                >
                  <div className="flex items-center space-x-1.5 truncate flex-1 min-w-0">
                    <span className="text-zinc-400 dark:text-zinc-500 flex-shrink-0">
                      {isFolded ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
                    </span>

                    {isFolded ? (
                      <FolderGit2 size={13} className="text-zinc-400 dark:text-zinc-500 flex-shrink-0" />
                    ) : (
                      <FolderOpen size={13} className="text-blue-500 flex-shrink-0" />
                    )}

                    <span className="truncate font-semibold text-zinc-700 dark:text-zinc-300">
                      {group.dirName}
                    </span>
                  </div>

                  {/* 悬停操作与计数 */}
                  <div className="flex items-center space-x-1 flex-shrink-0">
                    <button
                      onClick={(e) => {
                        e.stopPropagation()
                        setExpandedWorkspaceKey(group.key)
                        onCreateThread(group.workspace)
                      }}
                      className="opacity-0 group-hover:opacity-100 p-0.5 text-zinc-400 hover:text-blue-600 dark:hover:text-blue-400 hover:bg-zinc-200 dark:hover:bg-zinc-700 rounded transition-opacity cursor-pointer"
                      title={`在此工作区新建对话`}
                    >
                      <Plus size={12} />
                    </button>

                    {onRemoveWorkspace && workspaceGroups.length > 1 && (
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          onRemoveWorkspace(group.workspace)
                        }}
                        className="opacity-0 group-hover:opacity-100 p-0.5 text-zinc-400 hover:text-rose-500 hover:bg-zinc-200/70 dark:hover:bg-zinc-700/60 rounded transition-opacity cursor-pointer"
                        title={`从列表中移除工作区「${group.dirName}」`}
                      >
                        <Trash2 size={12} />
                      </button>
                    )}

                    <span className="text-[10px] px-1.5 py-0.2 rounded-full font-mono text-zinc-400 dark:text-zinc-500 bg-zinc-200/60 dark:bg-zinc-800/60">
                      {group.totalCount}
                    </span>
                  </div>
                </div>

                {/* 树叶节点：工作区所属会话列表（主会话及其子代理子项） */}
                {!isFolded && (
                  <div className="ml-3 pl-2 border-l border-zinc-200/80 dark:border-zinc-800 space-y-0.5 pt-0.5">
                    {group.nodes.length === 0 ? (
                      <div
                        onClick={() => onCreateThread(group.workspace)}
                        className="px-2 py-1 text-[11px] text-zinc-400 dark:text-zinc-500 hover:text-blue-500 cursor-pointer italic"
                      >
                        暂无会话 · 点击新建
                      </div>
                    ) : (
                      group.nodes.map((node) => {
                        const { thread, subagents } = node
                        const isActive = thread.id === activeThreadId
                        const isSubagent = Boolean(thread.isSubagent)
                        const hasSubagents = subagents.length > 0
                        // 子代理默认收起，仅在用户主动展开或当前选中的会话正是其中之一时自动展开
                        const isSubTreeExpanded =
                          Boolean(expandedParents[thread.id]) ||
                          subagents.some((sub) => sub.id === activeThreadId)
                        const isSubTreeFolded = !isSubTreeExpanded

                        return (
                          <div key={thread.id} className="flex flex-col space-y-0.5">
                            {/* 主会话节点 */}
                            <div
                              onClick={() => onSelectThread(thread.id)}
                              className={`group flex items-center justify-between px-2 py-1 rounded-md text-xs cursor-pointer transition-colors ${
                                isActive
                                  ? 'bg-white dark:bg-zinc-800 text-blue-600 dark:text-white font-medium shadow-xs border border-zinc-200/70 dark:border-zinc-700/60'
                                  : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/50 dark:hover:bg-zinc-800/50 hover:text-zinc-900 dark:hover:text-zinc-200'
                              }`}
                              title={thread.title || (isSubagent ? '子代理会话' : '新对话')}
                            >
                              <div className="flex items-center space-x-1.5 truncate min-w-0">
                                {hasSubagents && (
                                  <button
                                    type="button"
                                    onClick={(e) => {
                                      e.stopPropagation()
                                      toggleParent(thread.id)
                                    }}
                                    className="p-0.5 text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 rounded cursor-pointer transition-transform"
                                    title={isSubTreeFolded ? '展开子代理列表' : '折叠子代理列表'}
                                  >
                                    {isSubTreeFolded ? (
                                      <ChevronRight size={11} />
                                    ) : (
                                      <ChevronDown size={11} />
                                    )}
                                  </button>
                                )}

                                {runningThreadIds.includes(thread.id) ? (
                                  <span className="relative flex h-2 w-2 flex-shrink-0 mr-0.5">
                                    <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-blue-400 opacity-75" />
                                    <span className="relative inline-flex rounded-full h-2 w-2 bg-blue-500" />
                                  </span>
                                ) : isSubagent ? (
                                  <Bot
                                    size={12}
                                    className={isActive ? 'text-purple-500 flex-shrink-0' : 'text-purple-400 dark:text-purple-400/80 flex-shrink-0'}
                                  />
                                ) : (
                                  <MessageSquare
                                    size={12}
                                    className={isActive ? 'text-blue-500 flex-shrink-0' : 'text-zinc-400 dark:text-zinc-500 flex-shrink-0'}
                                  />
                                )}

                                {editingId === thread.id ? (
                                  <input
                                    type="text"
                                    value={editTitle}
                                    autoFocus
                                    onClick={(e) => e.stopPropagation()}
                                    onChange={(e) => setEditTitle(e.target.value)}
                                    onBlur={() => {
                                      if (editTitle.trim()) {
                                        agentClient.updateThreadTitle(thread.id, editTitle.trim())
                                      }
                                      setEditingId(null)
                                    }}
                                    onKeyDown={(e) => {
                                      if (e.key === 'Enter') {
                                        if (editTitle.trim()) {
                                          agentClient.updateThreadTitle(thread.id, editTitle.trim())
                                        }
                                        setEditingId(null)
                                      } else if (e.key === 'Escape') {
                                        setEditingId(null)
                                      }
                                    }}
                                    className="px-1 py-0.2 text-xs bg-white dark:bg-black/60 border border-blue-500 rounded outline-none w-full text-zinc-900 dark:text-zinc-100"
                                  />
                                ) : (
                                  <span
                                    className="truncate"
                                    onDoubleClick={(e) => {
                                      e.stopPropagation()
                                      setEditingId(thread.id)
                                      setEditTitle(thread.title || '')
                                    }}
                                    title="双击重命名会话"
                                  >
                                    {thread.title || (isSubagent ? '子代理会话' : '新对话')}
                                  </span>
                                )}

                                {isSubagent && (
                                  <span className="px-1 py-0.2 rounded text-[9px] bg-purple-100 dark:bg-purple-950/60 text-purple-700 dark:text-purple-300 border border-purple-200 dark:border-purple-800/50 font-medium flex-shrink-0">
                                    子代理
                                  </span>
                                )}

                                {hasSubagents && (
                                  <span
                                    className="px-1 py-0.2 rounded text-[9px] bg-purple-50 dark:bg-purple-950/40 text-purple-600 dark:text-purple-300 border border-purple-200/50 dark:border-purple-800/40 font-mono font-medium flex-shrink-0"
                                    title={`包含 ${subagents.length} 个子智能体执行会话`}
                                  >
                                    {subagents.length}子代理
                                  </span>
                                )}
                              </div>

                              <div className="flex items-center space-x-0.5 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0 ml-1">
                                <button
                                  onClick={(e) => handleExportThread(e, thread)}
                                  className="p-0.5 text-zinc-400 hover:text-blue-500 hover:bg-zinc-200/70 dark:hover:bg-zinc-700/40 rounded cursor-pointer"
                                  title="导出会话为 Markdown 并复制"
                                >
                                  <Download size={11} />
                                </button>
                                <button
                                  onClick={(e) => {
                                    e.stopPropagation()
                                    onDeleteThread(thread.id)
                                  }}
                                  className="p-0.5 text-zinc-400 hover:text-rose-500 hover:bg-zinc-200/70 dark:hover:bg-zinc-700/40 rounded cursor-pointer"
                                  title="删除会话"
                                >
                                  <Trash2 size={11} />
                                </button>
                              </div>
                            </div>

                            {/* 嵌套子项：所属子代理会话列表 */}
                            {hasSubagents && !isSubTreeFolded && (
                              <div className="ml-4 pl-2 border-l border-purple-200/80 dark:border-purple-900/50 space-y-0.5 pt-0.5">
                                {subagents.map((sub) => {
                                  const isSubActive = sub.id === activeThreadId
                                  return (
                                    <div
                                      key={sub.id}
                                      onClick={() => onSelectThread(sub.id)}
                                      className={`group flex items-center justify-between px-2 py-1 rounded-md text-xs cursor-pointer transition-colors ${
                                        isSubActive
                                          ? 'bg-purple-50 dark:bg-purple-950/40 text-purple-700 dark:text-purple-300 font-medium shadow-xs border border-purple-200/80 dark:border-purple-800/60'
                                          : 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/50 dark:hover:bg-zinc-800/50 hover:text-zinc-900 dark:hover:text-zinc-200'
                                      }`}
                                      title={sub.title || '子代理会话'}
                                    >
                                      <div className="flex items-center space-x-1.5 truncate min-w-0">
                                        <Bot
                                          size={12}
                                          className={
                                            isSubActive
                                              ? 'text-purple-600 dark:text-purple-400 flex-shrink-0'
                                              : 'text-purple-400/80 flex-shrink-0'
                                          }
                                        />
                                        <span className="truncate">{sub.title || '子代理会话'}</span>
                                        <span className="px-1 py-0.2 rounded text-[9px] bg-purple-100 dark:bg-purple-950/60 text-purple-700 dark:text-purple-300 border border-purple-200 dark:border-purple-800/50 font-medium flex-shrink-0">
                                          子代理
                                        </span>
                                      </div>

                                      <div className="flex items-center space-x-0.5 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0 ml-1">
                                        <button
                                          onClick={(e) => handleExportThread(e, sub)}
                                          className="p-0.5 text-zinc-400 hover:text-blue-500 hover:bg-zinc-200/70 dark:hover:bg-zinc-700/40 rounded cursor-pointer"
                                          title="导出子代理会话为 Markdown 并复制"
                                        >
                                          <Download size={11} />
                                        </button>
                                        <button
                                          onClick={(e) => {
                                            e.stopPropagation()
                                            onDeleteThread(sub.id)
                                          }}
                                          className="p-0.5 text-zinc-400 hover:text-rose-500 hover:bg-zinc-200/70 dark:hover:bg-zinc-700/40 rounded cursor-pointer"
                                          title="删除子代理会话"
                                        >
                                          <Trash2 size={11} />
                                        </button>
                                      </div>
                                    </div>
                                  )
                                })}
                              </div>
                            )}
                          </div>
                        )
                      })
                    )}
                  </div>
                )}
              </div>
            )
          })
        )}
      </div>

      {/* 底部统计栏 */}
      <div className="p-2.5 border-t border-zinc-200 dark:border-[#27272a] text-[10.5px] text-zinc-400 dark:text-zinc-500 flex items-center justify-between">
        <span>{workspaceGroups.length} 工作区 · {threads.length} 会话</span>
        <span className="text-[9.5px] text-zinc-400 dark:text-zinc-600">Tauri v2</span>
      </div>
    </aside>
  )
}

/**
 * 使用自定义比较函数的 React.memo：
 * 对话时（如 Assistant 流式吐字、Tool 执行），会话内部的 items 在高频追加，但会话的元数据并未变化。
 * 此处阻断由于 items 高频更新引发的 Sidebar 无意义重渲染，彻底消除由于频繁 re-render 导致的视觉抖动！
 */
export const Sidebar = memo(SidebarComponent, (prev, next) => {
  if (prev.activeThreadId !== next.activeThreadId) return false
  if (prev.activeWorkspace !== next.activeWorkspace) return false
  if (prev.threads.length !== next.threads.length) return false

  // 浅比较每个会话的元数据（id, title, workspace, isSubagent, parentId）
  for (let i = 0; i < prev.threads.length; i++) {
    const pt = prev.threads[i]
    const nt = next.threads[i]
    if (
      pt.id !== nt.id ||
      pt.title !== nt.title ||
      pt.workspace !== nt.workspace ||
      pt.isSubagent !== nt.isSubagent ||
      pt.parentId !== nt.parentId
    ) {
      return false
    }
  }

  // 元数据无变化，跳过重绘，保持左侧树完全静止稳定
  return true
})
