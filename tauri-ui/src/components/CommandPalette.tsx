import React, { useState, useEffect, useRef } from 'react'
import {
  Search,
  Code2,
  Compass,
  Sparkles,
  Settings,
  Puzzle,
  History,
  Terminal,
  FolderOpen,
  Plus,
  Trash2,
  Sun,
  Moon,
  Minimize2,
} from 'lucide-react'
import type { AgentMode } from '../types'

export interface PaletteItem {
  id: string
  label: string
  hint?: string
  description?: string
  icon: any
  action: () => void
}

interface CommandPaletteProps {
  isOpen: boolean
  onClose: () => void
  currentMode: AgentMode
  onSelectMode: (mode: AgentMode) => void
  onNewThread: () => void
  onOpenSettings: () => void
  onOpenPlugins: () => void
  onOpenChanges: () => void
  onOpenDebug: () => void
  onOpenFilePicker: () => void
  onCompact: () => void
  onToggleTheme: () => void
  isDark: boolean
}

export const CommandPalette: React.FC<CommandPaletteProps> = ({
  isOpen,
  onClose,
  currentMode,
  onSelectMode,
  onNewThread,
  onOpenSettings,
  onOpenPlugins,
  onOpenChanges,
  onOpenDebug,
  onOpenFilePicker,
  onCompact,
  onToggleTheme,
  isDark,
}) => {
  const [query, setQuery] = useState('')
  const [selectedIndex, setSelectedIndex] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)

  const items: PaletteItem[] = [
    {
      id: 'thread-new',
      label: '新建会话',
      hint: 'Ctrl+N',
      description: '在当前工作区创建一个全新的交互会话',
      icon: Plus,
      action: onNewThread,
    },
    {
      id: 'mode-code',
      label: '切换到 Code 敏捷编码模式',
      description: currentMode === 'code' ? '全量工具开放（当前激活）' : '全量工具开放',
      icon: Code2,
      action: () => onSelectMode('code'),
    },
    {
      id: 'mode-plan',
      label: '切换到 Plan 只读规划模式',
      description: currentMode === 'plan' ? '严禁文件修改，只读探查与规划（当前激活）' : '严禁文件修改，只读探查与规划',
      icon: Compass,
      action: () => onSelectMode('plan'),
    },
    {
      id: 'mode-create',
      label: '切换到 Create 架构创造模式',
      description: currentMode === 'create' ? '智能体自我进化与工具扩展（当前激活）' : '智能体自我进化与工具扩展',
      icon: Sparkles,
      action: () => onSelectMode('create'),
    },
    {
      id: 'open-workspace',
      label: '打开工作区目录',
      description: '使用全平台文件浏览器选择并切换项目工程',
      icon: FolderOpen,
      action: onOpenFilePicker,
    },
    {
      id: 'open-changes',
      label: '打开改动审查面板',
      hint: 'Ctrl+Shift+C',
      description: '查看当前会话修改的文件、Diff 差异与一键回滚',
      icon: History,
      action: onOpenChanges,
    },
    {
      id: 'compact-context',
      label: '压缩上下文 (Compact)',
      description: '总结当前会话历史并提取持久化上下文摘要',
      icon: Minimize2,
      action: onCompact,
    },
    {
      id: 'open-settings',
      label: '打开设置中心',
      hint: 'Ctrl+,',
      description: '配置 AI 模型供应商、API Key 与上下文容量',
      icon: Settings,
      action: onOpenSettings,
    },
    {
      id: 'open-plugins',
      label: '打开插件与技能中心',
      hint: 'Ctrl+Shift+X',
      description: '管理内置与扩展插件、能力权限及自定义技能',
      icon: Puzzle,
      action: onOpenPlugins,
    },
    {
      id: 'open-debug',
      label: '打开通信与事件调试面板',
      hint: 'Ctrl+Shift+D',
      description: '实时观察底层 JSON-RPC 通信报文与 Token 遥测',
      icon: Terminal,
      action: onOpenDebug,
    },
    {
      id: 'toggle-theme',
      label: isDark ? '切换至亮色外观' : '切换至暗色外观',
      description: '切换界面明暗主题色彩',
      icon: isDark ? Sun : Moon,
      action: onToggleTheme,
    },
  ]

  const filtered = items.filter((item) => {
    const q = query.trim().toLowerCase()
    if (!q) return true
    return item.label.toLowerCase().includes(q) || (item.description || '').toLowerCase().includes(q)
  })

  useEffect(() => {
    if (isOpen) {
      setQuery('')
      setSelectedIndex(0)
      setTimeout(() => inputRef.current?.focus(), 50)
    }
  }, [isOpen])

  useEffect(() => {
    setSelectedIndex(0)
  }, [query])

  if (!isOpen) return null

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setSelectedIndex((prev) => (prev + 1) % Math.max(1, filtered.length))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setSelectedIndex((prev) => (prev - 1 + filtered.length) % Math.max(1, filtered.length))
    } else if (e.key === 'Enter') {
      e.preventDefault()
      if (filtered[selectedIndex]) {
        filtered[selectedIndex].action()
        onClose()
      }
    } else if (e.key === 'Escape') {
      onClose()
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-20 bg-black/50 backdrop-blur-xs select-none animate-in fade-in duration-100">
      <div
        className="w-full max-w-xl bg-white dark:bg-[#1c1c20] border border-zinc-200 dark:border-[#333338] rounded-2xl shadow-2xl overflow-hidden flex flex-col animate-in zoom-in-95 duration-150"
        onKeyDown={handleKeyDown}
      >
        {/* 输入框 */}
        <div className="flex items-center px-4 py-3 border-b border-zinc-200 dark:border-[#2b2b30] bg-zinc-50/50 dark:bg-[#18181c]/50">
          <Search size={16} className="text-zinc-400 mr-2.5 flex-shrink-0" />
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="输入指令名称进行搜索，回车执行，Esc 关闭..."
            className="w-full text-xs text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 bg-transparent outline-none"
          />
          <span className="text-[10px] text-zinc-400 dark:text-zinc-500 font-mono px-1.5 py-0.5 rounded border border-zinc-200 dark:border-zinc-700">
            Esc
          </span>
        </div>

        {/* 条目列表 */}
        <div className="max-h-80 overflow-y-auto p-1.5 space-y-0.5">
          {filtered.length === 0 ? (
            <div className="py-8 text-center text-xs text-zinc-400">未找到匹配的指令</div>
          ) : (
            filtered.map((item, idx) => {
              const IconComp = item.icon
              const isSelected = idx === selectedIndex

              return (
                <div
                  key={item.id}
                  onClick={() => {
                    item.action()
                    onClose()
                  }}
                  onMouseEnter={() => setSelectedIndex(idx)}
                  className={`flex items-center justify-between px-3 py-2 rounded-xl cursor-pointer transition-colors ${
                    isSelected
                      ? 'bg-blue-600 text-white'
                      : 'text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                  }`}
                >
                  <div className="flex items-center space-x-2.5 min-w-0 flex-1">
                    <IconComp
                      size={15}
                      className={isSelected ? 'text-white' : 'text-zinc-400 dark:text-zinc-500'}
                    />
                    <div className="flex flex-col min-w-0">
                      <span className="text-xs font-medium truncate">{item.label}</span>
                      {item.description && (
                        <span
                          className={`text-[10.5px] truncate ${
                            isSelected ? 'text-blue-100' : 'text-zinc-400 dark:text-zinc-500'
                          }`}
                        >
                          {item.description}
                        </span>
                      )}
                    </div>
                  </div>

                  {item.hint && (
                    <span
                      className={`text-[10px] font-mono ml-2 px-1.5 py-0.5 rounded ${
                        isSelected
                          ? 'bg-blue-700 text-blue-100'
                          : 'bg-zinc-100 dark:bg-zinc-800 text-zinc-500 dark:text-zinc-400 border border-zinc-200 dark:border-zinc-700'
                      }`}
                    >
                      {item.hint}
                    </span>
                  )}
                </div>
              )
            })
          )}
        </div>
      </div>
    </div>
  )
}
