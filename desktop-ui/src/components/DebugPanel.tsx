import React, { useState, useEffect } from 'react'
import {
  Terminal,
  X,
  Trash2,
  Copy,
  Check,
  Search,
  Filter,
  ChevronDown,
  ChevronRight,
  ArrowUpRight,
  ArrowDownLeft,
  AlertTriangle,
  Brain,
  Wrench,
} from 'lucide-react'
import type { DebugEntry, DebugKind } from '../types'
import { agentClient } from '../client/ws-client'

interface DebugPanelProps {
  isOpen: boolean
  onClose: () => void
}

function clock(at: number): string {
  const d = new Date(at)
  const pad = (n: number) => String(n).padStart(2, '0')
  const ms = String(d.getMilliseconds()).padStart(3, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${ms}`
}

export const DebugPanel: React.FC<DebugPanelProps> = ({ isOpen, onClose }) => {
  const [logs, setLogs] = useState<DebugEntry[]>([])
  const [selectedKind, setSelectedKind] = useState<DebugKind | 'all'>('all')
  const [search, setSearch] = useState('')
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [copiedId, setCopiedId] = useState<string | null>(null)

  useEffect(() => {
    if (isOpen) {
      const unsub = agentClient.subscribeDebug(setLogs)
      return unsub
    }
  }, [isOpen])

  if (!isOpen) return null

  const handleCopy = (id: string, text: string) => {
    navigator.clipboard.writeText(text)
    setCopiedId(id)
    setTimeout(() => setCopiedId(null), 1500)
  }

  const filtered = logs.filter((log) => {
    if (selectedKind !== 'all' && log.kind !== selectedKind) return false
    if (!search.trim()) return true
    const term = search.trim().toLowerCase()
    return (
      (log.method && log.method.toLowerCase().includes(term)) ||
      (log.error && log.error.toLowerCase().includes(term)) ||
      JSON.stringify(log.payload || '').toLowerCase().includes(term)
    )
  })

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs select-none animate-in fade-in duration-100">
      <div className="w-full max-w-4xl h-[80vh] bg-white dark:bg-[#18181b] border border-zinc-200 dark:border-[#303036] rounded-2xl shadow-2xl flex flex-col overflow-hidden animate-in zoom-in-95 duration-150">
        {/* 顶部标题栏 */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416]">
          <div className="flex items-center space-x-2">
            <Terminal size={16} className="text-blue-500" />
            <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
              通信与事件调试面板
            </span>
            <span className="px-1.5 py-0.2 rounded-full text-[10px] font-mono bg-blue-500/10 text-blue-600 dark:text-blue-400 border border-blue-500/20">
              {logs.length} 条记录
            </span>
          </div>

          <div className="flex items-center space-x-2">
            <button
              onClick={() => agentClient.clearDebugLogs()}
              className="flex items-center space-x-1 px-2.5 py-1 rounded-lg text-zinc-600 dark:text-zinc-400 hover:text-rose-500 hover:bg-zinc-200/60 dark:hover:bg-zinc-800 text-xs transition-colors cursor-pointer"
              title="清空所有记录"
            >
              <Trash2 size={12} />
              <span>清空</span>
            </button>

            <button
              onClick={onClose}
              className="p-1 rounded-lg text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200 dark:hover:bg-zinc-800 transition-colors cursor-pointer"
            >
              <X size={15} />
            </button>
          </div>
        </div>

        {/* 过滤与搜索工具栏 */}
        <div className="flex items-center justify-between px-3.5 py-2 border-b border-zinc-200 dark:border-[#27272a] bg-white dark:bg-[#1a1a1e] gap-2 text-xs">
          <div className="flex items-center space-x-1 flex-wrap gap-y-1">
            {(['all', 'request', 'response', 'tool', 'error', 'delta'] as const).map((k) => {
              const isSelected = selectedKind === k
              const label =
                k === 'all'
                  ? '全部'
                  : k === 'request'
                  ? '请求'
                  : k === 'response'
                  ? '响应'
                  : k === 'tool'
                  ? '工具'
                  : k === 'error'
                  ? '错误'
                  : '增量'

              return (
                <button
                  key={k}
                  onClick={() => setSelectedKind(k)}
                  className={`px-2 py-0.5 rounded-md font-medium transition-colors cursor-pointer text-[11px] ${
                    isSelected
                      ? 'bg-blue-600 text-white shadow-xs'
                      : 'text-zinc-500 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                  }`}
                >
                  {label}
                </button>
              )
            })}
          </div>

          <div className="flex items-center space-x-1.5 px-2 py-1 rounded-lg bg-zinc-100 dark:bg-zinc-800/80 border border-zinc-200 dark:border-zinc-700/60 w-56">
            <Search size={12} className="text-zinc-400" />
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="搜索事件或内容..."
              className="w-full text-[11px] bg-transparent outline-none text-zinc-900 dark:text-zinc-100 placeholder-zinc-400"
            />
          </div>
        </div>

        {/* 报文日志列表 */}
        <div className="flex-1 overflow-y-auto p-2 space-y-1 text-xs font-mono">
          {filtered.length === 0 ? (
            <div className="py-16 text-center text-zinc-400 font-sans">暂无符合条件的通信日志</div>
          ) : (
            filtered.map((log) => {
              const isExpanded = expandedId === log.id
              const isReq = log.kind === 'request'
              const isResp = log.kind === 'response'
              const isErr = log.kind === 'error'
              const isTool = log.kind === 'tool'
              const isDelta = log.kind === 'delta'

              const jsonText = JSON.stringify(log.payload || log.error || {}, null, 2)

              return (
                <div
                  key={log.id}
                  className="rounded-xl border border-zinc-200 dark:border-[#29292e] bg-zinc-50/50 dark:bg-[#161619] overflow-hidden"
                >
                  <div
                    onClick={() => setExpandedId(isExpanded ? null : log.id)}
                    className="flex items-center justify-between p-2 cursor-pointer hover:bg-zinc-100/60 dark:hover:bg-zinc-800/40 transition-colors"
                  >
                    <div className="flex items-center space-x-2 min-w-0 flex-1">
                      <span className="text-zinc-400">
                        {isExpanded ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                      </span>

                      <span className="text-[10px] text-zinc-400 flex-shrink-0">
                        {clock(log.at)}
                      </span>

                      {/* 类别徽标 */}
                      <span
                        className={`px-1.5 py-0.2 rounded text-[10px] font-semibold flex items-center space-x-1 ${
                          isReq
                            ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400 border border-blue-500/20'
                            : isResp
                            ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20'
                            : isErr
                            ? 'bg-rose-500/10 text-rose-600 dark:text-rose-400 border border-rose-500/20'
                            : isTool
                            ? 'bg-purple-500/10 text-purple-600 dark:text-purple-400 border border-purple-500/20'
                            : 'bg-zinc-200 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400'
                        }`}
                      >
                        {isReq && <ArrowUpRight size={10} />}
                        {isResp && <ArrowDownLeft size={10} />}
                        {isErr && <AlertTriangle size={10} />}
                        {isTool && <Wrench size={10} />}
                        {isDelta && <Brain size={10} />}
                        <span>{log.kind.toUpperCase()}</span>
                      </span>

                      <span className="text-xs font-semibold text-zinc-800 dark:text-zinc-200 truncate">
                        {log.method || 'Unknown Event'}
                      </span>
                    </div>

                    <div className="flex items-center space-x-2 flex-shrink-0 ml-2">
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          handleCopy(log.id, jsonText)
                        }}
                        className="flex items-center space-x-1 px-1.5 py-0.5 rounded hover:bg-zinc-200 dark:hover:bg-zinc-800 text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 transition-colors"
                        title="复制 Payload"
                      >
                        {copiedId === log.id ? (
                          <Check size={11} className="text-emerald-500" />
                        ) : (
                          <Copy size={11} />
                        )}
                        <span className="text-[10px]">
                          {copiedId === log.id ? '已复制' : '复制'}
                        </span>
                      </button>
                    </div>
                  </div>

                  {isExpanded && (
                    <div className="p-2.5 border-t border-zinc-200/80 dark:border-[#27272a] bg-zinc-100/60 dark:bg-black/30 overflow-x-auto max-h-64 select-text">
                      <pre className="text-[11px] text-zinc-700 dark:text-zinc-300 leading-normal whitespace-pre-wrap">
                        {jsonText}
                      </pre>
                    </div>
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
