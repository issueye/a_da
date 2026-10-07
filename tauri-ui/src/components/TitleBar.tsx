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
} from 'lucide-react'
import type { AgentMode } from '../types'

interface TitleBarProps {
  title: string
  workspace?: string
  mode: AgentMode
  connected: boolean
  isDark: boolean
  onToggleTheme: () => void
  onOpenSettings: () => void
  onOpenPlugins: () => void
  onOpenCommandPalette?: () => void
  onOpenChanges?: () => void
  activeChangeCount?: number
}

export const TitleBar: React.FC<TitleBarProps> = ({
  title,
  workspace,
  mode,
  connected,
  isDark,
  onToggleTheme,
  onOpenSettings,
  onOpenPlugins,
  onOpenCommandPalette,
  onOpenChanges,
  activeChangeCount = 0,
}) => {
  const handleMinimize = async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      getCurrentWindow().minimize()
    } catch {
      console.log('Minimize window (browser mode)')
    }
  }

  const handleMaximize = async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      getCurrentWindow().toggleMaximize()
    } catch {
      console.log('Maximize window (browser mode)')
    }
  }

  const handleClose = async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      getCurrentWindow().close()
    } catch {
      console.log('Close window (browser mode)')
    }
  }

  const modeBadge = {
    code: { label: '编码', color: 'bg-blue-500/20 text-blue-400 border-blue-500/30' },
    plan: { label: '规划', color: 'bg-purple-500/20 text-purple-400 border-purple-500/30' },
    create: { label: '创造', color: 'bg-emerald-500/20 text-emerald-400 border-emerald-500/30' },
  }[mode]

  return (
    <header
      data-tauri-drag-region
      className="h-10 border-b border-zinc-200 dark:border-[#27272a] bg-zinc-50/95 dark:bg-[#18181b]/95 backdrop-blur flex items-center justify-between px-3 text-xs select-none z-50 flex-shrink-0 transition-colors"
    >
      {/* 左侧：Logo与标题 */}
      <div className="flex items-center space-x-2.5 pointer-events-none">
        <div className="flex items-center space-x-1.5 font-bold tracking-wide text-zinc-900 dark:text-zinc-100">
          <div className="w-5 h-5 rounded-md bg-blue-600 flex items-center justify-center text-white text-[11px] font-black shadow-sm">
            <Terminal size={12} strokeWidth={2.5} />
          </div>
          <span className="text-sm font-semibold tracking-tight">a_da</span>
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
      </div>

      {/* 中间：连接状态 */}
      <div className="flex items-center space-x-1.5 pointer-events-none">
        <Circle
          size={7}
          className={`${connected ? 'fill-emerald-500 text-emerald-500' : 'fill-rose-500 text-rose-500'} animate-pulse`}
        />
        <span className="text-[11px] text-zinc-500 dark:text-zinc-400">
          {connected ? '核心在线' : '等待核心连接...'}
        </span>
      </div>

      {/* 右侧：按钮群组 */}
      <div className="flex items-center space-x-1" data-tauri-drag-region={false}>
        {onOpenCommandPalette && (
          <button
            onClick={onOpenCommandPalette}
            title="命令面板 (Ctrl+K)"
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
          className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
        >
          <Puzzle size={14} />
        </button>

        <button
          onClick={onOpenSettings}
          title="设置中心"
          className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
        >
          <Settings size={14} />
        </button>

        <button
          onClick={onToggleTheme}
          title={isDark ? '切换至亮色模式' : '切换至暗色模式'}
          className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
        >
          {isDark ? <Sun size={14} /> : <Moon size={14} />}
        </button>

        <div className="w-[1px] h-3.5 bg-zinc-300 dark:bg-zinc-700 mx-1" />

        {/* 窗口控制按钮 */}
        <button
          onClick={handleMinimize}
          className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
          title="最小化"
        >
          <Minus size={13} />
        </button>
        <button
          onClick={handleMaximize}
          className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded transition-colors"
          title="最大化"
        >
          <Square size={12} />
        </button>
        <button
          onClick={handleClose}
          className="p-1.5 text-zinc-600 dark:text-zinc-400 hover:text-white hover:bg-rose-600 rounded transition-colors"
          title="关闭"
        >
          <X size={14} />
        </button>
      </div>
    </header>
  )
}
