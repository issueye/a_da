import React, { useState, useEffect, useRef } from 'react'
import {
  FileCode,
  Sparkles,
  Bot,
  Search,
  Code2,
  Image as ImageIcon,
  FileText,
  Filter,
} from 'lucide-react'
import type { SkillSummary, SubagentProfile } from '../types'
import { agentClient } from '../client/ws-client'

export interface MentionItem {
  id: string
  title: string
  subtitle?: string
  category: 'file' | 'skill' | 'subagent'
  insertText: string
}

interface MentionMenuProps {
  filterQuery: string
  onSelect: (item: MentionItem) => void
  onClose: () => void
}

function getFileIcon(path: string) {
  const lower = path.toLowerCase()
  if (/\.(ts|tsx|js|jsx|rs|py|go|c|cpp|h|java|json|toml|yaml|yml)$/.test(lower)) {
    return <Code2 size={13} className="text-cyan-500" />
  }
  if (/\.(png|jpe?g|gif|svg|webp|ico)$/.test(lower)) {
    return <ImageIcon size={13} className="text-indigo-500" />
  }
  return <FileText size={13} className="text-blue-500" />
}

export const MentionMenu: React.FC<MentionMenuProps> = ({ filterQuery, onSelect, onClose }) => {
  const [category, setCategory] = useState<'all' | 'file' | 'skill' | 'subagent'>('all')
  const [items, setItems] = useState<MentionItem[]>([])
  const [selectedIndex, setSelectedIndex] = useState(0)
  const [loading, setLoading] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let cancelled = false
    void (async () => {
      setLoading(true)
      try {
        const [entries, skills, subagents] = await Promise.all([
          agentClient.fetchWorkspaceEntries().catch(() => []),
          agentClient.fetchSkills().catch(() => []),
          agentClient.fetchSubagentProfiles().catch(() => []),
        ])

        if (cancelled) return

        const fileItems: MentionItem[] = (entries || []).map((p) => ({
          id: `file:${p}`,
          title: p.split(/[\\/]/).pop() || p,
          subtitle: p,
          category: 'file',
          insertText: `[文件: ${p}]`,
        }))

        const skillItems: MentionItem[] = (skills || []).map((s) => ({
          id: `skill:${s.id}`,
          title: s.name,
          subtitle: s.description,
          category: 'skill',
          insertText: `@skill:${s.id}`,
        }))

        const subagentItems: MentionItem[] = (subagents || []).map((sub) => ({
          id: `subagent:${sub.id}`,
          title: sub.name,
          subtitle: sub.role || sub.description,
          category: 'subagent',
          insertText: `@subagent:${sub.id}`,
        }))

        setItems([...fileItems, ...skillItems, ...subagentItems])
      } finally {
        setLoading(false)
      }
    })()

    return () => {
      cancelled = true
    }
  }, [])

  const filtered = items.filter((item) => {
    if (category !== 'all' && item.category !== category) return false
    const q = filterQuery.trim().toLowerCase()
    if (!q) return true
    return (
      item.title.toLowerCase().includes(q) ||
      (item.subtitle && item.subtitle.toLowerCase().includes(q)) ||
      item.insertText.toLowerCase().includes(q)
    )
  })

  useEffect(() => {
    setSelectedIndex(0)
  }, [filterQuery, category])

  // 监听键盘按键
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

  if (filtered.length === 0 && !loading) return null

  return (
    <div
      ref={menuRef}
      className="absolute bottom-full left-0 mb-2 w-80 bg-white dark:bg-[#1f1f23] border border-zinc-200 dark:border-[#333338] rounded-2xl shadow-2xl overflow-hidden z-50 flex flex-col animate-in fade-in slide-in-from-bottom-2 duration-150 backdrop-blur-md select-none"
    >
      {/* 头部分类切换药丸 */}
      <div className="flex items-center space-x-1 p-2 border-b border-zinc-100 dark:border-[#2b2b30] bg-zinc-50 dark:bg-[#18181c] text-[11px]">
        {(['all', 'file', 'skill', 'subagent'] as const).map((cat) => {
          const isSelected = category === cat
          const label =
            cat === 'all'
              ? '全部'
              : cat === 'file'
              ? '文件'
              : cat === 'skill'
              ? '技能'
              : '智能体'

          return (
            <button
              key={cat}
              type="button"
              onClick={() => setCategory(cat)}
              className={`px-2 py-0.5 rounded-lg transition-colors font-medium cursor-pointer ${
                isSelected
                  ? 'bg-blue-600 text-white shadow-xs'
                  : 'text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200 hover:bg-zinc-200/60 dark:hover:bg-zinc-800'
              }`}
            >
              {label}
            </button>
          )
        })}
      </div>

      {/* 匹配列表 */}
      <div className="max-h-64 overflow-y-auto p-1.5 space-y-0.5">
        {loading && items.length === 0 ? (
          <div className="py-6 text-center text-xs text-zinc-400">正在搜索可提及资源...</div>
        ) : filtered.length === 0 ? (
          <div className="py-6 text-center text-xs text-zinc-400">无匹配结果</div>
        ) : (
          filtered.slice(0, 30).map((item, idx) => {
            const isSelected = idx === selectedIndex

            return (
              <div
                key={item.id}
                onClick={() => onSelect(item)}
                onMouseEnter={() => setSelectedIndex(idx)}
                className={`flex items-center space-x-2 px-2.5 py-1.5 rounded-xl cursor-pointer transition-colors ${
                  isSelected
                    ? 'bg-blue-600 text-white'
                    : 'text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                }`}
              >
                <div className="flex-shrink-0">
                  {item.category === 'file' ? (
                    getFileIcon(item.subtitle || item.title)
                  ) : item.category === 'skill' ? (
                    <Sparkles size={13} className={isSelected ? 'text-white' : 'text-amber-500'} />
                  ) : (
                    <Bot size={13} className={isSelected ? 'text-white' : 'text-purple-500'} />
                  )}
                </div>

                <div className="flex flex-col min-w-0 flex-1">
                  <span className="text-xs font-mono font-medium truncate">{item.title}</span>
                  {item.subtitle && (
                    <span
                      className={`text-[10px] truncate ${
                        isSelected ? 'text-blue-100' : 'text-zinc-400 dark:text-zinc-500'
                      }`}
                    >
                      {item.subtitle}
                    </span>
                  )}
                </div>

                <span
                  className={`text-[9.5px] px-1 py-0.2 rounded font-mono ${
                    isSelected
                      ? 'bg-blue-700 text-blue-100'
                      : 'bg-zinc-100 dark:bg-zinc-800 text-zinc-400'
                  }`}
                >
                  {item.category === 'file' ? '文件' : item.category === 'skill' ? '技能' : '代理'}
                </span>
              </div>
            )
          })
        )}
      </div>
    </div>
  )
}
