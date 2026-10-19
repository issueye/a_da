import React from 'react'
import {
  Minus,
  Square,
  X,
  Sun,
  Moon,
  Settings,
  Puzzle,
  Terminal,
  Circle,
  Command,
  History,
  Activity,
  Copy,
} from 'lucide-react'
import type { AgentMode, ApprovalMode } from '../types'
import { agentClient } from '../client/ws-client'
import {
  windowMinimize,
  windowMaximize,
  windowClose,
  isWindowMaximized,
  onWindowMaximizeChange,
  isElectronShell,
} from '../client/desktop-bridge'

interface TitleBarProps {
  title: string
  workspace?: string
  productName?: string
  mode: AgentMode
  connected: boolean
  isDark: boolean
  onToggleTheme: () => void
  onOpenSettings: () => void
  onOpenPlugins: () => void
  onOpenProcesses?: () => void
  onOpenCommandPalette?: () => void
  onOpenChanges?: () => void
  onReconnect?: () => void
  activeChangeCount?: number
  /** 当前审批档位（只在「完全访问」时用于常驻指示，P1b）。 */
  approvalMode?: ApprovalMode
}

export const TitleBar: React.FC<TitleBarProps> = ({
  title,
  workspace,
  productName,
  mode,
  connected,
  isDark,
  onToggleTheme,
  onOpenSettings,
  onOpenPlugins,
  onOpenProcesses,
  onOpenCommandPalette,
  onOpenChanges,
  onReconnect,
  activeChangeCount = 0,
  approvalMode,
}) => {
  const [isMax, setIsMax] = React.useState(false)

  React.useEffect(() => {
    isWindowMaximized().then(setIsMax)
    const unsubscribe = onWindowMaximizeChange(setIsMax)
    return () => {
      unsubscribe()
    }
  }, [])

  // 每次动作后回读状态：宿主事件是主通道，这里兜一手，保证按钮图标不漂移
  const syncMaxState = React.useCallback(() => {
    isWindowMaximized().then(setIsMax)
  }, [])

  const handleMinimize = () => windowMinimize()
  const handleMaximize = () => {
    windowMaximize().then(syncMaxState)
  }
  const handleClose = () => windowClose()

  const modeBadge = {
    code: { label: '编码', color: 'bg-blue-500/20 text-blue-400 border-blue-500/30' },
    pm: { label: 'PM', color: 'bg-amber-500/20 text-amber-500 border-amber-500/30' },
    plan: { label: '规划', color: 'bg-purple-500/20 text-purple-400 border-purple-500/30' },
    create: { label: '创造', color: 'bg-emerald-500/20 text-emerald-400 border-emerald-500/30' },
  }[mode] || { label: '编码', color: 'bg-blue-500/20 text-blue-400 border-blue-500/30' }

  // 产品只有一个（ada-coding）；PM 由 `mode` 表达，不再改产品名
  const displayProduct = productName || 'ada-coding'

  return (
    <header
      // `deep`：标题栏**整块**都可拖拽；双击空白处由 Electron 宿主原生切换最大化/还原
      // （前端不再自己 onDoubleClick，两边都处理会连按两次，等于什么都没发生）。
      data-app-drag-region="deep"
      // 🔴 无边框窗口的标题栏是**窗口 chrome**，必须永远压在所有应用模态层之上
      // （应用里的弹窗统一是 `fixed inset-0 z-50`）。少了这一层，任何弹窗打开时
      // 它都会被盖住：最小化/最大化/关闭点不动、窗口也拖不动——正是"无法最小化/关闭窗口"。
      className="relative z-[100] h-10 border-b border-zinc-200 dark:border-[#27272a] bg-zinc-50/95 dark:bg-[#18181b]/95 backdrop-blur flex items-center justify-between px-3 text-xs select-none flex-shrink-0 transition-colors"
    >
      {/* 左侧：Logo与标题 */}
      <div className="flex items-center space-x-2.5 pointer-events-none">
        <div className="flex items-center space-x-1.5 font-bold tracking-wide text-zinc-900 dark:text-zinc-100">
          <div className="w-5 h-5 rounded-md bg-blue-600 flex items-center justify-center text-white text-[11px] font-black shadow-sm">
            <Terminal size={12} strokeWidth={2.5} />
          </div>
          <span className="text-sm font-semibold tracking-tight">a_da</span>
          <span className="text-[10px] px-1.5 py-0.2 rounded font-mono font-medium bg-blue-500/10 text-blue-600 dark:text-blue-400 border border-blue-500/20">
            {displayProduct}
          </span>
        </div>

        {workspace && (
          <div className="flex items-center space-x-1 text-zinc-400 dark:text-zinc-500">
            <span className="text-zinc-300 dark:text-zinc-600">/</span>
            <span
              className="px-1.5 py-0.5 rounded text-[10px] font-mono text-zinc-600 dark:text-zinc-400 bg-zinc-200/50 dark:bg-zinc-800/60 border border-zinc-200/80 dark:border-zinc-700/60 truncate max-w-[130px]"
              title={`会话归属工作区: ${workspace}`}
            >
              {workspace.split(/[\\/]/).filter(Boolean).pop() || workspace}
            </span>
          </div>
        )}

        <div className="flex items-center space-x-1 text-zinc-400 dark:text-zinc-500">
          <span className="text-zinc-300 dark:text-zinc-600">/</span>
          <span className="truncate max-w-[200px] text-zinc-700 dark:text-zinc-300 font-medium">
            {title || '新对话'}
          </span>
        </div>

        <span
          className={`px-1.5 py-0.5 rounded text-[10px] font-medium border ${modeBadge.color}`}
        >
          {modeBadge.label}
        </span>

        {/* 完全访问档位的**第二处常驻指示**（P1b · SP-35）：标题栏在最上层，
            不管对话流滚到哪里、输入区是否被遮住，这一档都看得见。 */}
        {approvalMode === 'full' && (
          <span
            className="px-1.5 py-0.5 rounded text-[10px] font-semibold border bg-rose-500/15 text-rose-600 dark:text-rose-400 border-rose-500/30"
            title="完全访问：不再询问，可读写工作区之外的路径，脚本可联网（应用自身目录除外）"
          >
            完全访问
          </span>
        )}
      </div>

      {/* 中间：连接状态与运行模式（点击打开进程管理或重试） */}
      {/* 🔴 UI-08：这里原本是 `<div onClick>`——键盘聚焦不到、读屏也认不出它是按钮。
          连接态是可以点的入口（打开进程管理 / 重连），必须是真 button。 */}
      <button
        type="button"
        // 标题栏是 deep 拖拽区，这里必须显式退出拖拽，否则 mousedown 被拖拽吃掉、点不动
        data-app-drag-region="false"
        className="flex items-center space-x-1.5 px-2.5 py-1 rounded-md transition-all cursor-pointer hover:bg-zinc-200/80 dark:hover:bg-zinc-800 border border-transparent hover:border-zinc-300 dark:hover:border-zinc-700 pointer-events-auto group"
        aria-label={connected ? '网关在线，打开进程管理' : '未连接，打开进程管理'}
        onClick={() => {
          if (onOpenProcesses) {
            onOpenProcesses()
          } else if (!connected && onReconnect) {
            onReconnect()
          }
        }}
        title={
          !connected
            ? '服务连接断开，点击查看进程与服务状态'
            : '服务在线 · 模式：网关模式（点击打开进程管理）'
        }
      >
        <Circle
          size={7}
          className={`${connected ? 'fill-emerald-500 text-emerald-500' : 'fill-rose-500 text-rose-500'} animate-pulse`}
        />
        <span className="text-[11px] text-zinc-600 dark:text-zinc-300 font-medium">
          {connected ? '网关在线' : '未连接 (点击查看进程)'}
        </span>
        <Activity size={12} className="text-zinc-400 group-hover:text-blue-500 transition-colors ml-0.5" />
      </button>

      {/* 右侧：按钮群组 */}
      <div className="flex items-center space-x-1" data-app-drag-region={false}>
        {onOpenCommandPalette && (
          <button
            onClick={onOpenCommandPalette}
            title="命令面板 (Ctrl+K)"
            aria-label="命令面板"
            className="flex items-center space-x-1 px-2 py-1 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors text-[11px] font-medium"
          >
            <Command size={12} />
            <span className="font-mono text-[10px]">Ctrl+K</span>
          </button>
        )}

        {onOpenChanges && (
          <button
            onClick={onOpenChanges}
            title="改动审查面板 (Ctrl+Shift+C)"
            aria-label={`改动审查${activeChangeCount > 0 ? `，${activeChangeCount} 个文件待处理` : ''}`}
            className={`flex items-center space-x-1 px-1.5 py-1 rounded transition-colors text-[11px] font-medium ${
              activeChangeCount > 0
                ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400 hover:bg-blue-500/20'
                : 'text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800'
            }`}
          >
            <History size={13} className={activeChangeCount > 0 ? 'text-blue-500' : ''} />
            {activeChangeCount > 0 && <span className="text-[10px]">({activeChangeCount})</span>}
          </button>
        )}

        <button
          onClick={onOpenPlugins}
          title="插件与技能扩展 (Ctrl+Shift+X)"
          aria-label="插件与技能扩展"
          className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
        >
          <Puzzle size={14} />
        </button>

        {onOpenProcesses && (
          <button
            onClick={onOpenProcesses}
            title="进程管理与服务监控"
            aria-label="进程管理与服务监控"
            className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
          >
            <Activity size={14} />
          </button>
        )}

        <button
          onClick={onOpenSettings}
          title="设置中心"
          aria-label="设置中心"
          className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
        >
          <Settings size={14} />
        </button>

        <button
          onClick={onToggleTheme}
          title={isDark ? '切换至亮色模式' : '切换至暗色模式'}
          aria-label={isDark ? '切换至亮色模式' : '切换至暗色模式'}
          className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
        >
          {isDark ? <Sun size={14} /> : <Moon size={14} />}
        </button>

        {/* 窗口控制按钮：**只有 Electron 宿主才渲染**。
            浏览器里这三个动作在 `desktop-bridge` 里只是 console.log——摆一排点不动的按钮，
            用户只会以为页面卡住了。网页访问时浏览器自己就有标题栏，不需要我们来画。 */}
        {isElectronShell() && (
          <>
            <div className="w-[1px] h-3.5 bg-zinc-300 dark:bg-zinc-700 mx-1" />
            <button
              onClick={handleMinimize}
              className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
              title="最小化"
              aria-label="最小化"
            >
              <Minus size={13} />
            </button>
            <button
              onClick={handleMaximize}
              className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
              title={isMax ? '向下还原' : '最大化'}
              aria-label={isMax ? '向下还原' : '最大化'}
            >
              {isMax ? <Copy size={12} className="rotate-180" /> : <Square size={12} />}
            </button>
            <button
              onClick={handleClose}
              className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-white hover:bg-rose-600 rounded transition-colors"
              title="关闭窗口"
              aria-label="关闭窗口"
            >
              <X size={14} />
            </button>
          </>
        )}
      </div>
    </header>
  )
}
