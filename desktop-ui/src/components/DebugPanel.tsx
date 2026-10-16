import React, { useState, useEffect, useMemo } from 'react'
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
  Activity,
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
  /** 来源过滤（P0 埋点）：诊断条目带 `payload.origin`，按它能把某个扩展点的调用挑出来。 */
  const [selectedOrigin, setSelectedOrigin] = useState<string | 'all'>('all')
  const [search, setSearch] = useState('')
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [copiedId, setCopiedId] = useState<string | null>(null)

  useEffect(() => {
    if (isOpen) {
      const unsub = agentClient.subscribeDebug(setLogs)
      return unsub
    }
  }, [isOpen])

  /**
   * 可选的来源清单，从当前条目里**派生**（不写死名单）。
   *
   * 为什么派生：来源会随分期增长（内置工具 → 插件 → 技能 → 钩子点位），
   * 写死一份就等于给未来埋一个"忘了改"的坑。
   */
  const origins = useMemo(() => {
    const set = new Set<string>()
    for (const l of logs) {
      const o = (l.payload as any)?.origin
      if (typeof o === 'string' && o) set.add(o)
    }
    return Array.from(set).sort()
  }, [logs])

  /**
   * 诊断的**按来源聚合**（P4 · SP-74）。
   *
   * 为什么需要：日志是流水，"这个插件到底出过什么问题"得人自己一条条翻。
   * 聚合后回答两件事——**多少条**、**最近一条是哪个点位**。点位名本身就说明了问题类型：
   * `hook.replace_rejected` 是被重校验拒了、`hook.finished{timed_out}` 是撞预算、
   * `plugin.skipped` 是压根没装上、`skill.invalid` 是技能文件写错了键名。
   *
   * 与来源过滤**共用同一个状态**：点一下聚合条就等于按它过滤，不引入第二套交互。
   */
  const diagByOrigin = useMemo(() => {
    const map = new Map<string, { count: number; lastPoint: string }>()
    for (const l of logs) {
      if (l.kind !== 'diag') continue
      const p = (l.payload || {}) as any
      const origin = typeof p.origin === 'string' && p.origin ? p.origin : '(无来源)'
      const point = typeof p.point === 'string' ? p.point : ''
      const cur = map.get(origin)
      if (cur) {
        cur.count += 1
        if (point) cur.lastPoint = point
      } else {
        map.set(origin, { count: 1, lastPoint: point })
      }
    }
    return Array.from(map.entries()).sort((a, b) => b[1].count - a[1].count)
  }, [logs])

  if (!isOpen) return null

  const handleCopy = (id: string, text: string) => {
    navigator.clipboard.writeText(text)
    setCopiedId(id)
    setTimeout(() => setCopiedId(null), 1500)
  }

  const filtered = logs.filter((log) => {
    if (selectedKind !== 'all' && log.kind !== selectedKind) return false
    if (selectedOrigin !== 'all' && (log.payload as any)?.origin !== selectedOrigin) return false
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
        <div className="flex flex-col px-3.5 py-2 border-b border-zinc-200 dark:border-[#27272a] bg-white dark:bg-[#1a1a1e] gap-1.5 text-xs">
          <div className="flex items-center justify-between gap-2">
            <div className="flex items-center space-x-1 flex-wrap gap-y-1">
              {(['all', 'request', 'response', 'tool', 'error', 'delta', 'diag'] as const).map((k) => {
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
                    : k === 'delta'
                    ? '增量'
                    : '诊断'

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

          {/* 来源过滤（只在真有诊断条目时出现）：把某个扩展点的调用一次挑出来 */}
          {origins.length > 0 && (
            <div className="flex items-center space-x-1 flex-wrap gap-y-1">
              <span className="text-[10px] text-zinc-400 mr-0.5">来源</span>
              {(['all', ...origins] as const).map((o) => {
                const isSelected = selectedOrigin === o
                return (
                  <button
                    key={o}
                    onClick={() => setSelectedOrigin(o)}
                    className={`px-1.5 py-0.5 rounded font-mono transition-colors cursor-pointer text-[10px] ${
                      isSelected
                        ? 'bg-emerald-600 text-white'
                        : 'text-zinc-500 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                    }`}
                  >
                    {o === 'all' ? '全部来源' : o}
                  </button>
                )
              })}
            </div>
          )}

          {/* 诊断聚合（SP-74）：按来源回答"谁出了多少问题、最近一条是什么"。
              点一下就按它过滤——复用上面那个来源状态，不引入第二套交互。 */}
          {diagByOrigin.length > 0 && (
            <div className="flex items-center space-x-1 flex-wrap gap-y-1">
              <span className="text-[10px] text-zinc-400 mr-0.5">诊断</span>
              {diagByOrigin.map(([origin, info]) => (
                <button
                  key={origin}
                  onClick={() => setSelectedOrigin(origin === '(无来源)' ? 'all' : origin)}
                  title={info.lastPoint ? `最近：${info.lastPoint}` : undefined}
                  className="px-1.5 py-0.5 rounded text-[10px] bg-zinc-100 dark:bg-zinc-800/60 text-zinc-600 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors cursor-pointer"
                >
                  {origin} <span className="font-semibold">{info.count}</span>
                  {info.lastPoint && (
                    <span className="ml-1 opacity-70 font-mono">{info.lastPoint}</span>
                  )}
                </button>
              ))}
            </div>
          )}
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
              const isDiag = log.kind === 'diag'
              const diagPayload = isDiag ? ((log.payload || {}) as any) : null

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
                            : isDiag
                            ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400 border border-amber-500/20'
                            : 'bg-zinc-200 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400'
                        }`}
                      >
                        {isReq && <ArrowUpRight size={10} />}
                        {isResp && <ArrowDownLeft size={10} />}
                        {isErr && <AlertTriangle size={10} />}
                        {isTool && <Wrench size={10} />}
                        {isDelta && <Brain size={10} />}
                        {isDiag && <Activity size={10} />}
                        <span>{log.kind.toUpperCase()}</span>
                      </span>

                      <span className="text-xs font-semibold text-zinc-800 dark:text-zinc-200 truncate">
                        {log.method || 'Unknown Event'}
                      </span>

                      {/* 诊断遥测：把"哪一族 / 什么结果 / 多久 / 多大 / 谁产生"一列摆出来，
                          不用展开 payload 才能看懂一条埋点 */}
                      {diagPayload && (
                        <span className="text-[10px] text-zinc-500 dark:text-zinc-400 font-mono truncate">
                          {diagPayload.family}
                          {diagPayload.outcome ? ` · ${diagPayload.outcome}` : ''}
                          {typeof diagPayload.durationMs === 'number' ? ` · ${diagPayload.durationMs}ms` : ''}
                          {typeof diagPayload.bytes === 'number' ? ` · ${diagPayload.bytes}B` : ''}
                          {diagPayload.origin ? ` · ← ${diagPayload.origin}` : ''}
                        </span>
                      )}
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
