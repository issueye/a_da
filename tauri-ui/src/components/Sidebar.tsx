import React, { useState, useMemo, useEffect, memo } from 'react'
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
  CheckCircle2,
  XCircle,
  Clock,
  Circle,
  Loader2,
} from 'lucide-react'
import type { Thread, AgentMode } from '../types'
import { agentClient } from '../client/ws-client'
import { notify } from './ToastHost'
// 会话状态判定是**纯函数**，放在 utils 里（无 React / 无 tauri 依赖）：
// 侧边栏与标签页共用同一份，且可以拿真实快照直接跑脚本验证（见文件头注释）。
import { resolveThreadStatus } from '../utils/thread-status'

export interface TreeNode {
  thread: Thread
  level: number
  children: TreeNode[]
}

interface SidebarProps {
  threads: Thread[]
  activeThreadId: string
  activeWorkspace: string
  currentMode?: AgentMode
  runningThreadIds?: string[]
  onSelectThread: (threadId: string) => void
  onCreateThread: (workspace?: string, mode?: AgentMode) => void
  onSelectMode?: (mode: AgentMode) => void
  onDeleteThread: (threadId: string) => void
  onRemoveWorkspace?: (workspace: string) => void
  onOpenWorkspacePicker?: () => void
}

/** 路径规范化函数，抹平 Windows 斜杠与大小写差异，防止重复分组与闪烁 */
function normalizePathKey(p?: string): string {
  if (!p) return ''
  return p.trim().replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()
}

/** 递归检查当前节点或其任意后代节点是否为激活会话 */
function hasActiveDescendant(node: TreeNode, activeId: string): boolean {
  if (node.thread.id === activeId) return true
  for (const child of node.children) {
    if (hasActiveDescendant(child, activeId)) return true
  }
  return false
}

/** 优化提取子代理标题中的角色与核心任务文本，去除 redundant 前缀 */
function parseSubagentTitle(rawTitle?: string): { role?: string; cleanTitle: string } {
  if (!rawTitle || !rawTitle.trim()) {
    return { cleanTitle: '' }
  }
  const title = rawTitle.trim()
  const match = title.match(/^(?:子代理|子智能体)?\s*\[(.*?)\]\s*[:：]?\s*(.*)$/)
  if (match) {
    const role = match[1].trim()
    const rest = match[2].trim()
    return {
      role,
      cleanTitle: rest || `[${role}]`,
    }
  }
  return { cleanTitle: title }
}

/** 递归搜索过滤树节点 */
function filterTreeNode(node: TreeNode, term: string): TreeNode | null {
  const defaultTitle =
    node.level === 1 ? '新对话' : node.level === 2 ? '子进程子代理' : '进程内子代理'
  const parsed = parseSubagentTitle(node.thread.title)
  const fullText = `${node.thread.title || defaultTitle} ${parsed.cleanTitle} ${parsed.role || ''}`.toLowerCase()
  const selfMatch = fullText.includes(term)
  const filteredChildren = node.children
    .map((c) => filterTreeNode(c, term))
    .filter((c): c is TreeNode => c !== null)

  if (selfMatch || filteredChildren.length > 0) {
    return {
      ...node,
      children: filteredChildren,
    }
  }
  return null
}

/** 统计树结构中所有节点总数 */
function countTreeNodes(nodes: TreeNode[]): number {
  return nodes.reduce((acc, n) => acc + 1 + countTreeNodes(n.children), 0)
}

/**
 * 紧凑型树节点展示组件：
 * - 紧凑行高（min-h-[26px]，py-0.5）
 * - 专属状态图标（运行中/完成/失败/等待/就绪）清晰直观，覆盖各层级会话与子智能体
 * - 左右分栏对齐：左侧占据所有标题空间，右侧统一徽章与悬停按钮
 */
interface SidebarTreeItemProps {
  node: TreeNode
  parentThread?: Thread
  rootThread: Thread
  activeThreadId: string
  runningThreadIds: string[]
  allThreadMap: Map<string, Thread>
  expandedParents: Record<string, boolean>
  editingId: string | null
  editTitle: string
  onToggleParent: (id: string, currentExpanded: boolean) => void
  onSelectThread: (id: string) => void
  onDeleteThread: (id: string) => void
  onExportThread: (e: React.MouseEvent, t: Thread) => void
  setEditingId: (id: string | null) => void
  setEditTitle: (title: string) => void
}

const SidebarTreeItem: React.FC<SidebarTreeItemProps> = ({
  node,
  parentThread,
  rootThread,
  activeThreadId,
  runningThreadIds,
  allThreadMap,
  expandedParents,
  editingId,
  editTitle,
  onToggleParent,
  onSelectThread,
  onDeleteThread,
  onExportThread,
  setEditingId,
  setEditTitle,
}) => {
  const { thread, level, children } = node
  const isActive = thread.id === activeThreadId
  const hasChildren = children.length > 0

  // PM 是**模式**（同一 ada-coding 实例的模式预设），不是第二个产品。
  // 仍兼容历史会话的 `agentId` 写法（读盘时会被归一，但界面不必依赖它）。
  const isRootPm =
    rootThread.mode === 'pm' ||
    rootThread.agentId === 'ada-pm' ||
    rootThread.agentId === 'pm-assistant'

  // 计算折叠/展开状态
  const hasActiveChild = useMemo(
    () => hasActiveDescendant(node, activeThreadId),
    [node, activeThreadId]
  )
  const isExpanded =
    expandedParents[thread.id] !== undefined ? expandedParents[thread.id] : hasActiveChild

  // 默认标题
  const defaultTitle =
    level === 1
      ? isRootPm
        ? 'PM 对话'
        : '新对话'
      : level === 2
      ? '子进程子代理'
      : '进程内子代理'

  // 解析清洗标题
  const parsed = useMemo(() => parseSubagentTitle(thread.title), [thread.title])
  const displayTitle = parsed.cleanTitle || defaultTitle

  // 计算会话状态及图标
  const statusInfo = useMemo(
    () => resolveThreadStatus(thread, allThreadMap, runningThreadIds),
    [thread, allThreadMap, runningThreadIds]
  )

  // 完整悬停提示信息
  const tooltipText = useMemo(() => {
    const lines = [thread.title || defaultTitle]
    if (parsed.role) {
      lines.push(`角色: ${parsed.role}`)
    }
    lines.push(`状态: ${statusInfo.label}`)
    if (thread.workspace) {
      lines.push(`工作区: ${thread.workspace}`)
    }
    return lines.join('\n')
  }, [thread.title, defaultTitle, parsed.role, statusInfo.label, thread.workspace])

  // 紧凑背景高亮样式
  const itemStyle = useMemo(() => {
    if (isActive) {
      if (level === 1) {
        return isRootPm
          ? 'bg-amber-500/10 dark:bg-amber-950/40 text-amber-900 dark:text-amber-200 font-medium shadow-2xs border border-amber-300/60 dark:border-amber-800/60'
          : 'bg-white dark:bg-zinc-800 text-blue-600 dark:text-white font-medium shadow-2xs border border-zinc-200/70 dark:border-zinc-700/60'
      } else if (level === 2) {
        return 'bg-indigo-50/90 dark:bg-indigo-950/50 text-indigo-800 dark:text-indigo-200 font-medium shadow-2xs border border-indigo-200 dark:border-indigo-800/60'
      } else {
        return 'bg-purple-50/90 dark:bg-purple-950/50 text-purple-800 dark:text-purple-200 font-medium shadow-2xs border border-purple-200 dark:border-purple-800/60'
      }
    }
    return 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/50 dark:hover:bg-zinc-800/50 hover:text-zinc-900 dark:hover:text-zinc-200 border border-transparent'
  }, [isActive, level, isRootPm])

  return (
    <div className="flex flex-col space-y-0.5">
      {/* 紧凑节点条 */}
      <div
        onClick={() => onSelectThread(thread.id)}
        className={`group flex items-center justify-between px-1.5 py-0.5 min-h-[26px] rounded text-[11.5px] cursor-pointer transition-colors ${itemStyle}`}
        title={tooltipText}
      >
        {/* 左侧：折叠指示、类型图标、状态图标、标题 */}
        <div className="flex items-center space-x-1 min-w-0 flex-1 mr-1">
          {/* 折叠/展开箭头或占位对齐符 */}
          {hasChildren ? (
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                onToggleParent(thread.id, isExpanded)
              }}
              className="w-3.5 h-3.5 flex items-center justify-center p-0 text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 rounded cursor-pointer transition-transform flex-shrink-0"
              title={isExpanded ? '折叠子列表' : '展开子列表'}
            >
              {isExpanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
            </button>
          ) : (
            <span className="w-3.5 flex-shrink-0" />
          )}

          {/* 类型图标 */}
          {level === 1 ? (
            isRootPm ? (
              <Bot
                size={11.5}
                className={
                  isActive
                    ? 'text-amber-600 dark:text-amber-400 flex-shrink-0'
                    : 'text-amber-500/80 dark:text-amber-400/70 flex-shrink-0'
                }
              />
            ) : (
              <MessageSquare
                size={11.5}
                className={
                  isActive
                    ? 'text-blue-500 flex-shrink-0'
                    : 'text-zinc-400 dark:text-zinc-500 flex-shrink-0'
                }
              />
            )
          ) : level === 2 ? (
            <Bot
              size={11.5}
              className={
                isActive
                  ? 'text-indigo-600 dark:text-indigo-400 flex-shrink-0'
                  : 'text-indigo-500/80 dark:text-indigo-400/70 flex-shrink-0'
              }
            />
          ) : (
            <Bot
              size={11.5}
              className={
                isActive
                  ? 'text-purple-600 dark:text-purple-400 flex-shrink-0'
                  : 'text-purple-400/80 flex-shrink-0'
              }
            />
          )}

          {/* 专属状态图标 */}
          {statusInfo.status === 'running' ? (
            <span
              className="flex items-center justify-center flex-shrink-0"
              title="状态: 执行中"
            >
              <Loader2 size={11} className="animate-spin text-blue-500" />
            </span>
          ) : statusInfo.status === 'failed' ? (
            <span
              className="flex items-center justify-center flex-shrink-0"
              title="状态: 执行失败"
            >
              <XCircle size={11} className="text-rose-500 dark:text-rose-400" />
            </span>
          ) : statusInfo.status === 'completed' ? (
            <span
              className="flex items-center justify-center flex-shrink-0"
              title="状态: 执行完成"
            >
              <CheckCircle2 size={11} className="text-emerald-500 dark:text-emerald-400" />
            </span>
          ) : statusInfo.status === 'waiting' ? (
            <span
              className="flex items-center justify-center flex-shrink-0"
              title="状态: 等待审批/答复"
            >
              <Clock size={11} className="text-amber-500 dark:text-amber-400" />
            </span>
          ) : (
            <span
              className="flex items-center justify-center flex-shrink-0"
              title="状态: 空闲就绪"
            >
              <Circle size={6.5} className="text-zinc-300 dark:text-zinc-600" />
            </span>
          )}

          {/* 标题（支持双击重命名，自适应填满剩余空间） */}
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
              className="px-1 py-0 text-[11.5px] bg-white dark:bg-black/60 border border-blue-500 rounded outline-none w-full text-zinc-900 dark:text-zinc-100 flex-1 min-w-0"
            />
          ) : (
            <span
              className="truncate flex-1 min-w-0 font-normal leading-tight select-none"
              onDoubleClick={(e) => {
                e.stopPropagation()
                setEditingId(thread.id)
                setEditTitle(thread.title || '')
              }}
              title="双击重命名会话"
            >
              {displayTitle}
            </span>
          )}
        </div>

        {/* 右侧：标签与计数 / 悬停操作按钮 */}
        <div className="flex items-center space-x-1 flex-shrink-0">
          <div className="flex items-center space-x-1 group-hover:hidden">
            {/* 角色与层级徽章 */}
            {level === 1 ? (
              isRootPm ? (
                <span
                  className="px-1 py-0 rounded text-[8.5px] bg-amber-100 dark:bg-amber-950/60 text-amber-700 dark:text-amber-300 border border-amber-200 dark:border-amber-800/50 font-semibold flex-shrink-0 leading-tight"
                  title="PM 模式（规划与委派）——同一个 ada-coding 实例的模式预设"
                >
                  PM
                </span>
              ) : (
                <span
                  className="px-1 py-0 rounded text-[8.5px] bg-blue-50 dark:bg-blue-950/40 text-blue-600 dark:text-blue-400 border border-blue-200/50 dark:border-blue-800/40 font-mono font-medium flex-shrink-0 leading-tight"
                  title="CODING 模式（编码与执行）——产品 ada-coding"
                >
                  CODING
                </span>
              )
            ) : level === 2 ? (
              <span
                className="px-1 py-0 rounded text-[8.5px] bg-indigo-100 dark:bg-indigo-950/60 text-indigo-700 dark:text-indigo-300 border border-indigo-200 dark:border-indigo-800/50 font-medium flex-shrink-0 leading-tight"
                title={parsed.role ? `子进程子代理 [${parsed.role}]` : '子进程子代理'}
              >
                子进程
              </span>
            ) : (
              <span
                className="px-1 py-0 rounded text-[8.5px] bg-purple-100 dark:bg-purple-950/60 text-purple-700 dark:text-purple-300 border border-purple-200 dark:border-purple-800/50 font-medium flex-shrink-0 leading-tight"
                title={parsed.role ? `进程内子代理 [${parsed.role}]` : '进程内子代理'}
              >
                进程内
              </span>
            )}

            {/* 子项计数徽章 */}
            {hasChildren && (
              <span
                className={`px-1 py-0 rounded-full text-[8.5px] font-mono font-medium flex-shrink-0 leading-tight ${
                  level === 1
                    ? 'bg-indigo-50 dark:bg-indigo-950/40 text-indigo-600 dark:text-indigo-300 border border-indigo-200/50 dark:border-indigo-800/40'
                    : 'bg-purple-50 dark:bg-purple-950/40 text-purple-600 dark:text-purple-300 border border-purple-200/50 dark:border-purple-800/40'
                }`}
                title={
                  level === 1
                    ? `包含 ${children.length} 个子智能体会话`
                    : `包含 ${children.length} 个进程内子代理`
                }
              >
                {children.length}
              </span>
            )}
          </div>

          {/* 悬停操作按钮 */}
          <div className="hidden group-hover:flex items-center space-x-0.5">
            <button
              type="button"
              onClick={(e) => onExportThread(e, thread)}
              className="p-0.5 text-zinc-400 hover:text-blue-500 hover:bg-zinc-200/70 dark:hover:bg-zinc-700/50 rounded cursor-pointer transition-colors"
              title="导出会话为 Markdown 并复制"
            >
              <Download size={11} />
            </button>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                onDeleteThread(thread.id)
              }}
              className="p-0.5 text-zinc-400 hover:text-rose-500 hover:bg-zinc-200/70 dark:hover:bg-zinc-700/50 rounded cursor-pointer transition-colors"
              title="删除会话"
            >
              <Trash2 size={11} />
            </button>
          </div>
        </div>
      </div>

      {/* 紧凑子层级缩进 */}
      {hasChildren && isExpanded && (
        <div
          className={`ml-2.5 pl-1.5 border-l ${
            level === 1
              ? 'border-indigo-200/80 dark:border-indigo-900/50'
              : 'border-purple-200/80 dark:border-purple-900/50'
          } space-y-0.5 pt-0.5`}
        >
          {children.map((childNode) => (
            <SidebarTreeItem
              key={childNode.thread.id}
              node={childNode}
              parentThread={thread}
              rootThread={rootThread}
              activeThreadId={activeThreadId}
              runningThreadIds={runningThreadIds}
              allThreadMap={allThreadMap}
              expandedParents={expandedParents}
              editingId={editingId}
              editTitle={editTitle}
              onToggleParent={onToggleParent}
              onSelectThread={onSelectThread}
              onDeleteThread={onDeleteThread}
              onExportThread={onExportThread}
              setEditingId={setEditingId}
              setEditTitle={setEditTitle}
            />
          ))}
        </div>
      )}
    </div>
  )
}

const DEFAULT_SIDEBAR_WIDTH = 270
const MIN_SIDEBAR_WIDTH = 180
const MAX_SIDEBAR_WIDTH = 560

const SidebarComponent: React.FC<SidebarProps> = ({
  threads,
  activeThreadId,
  activeWorkspace,
  currentMode = 'code',
  runningThreadIds = [],
  onSelectThread,
  onCreateThread,
  onSelectMode,
  onDeleteThread,
  onRemoveWorkspace,
  onOpenWorkspacePicker,
}) => {
  const [collapsed, setCollapsed] = useState(false)
  const [mode, setMode] = useState<AgentMode>(currentMode)

  // 侧边栏宽度支持拖拽缩放与持久化
  const [width, setWidth] = useState<number>(() => {
    const saved = localStorage.getItem('a_da_sidebar_width')
    if (saved) {
      const parsed = parseInt(saved, 10)
      if (!isNaN(parsed) && parsed >= MIN_SIDEBAR_WIDTH && parsed <= MAX_SIDEBAR_WIDTH) {
        return parsed
      }
    }
    return DEFAULT_SIDEBAR_WIDTH
  })
  const [isResizing, setIsResizing] = useState(false)

  // 拖动过程中锁定全局光标与禁用文本选取
  useEffect(() => {
    if (isResizing) {
      document.body.style.cursor = 'col-resize'
      document.body.style.userSelect = 'none'
    } else {
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
    }
    return () => {
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
    }
  }, [isResizing])

  // 处理拖动调整侧边栏宽度
  const handleMouseDown = (e: React.MouseEvent) => {
    e.preventDefault()
    setIsResizing(true)

    const startX = e.clientX
    const startWidth = width

    const handleMouseMove = (moveEvent: MouseEvent) => {
      const deltaX = moveEvent.clientX - startX
      const maxAllowed = Math.min(MAX_SIDEBAR_WIDTH, Math.floor(window.innerWidth * 0.6))
      const nextWidth = Math.min(
        Math.max(startWidth + deltaX, MIN_SIDEBAR_WIDTH),
        maxAllowed
      )
      setWidth(nextWidth)
    }

    const handleMouseUp = (upEvent: MouseEvent) => {
      setIsResizing(false)
      window.removeEventListener('mousemove', handleMouseMove)
      window.removeEventListener('mouseup', handleMouseUp)

      const finalDeltaX = upEvent.clientX - startX
      const maxAllowed = Math.min(MAX_SIDEBAR_WIDTH, Math.floor(window.innerWidth * 0.6))
      const finalWidth = Math.min(
        Math.max(startWidth + finalDeltaX, MIN_SIDEBAR_WIDTH),
        maxAllowed
      )
      localStorage.setItem('a_da_sidebar_width', String(Math.round(finalWidth)))
    }

    window.addEventListener('mousemove', handleMouseMove)
    window.addEventListener('mouseup', handleMouseUp)
  }

  // 双击分隔线恢复默认宽度
  const handleResetWidth = () => {
    setWidth(DEFAULT_SIDEBAR_WIDTH)
    localStorage.setItem('a_da_sidebar_width', String(DEFAULT_SIDEBAR_WIDTH))
  }

  useEffect(() => {
    if (currentMode) {
      setMode(currentMode)
    }
  }, [currentMode])

  const [search, setSearch] = useState('')
  const [expandedWorkspaceKey, setExpandedWorkspaceKey] = useState<string | null>(null)
  const [expandedParents, setExpandedParents] = useState<Record<string, boolean>>({})
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editTitle, setEditTitle] = useState('')

  const toggleParent = (parentId: string, currentExpanded: boolean) => {
    setExpandedParents((prev) => ({
      ...prev,
      [parentId]: !currentExpanded,
    }))
  }

  const toggleWorkspace = (key: string, wsPath?: string) => {
    setExpandedWorkspaceKey((prev) => {
      const next = prev === key ? null : key
      if (next && wsPath) {
        agentClient.setActiveProject(wsPath)
      }
      return next
    })
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

  // 全局所有会话索引
  const allThreadMap = useMemo(() => {
    const map = new Map<string, Thread>()
    for (const t of threads) {
      map.set(t.id, t)
    }
    return map
  }, [threads])

  // 将会话按工作区树形归纳分组，并在工作区内按父子关系构建多级树形会话列表（三级紧凑展示）
  const workspaceGroups = useMemo(() => {
    const groupMap = new Map<
      string,
      {
        rawWorkspace: string
        dirName: string
        threads: Thread[]
      }
    >()

    // 归组所有会话
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

    const childrenMap = new Map<string, Thread[]>()
    for (const t of threads) {
      if (t.parentId && allThreadMap.has(t.parentId)) {
        const list = childrenMap.get(t.parentId) || []
        list.push(t)
        childrenMap.set(t.parentId, list)
      }
    }

    function buildTreeNode(thread: Thread, level: number, visited: Set<string>): TreeNode {
      visited.add(thread.id)
      const rawChildren = childrenMap.get(thread.id) || []
      const sortedChildren = [...rawChildren].sort((a, b) => {
        const timeA = a.createdAt || a.updatedAt || 0
        const timeB = b.createdAt || b.updatedAt || 0
        if (timeA !== timeB) return timeA - timeB
        return a.id.localeCompare(b.id)
      })

      const children: TreeNode[] = []
      for (const child of sortedChildren) {
        if (!visited.has(child.id)) {
          children.push(buildTreeNode(child, level + 1, visited))
        }
      }

      return {
        thread,
        level,
        children,
      }
    }

    return Array.from(groupMap.entries()).map(([key, group]) => {
      const sorted = [...group.threads].sort((a, b) => {
        const timeA = a.createdAt || a.updatedAt || 0
        const timeB = b.createdAt || b.updatedAt || 0
        if (timeB !== timeA) return timeB - timeA
        return a.id.localeCompare(b.id)
      })

      const rootThreads = sorted.filter((t) => !(t.parentId && allThreadMap.has(t.parentId)))

      const visited = new Set<string>()
      const rawNodes = rootThreads.map((rt) => buildTreeNode(rt, 1, visited))

      const term = search.trim().toLowerCase()
      const filteredNodes = term
        ? rawNodes
            .map((node) => filterTreeNode(node, term))
            .filter((n): n is TreeNode => n !== null)
        : rawNodes

      return {
        key,
        workspace: group.rawWorkspace,
        dirName: group.dirName,
        nodes: filteredNodes,
        totalCount: countTreeNodes(filteredNodes),
      }
    }).filter((g) => g.nodes.length > 0 || normalizePathKey(g.workspace) === normalizePathKey(activeWorkspace))
  }, [threads, activeWorkspace, search, allThreadMap])

  const currentThreadWorkspace =
    threads.find((t) => t.id === activeThreadId)?.workspace || activeWorkspace

  // 极简折叠模式
  if (collapsed) {
    return (
      <aside className="w-12 bg-zinc-50 dark:bg-[#121214] border-r border-zinc-200 dark:border-[#27272a] flex flex-col items-center py-2.5 flex-shrink-0 select-none transition-colors duration-100">
        <button
          onClick={() => setCollapsed(false)}
          className="p-1.5 text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded-lg transition-colors mb-2.5 cursor-pointer"
          title="展开工作区树"
        >
          <ChevronRight size={15} />
        </button>
        <button
          onClick={() => onCreateThread(currentThreadWorkspace, mode)}
          className={`p-1.5 ${mode === 'pm' ? 'bg-amber-600 hover:bg-amber-500' : 'bg-blue-600 hover:bg-blue-500'} text-white rounded-lg transition-colors shadow-2xs mb-3 cursor-pointer`}
          title="新建对话"
        >
          <Plus size={15} />
        </button>
      </aside>
    )
  }

  return (
    <aside
      style={{ width: `${width}px` }}
      className="relative bg-zinc-50 dark:bg-[#121214] border-r border-zinc-200 dark:border-[#27272a] flex flex-col h-full flex-shrink-0 select-none transition-colors duration-100"
    >
      {/* 顶部：新建对话、打开工作区与折叠（紧凑间距） */}
      <div className="p-2 border-b border-zinc-200 dark:border-[#27272a]/60 flex items-center justify-between gap-1">
        <button
          onClick={() => onCreateThread(currentThreadWorkspace, mode)}
          className={`flex-1 flex items-center justify-center space-x-1 py-1 px-2 ${
            mode === 'pm'
              ? 'bg-amber-600 hover:bg-amber-500 active:bg-amber-700'
              : 'bg-blue-600 hover:bg-blue-500 active:bg-blue-700'
          } text-white text-[11.5px] font-medium rounded-md transition-colors shadow-2xs cursor-pointer`}
          title="在所属工作区新建对话"
        >
          <Plus size={13} />
          <span>新建对话</span>
        </button>

        {onOpenWorkspacePicker && (
          <button
            onClick={onOpenWorkspacePicker}
            className="p-1 text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors cursor-pointer"
            title="选择并打开工作区目录"
          >
            <FolderOpen size={14} />
          </button>
        )}

        <button
          onClick={() => setCollapsed(true)}
          className="p-1 text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors cursor-pointer"
          title="折叠侧边栏"
        >
          <ChevronLeft size={15} />
        </button>
      </div>

      {/* 紧凑搜索框 */}
      <div className="px-2 py-1.5 border-b border-zinc-200/80 dark:border-[#27272a]/60">
        <div className="relative flex items-center">
          <Search size={11} className="absolute left-2 text-zinc-400 dark:text-zinc-500 pointer-events-none" />
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="搜索工作区或会话..."
            className="w-full bg-white dark:bg-[#1e1e22] text-zinc-900 dark:text-zinc-200 placeholder-zinc-400 dark:placeholder-zinc-500 text-[11px] rounded pl-6 pr-2 py-1 outline-none border border-zinc-200 dark:border-transparent focus:border-blue-500/50 shadow-2xs dark:shadow-none transition-colors"
          />
        </div>
      </div>

      {/* 紧凑工作区 + 会话 树形展示列表 */}
      <div className="flex-1 overflow-y-auto px-1.5 py-1 space-y-1 [scrollbar-gutter:stable]">
        {workspaceGroups.length === 0 ? (
          <div className="text-center py-6 text-xs text-zinc-400 dark:text-zinc-600">
            暂无工作区记录
          </div>
        ) : (
          workspaceGroups.map((group) => {
            const isFolded = search.trim() ? false : expandedWorkspaceKey !== group.key

            if (search.trim() && group.nodes.length === 0) {
              return null
            }

            return (
              <div key={group.key} className="space-y-0.5">
                {/* 树节点：工作区分组头部（紧凑） */}
                <div
                  onClick={() => {
                    toggleWorkspace(group.key, group.workspace)
                    // 若当前激活会话不属于此工作区，将视图即时定位到该工作区
                    const curWsNorm = normalizePathKey(activeWorkspace)
                    const targetWsNorm = normalizePathKey(group.workspace)
                    if (curWsNorm !== targetWsNorm) {
                      agentClient.setActiveProject(group.workspace)
                      if (group.nodes.length > 0) {
                        onSelectThread(group.nodes[0].thread.id)
                      } else {
                        onCreateThread(group.workspace, mode)
                      }
                    }
                  }}
                  className="group flex items-center justify-between px-1.5 py-0.5 min-h-[24px] rounded text-[11.5px] cursor-pointer hover:bg-zinc-200/50 dark:hover:bg-zinc-800/40 transition-colors"
                  title={group.workspace}
                >
                  <div className="flex items-center space-x-1 truncate flex-1 min-w-0">
                    <span className="text-zinc-400 dark:text-zinc-500 flex-shrink-0">
                      {isFolded ? <ChevronRight size={11} /> : <ChevronDown size={11} />}
                    </span>

                    {isFolded ? (
                      <FolderGit2 size={12} className="text-zinc-400 dark:text-zinc-500 flex-shrink-0" />
                    ) : (
                      <FolderOpen size={12} className="text-blue-500 flex-shrink-0" />
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
                        agentClient.setActiveProject(group.workspace)
                        onCreateThread(group.workspace, mode)
                      }}
                      className="p-0.5 text-zinc-400 hover:text-blue-600 dark:hover:text-blue-400 hover:bg-zinc-200 dark:hover:bg-zinc-700 rounded transition-colors cursor-pointer"
                      title="在此工作区新建对话"
                    >
                      <Plus size={11} />
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
                        <Trash2 size={11} />
                      </button>
                    )}

                    <span className="text-[9px] px-1 py-0 rounded-full font-mono text-zinc-400 dark:text-zinc-500 bg-zinc-200/60 dark:bg-zinc-800/60 leading-tight">
                      {group.totalCount}
                    </span>
                  </div>
                </div>

                {/* 树叶节点：会话列表 */}
                {!isFolded && (
                  <div className="ml-2.5 pl-1.5 border-l border-zinc-200/80 dark:border-zinc-800 space-y-0.5 pt-0.5">
                    {group.nodes.length === 0 ? (
                      <div className="px-1.5 py-0.5 text-[10.5px] text-zinc-400 dark:text-zinc-500 italic">
                        暂无会话记录
                      </div>
                    ) : (
                      group.nodes.map((node) => (
                        <SidebarTreeItem
                          key={node.thread.id}
                          node={node}
                          rootThread={node.thread}
                          activeThreadId={activeThreadId}
                          runningThreadIds={runningThreadIds}
                          allThreadMap={allThreadMap}
                          expandedParents={expandedParents}
                          editingId={editingId}
                          editTitle={editTitle}
                          onToggleParent={toggleParent}
                          onSelectThread={onSelectThread}
                          onDeleteThread={onDeleteThread}
                          onExportThread={handleExportThread}
                          setEditingId={setEditingId}
                          setEditTitle={setEditTitle}
                        />
                      ))
                    )}
                  </div>
                )}
              </div>
            )
          })
        )}
      </div>

      {/* 底部统计栏（紧凑） */}
      <div className="p-2 border-t border-zinc-200 dark:border-[#27272a] text-[10px] text-zinc-400 dark:text-zinc-500 flex items-center justify-between">
        <span>{workspaceGroups.length} 工作区 · {threads.length} 会话</span>
        <span className="text-[9px] text-zinc-400 dark:text-zinc-600">Tauri v2</span>
      </div>

      {/* 拖拽调整侧边栏宽度的分割线手柄 */}
      <div
        onMouseDown={handleMouseDown}
        onDoubleClick={handleResetWidth}
        className={`absolute top-0 -right-1 w-2 h-full cursor-col-resize z-20 group transition-colors ${
          isResizing ? 'bg-blue-500/80' : 'hover:bg-blue-500/30'
        }`}
        title="拖动调整侧边栏宽度，双击恢复默认宽度"
      >
        <div
          className={`w-[1px] h-full mx-auto ${
            isResizing ? 'bg-blue-500' : 'group-hover:bg-blue-400'
          } transition-colors`}
        />
      </div>
    </aside>
  )
}

export const Sidebar = memo(SidebarComponent, (prev, next) => {
  if (prev.activeThreadId !== next.activeThreadId) return false
  if (prev.activeWorkspace !== next.activeWorkspace) return false
  if (prev.currentMode !== next.currentMode) return false
  if ((prev.runningThreadIds?.length || 0) !== (next.runningThreadIds?.length || 0)) return false
  if (prev.runningThreadIds?.some((id, idx) => id !== next.runningThreadIds?.[idx])) return false
  if (prev.threads.length !== next.threads.length) return false

  for (let i = 0; i < prev.threads.length; i++) {
    const pt = prev.threads[i]
    const nt = next.threads[i]
    if (
      pt.id !== nt.id ||
      pt.title !== nt.title ||
      pt.workspace !== nt.workspace ||
      pt.isSubagent !== nt.isSubagent ||
      pt.parentId !== nt.parentId ||
      pt.items.length !== nt.items.length
    ) {
      return false
    }
  }

  return true
})
