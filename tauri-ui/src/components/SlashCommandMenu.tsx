import React, { useState, useEffect, useRef } from 'react'
import {
  Slash,
  Sparkles,
  Settings,
  Puzzle,
  History,
  RotateCcw,
  Compass,
  Zap,
  Globe,
  FileText,
  Minimize2,
} from 'lucide-react'
import type { PromptItem } from '../types'
import { agentClient } from '../client/ws-client'

export interface SlashCommandItem {
  id: string
  name: string
  command: string
  description: string
  category: 'system' | 'prompt'
  icon: any
  action?: () => void
}

interface SlashCommandMenuProps {
  filterQuery: string
  onSelect: (item: SlashCommandItem) => void
  onClose: () => void
  onOpenSettings: () => void
  onOpenPlugins: () => void
  onOpenChanges: () => void
  onCompact: () => void
  onNewThread: () => void
  onSetMode: (mode: 'code' | 'plan' | 'create') => void
}

export const SlashCommandMenu: React.FC<SlashCommandMenuProps> = ({
  filterQuery,
  onSelect,
  onClose,
  onOpenSettings,
  onOpenPlugins,
  onOpenChanges,
  onCompact,
  onNewThread,
  onSetMode,
}) => {
  const [prompts, setPrompts] = useState<PromptItem[]>([])
  const [selectedIndex, setSelectedIndex] = useState(0)
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const res = await agentClient.fetchPrompts()
        if (!cancelled && res) {
          setPrompts(res)
        }
      } catch {}
    })()
    return () => {
      cancelled = true
    }
  }, [])

  const systemCommands: SlashCommandItem[] = [
    {
      id: 'sys-clear',
      name: 'clear',
      command: '/clear',
      description: '清空当前上下文并新建会话',
      category: 'system',
      icon: RotateCcw,
      action: onNewThread,
    },
    {
      id: 'sys-compact',
      name: 'compact',
      command: '/compact',
      description: '压缩当前会话历史并提取持久化上下文摘要',
      category: 'system',
      icon: Minimize2,
      action: onCompact,
    },
    {
      id: 'sys-plan',
      name: 'plan',
      command: '/plan',
      description: '切换至 Plan 只读架构规划与设计模式',
      category: 'system',
      icon: Compass,
      action: () => onSetMode('plan'),
    },
    {
      id: 'sys-changes',
      name: 'changes',
      command: '/changes',
      description: '审查当前会话修改的文件列表与 Diff',
      category: 'system',
      icon: History,
      action: onOpenChanges,
    },
    {
      id: 'sys-goal',
      name: 'goal',
      command: '/goal',
      description: '执行长期攻坚深度攻关任务',
      category: 'system',
      icon: Zap,
    },
    {
      id: 'sys-boost',
      name: 'boost',
      command: '/boost',
      description: '多视角严谨审视与验证代码方案',
      category: 'system',
      icon: Sparkles,
    },
    {
      id: 'sys-browser',
      name: 'browser',
      command: '/browser',
      description: '检索网络资料或网页抓取分析',
      category: 'system',
      icon: Globe,
    },
    {
      id: 'sys-settings',
      name: 'settings',
      command: '/settings',
      description: '打开模型供应商与连接参数设置',
      category: 'system',
      icon: Settings,
      action: onOpenSettings,
    },
    {
      id: 'sys-plugins',
      name: 'plugins',
      command: '/plugins',
      description: '打开插件中心、能力矩阵与技能管理',
      category: 'system',
      icon: Puzzle,
      action: onOpenPlugins,
    },
  ]

  const promptCommands: SlashCommandItem[] = prompts.map((p) => ({
    id: `prompt:${p.id}`,
    name: p.name,
    command: `/${p.name.replace(/\s+/g, '-').toLowerCase()}`,
    description: p.description || p.content.slice(0, 50),
    category: 'prompt',
    icon: FileText,
  }))

  const allCommands = [...systemCommands, ...promptCommands]

  const filtered = allCommands.filter((cmd) => {
    const q = filterQuery.trim().toLowerCase().replace(/^\//, '')
    if (!q) return true
    return (
      cmd.command.toLowerCase().includes(q) ||
      cmd.name.toLowerCase().includes(q) ||
      cmd.description.toLowerCase().includes(q)
    )
  })

  useEffect(() => {
    setSelectedIndex(0)
  }, [filterQuery])

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setSelectedIndex((prev) => (prev + 1) % Math.max(1, filtered.length))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setSelectedIndex((prev) => (prev - 1 + filtered.length) % Math.max(1, filtered.length))
      } else if (e.key === 'Enter') {
        if (filtered[selectedIndex]) {
          e.preventDefault()
          onSelect(filtered[selectedIndex])
        }
      } else if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      }
    }

    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [filtered, selectedIndex, onSelect, onClose])

  if (filtered.length === 0) return null

  return (
    <div
      ref={menuRef}
      className="absolute bottom-full left-0 mb-2 w-80 bg-white dark:bg-[#1f1f23] border border-zinc-200 dark:border-[#333338] rounded-2xl shadow-2xl overflow-hidden z-50 flex flex-col animate-in fade-in slide-in-from-bottom-2 duration-150 backdrop-blur-md select-none"
    >
      <div className="px-3 py-2 border-b border-zinc-100 dark:border-[#2b2b30] bg-zinc-50 dark:bg-[#18181c] text-[10px] font-semibold text-zinc-400 dark:text-zinc-500 uppercase tracking-wider">
        快捷指令与提示词模板
      </div>

      <div className="max-h-64 overflow-y-auto p-1.5 space-y-0.5">
        {filtered.map((item, idx) => {
          const isSelected = idx === selectedIndex
          const IconComp = item.icon

          return (
            <div
              key={item.id}
              onClick={() => onSelect(item)}
              onMouseEnter={() => setSelectedIndex(idx)}
              className={`flex items-center space-x-2.5 px-2.5 py-1.5 rounded-xl cursor-pointer transition-colors ${
                isSelected
                  ? 'bg-blue-600 text-white'
                  : 'text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800'
              }`}
            >
              <IconComp
                size={14}
                className={isSelected ? 'text-white' : 'text-blue-500 flex-shrink-0'}
              />

              <div className="flex flex-col min-w-0 flex-1">
                <div className="flex items-center space-x-1.5">
                  <span className="text-xs font-mono font-semibold">{item.command}</span>
                  <span
                    className={`text-[10px] ${
                      isSelected ? 'text-blue-100' : 'text-zinc-400 dark:text-zinc-500'
                    }`}
                  >
                    ({item.name})
                  </span>
                </div>
                <span
                  className={`text-[10px] truncate ${
                    isSelected ? 'text-blue-100' : 'text-zinc-400 dark:text-zinc-500'
                  }`}
                >
                  {item.description}
                </span>
              </div>

              <span
                className={`text-[9.5px] px-1 py-0.2 rounded font-mono ${
                  isSelected ? 'bg-blue-700 text-blue-100' : 'bg-zinc-100 dark:bg-zinc-800 text-zinc-400'
                }`}
              >
                {item.category === 'system' ? '系统' : '模板'}
              </span>
            </div>
          )
        })}
      </div>
    </div>
  )
}
