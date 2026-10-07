import React, { useState } from 'react'
import {
  FileText,
  RotateCcw,
  X,
  ChevronDown,
  ChevronRight,
  CheckCircle2,
  AlertTriangle,
  History,
  Check,
} from 'lucide-react'
import type { Thread, FileChange } from '../types'
import { deriveThreadFileChanges, deriveActiveChangeCount } from '../utils/derive-changes'
import { agentClient } from '../client/ws-client'
import { notify } from './ToastHost'

interface ChangesPanelProps {
  thread?: Thread
  isOpen: boolean
  onClose: () => void
}

function baseName(path: string): string {
  const norm = path.replace(/\\/g, '/')
  const cut = norm.lastIndexOf('/')
  return cut >= 0 ? norm.slice(cut + 1) : norm
}

function dirName(path: string): string {
  const norm = path.replace(/\\/g, '/')
  const cut = norm.lastIndexOf('/')
  return cut >= 0 ? norm.slice(0, cut) : ''
}

export const ChangesPanel: React.FC<ChangesPanelProps> = ({ thread, isOpen, onClose }) => {
  const [expandedPath, setExpandedPath] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  if (!isOpen || !thread) return null

  const changes: FileChange[] = deriveThreadFileChanges(thread.items || [])
  const activeCount = deriveActiveChangeCount(thread.items || [])

  const handleRevertFile = async (filePath: string) => {
    if (busy) return
    setBusy(true)
    const name = baseName(filePath)
    try {
      const res = await agentClient.revertFile(thread.id, filePath)
      if (res?.ok) {
        notify({ message: `已恢复 ${name}`, detail: filePath, level: 'success' })
      } else {
        notify({ message: `恢复 ${name} 失败`, detail: filePath, level: 'error' })
      }
    } catch (err: any) {
      notify({ message: `恢复 ${name} 失败: ${err.message}`, level: 'error' })
    } finally {
      setBusy(false)
    }
  }

  const handleRevertAll = async () => {
    if (busy || activeCount === 0) return
    if (!window.confirm(`确定要撤销当前会话所有的 ${activeCount} 个文件修改吗？`)) return
    setBusy(true)
    try {
      const res = await agentClient.revertAllChanges(thread.id)
      if (res?.ok) {
        notify({ message: `已恢复全部 ${activeCount} 个文件改动`, level: 'success' })
      } else {
        notify({ message: `恢复全部改动失败`, level: 'error' })
      }
    } catch (err: any) {
      notify({ message: `恢复全部改动失败: ${err.message}`, level: 'error' })
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="fixed top-12 right-4 w-96 max-h-[85vh] bg-white dark:bg-[#1c1c20] border border-zinc-200 dark:border-[#2f2f35] rounded-2xl shadow-2xl z-40 flex flex-col overflow-hidden select-none animate-in fade-in slide-in-from-top-2 duration-150">
      {/* 头部标题与全部恢复栏 */}
      <div className="flex items-center justify-between px-3.5 py-2.5 border-b border-zinc-200 dark:border-[#2b2b30] bg-zinc-50 dark:bg-[#18181c]">
        <div className="flex items-center space-x-2">
          <History size={15} className="text-blue-500" />
          <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">改动审查</span>
          <span className="px-1.5 py-0.2 rounded-full text-[10px] font-mono font-medium bg-blue-500/10 text-blue-600 dark:text-blue-400 border border-blue-500/20">
            {activeCount} 个文件
          </span>
        </div>

        <div className="flex items-center space-x-1.5">
          {activeCount > 0 && (
            <button
              onClick={handleRevertAll}
              disabled={busy}
              className="flex items-center space-x-1 px-2 py-1 rounded-lg bg-rose-50 dark:bg-rose-950/30 hover:bg-rose-100 dark:hover:bg-rose-900/50 text-rose-600 dark:text-rose-400 border border-rose-200 dark:border-rose-900/50 text-[11px] font-medium transition-colors cursor-pointer disabled:opacity-50"
              title="撤销本会话所有文件改动"
            >
              <RotateCcw size={11} />
              <span>全部恢复</span>
            </button>
          )}

          <button
            onClick={onClose}
            className="p-1 rounded-lg hover:bg-zinc-200 dark:hover:bg-zinc-800 text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 transition-colors cursor-pointer"
          >
            <X size={14} />
          </button>
        </div>
      </div>

      {/* 改动文件列表 */}
      <div className="flex-1 overflow-y-auto p-2 space-y-1.5 text-xs">
        {changes.length === 0 ? (
          <div className="py-8 text-center text-zinc-400 dark:text-zinc-500 text-xs">
            本会话尚未对任何文件产生改动
          </div>
        ) : (
          changes.map((change) => {
            const fileName = baseName(change.path)
            const dir = dirName(change.path)
            const isExpanded = expandedPath === change.path

            return (
              <div
                key={change.path}
                className={`rounded-xl border transition-all overflow-hidden ${
                  change.reverted
                    ? 'border-zinc-200 dark:border-zinc-800/60 bg-zinc-50/50 dark:bg-zinc-900/30 opacity-70'
                    : 'border-zinc-200 dark:border-[#2f2f35] bg-white dark:bg-[#1f1f23] hover:border-zinc-300 dark:hover:border-zinc-700 shadow-xs'
                }`}
              >
                {/* 文件摘要行 */}
                <div
                  onClick={() => setExpandedPath(isExpanded ? null : change.path)}
                  className="flex items-center justify-between p-2.5 cursor-pointer hover:bg-zinc-50/80 dark:hover:bg-zinc-800/40 transition-colors"
                >
                  <div className="flex items-center space-x-2 min-w-0 flex-1">
                    <span className="text-zinc-400">
                      {isExpanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                    </span>
                    <FileText size={14} className="text-blue-500 flex-shrink-0" />
                    <div className="flex flex-col min-w-0">
                      <span className="font-mono text-xs font-semibold text-zinc-800 dark:text-zinc-200 truncate">
                        {fileName}
                      </span>
                      {dir && (
                        <span className="text-[10px] text-zinc-400 dark:text-zinc-500 truncate font-mono">
                          {dir}
                        </span>
                      )}
                    </div>
                  </div>

                  <div className="flex items-center space-x-2 flex-shrink-0 ml-2">
                    {/* 统计指标 */}
                    <div className="flex items-center space-x-1 text-[10.5px] font-mono">
                      {change.additions > 0 && (
                        <span className="text-emerald-600 dark:text-emerald-400">+{change.additions}</span>
                      )}
                      {change.deletions > 0 && (
                        <span className="text-rose-600 dark:text-rose-400">-{change.deletions}</span>
                      )}
                      <span className="text-zinc-400 text-[9.5px]">({change.editsCount} 次修改)</span>
                    </div>

                    {/* 单文件恢复或已撤销指示 */}
                    {change.reverted ? (
                      <span className="flex items-center space-x-0.5 text-[10px] text-emerald-600 dark:text-emerald-400 bg-emerald-500/10 px-1.5 py-0.5 rounded font-medium">
                        <Check size={10} />
                        <span>已恢复</span>
                      </span>
                    ) : (
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          handleRevertFile(change.path)
                        }}
                        disabled={busy}
                        className="flex items-center space-x-1 px-2 py-0.5 rounded bg-zinc-100 dark:bg-zinc-800 hover:bg-rose-50 dark:hover:bg-rose-950/40 text-zinc-600 dark:text-zinc-400 hover:text-rose-600 dark:hover:text-rose-300 text-[10.5px] font-medium transition-colors border border-zinc-200 dark:border-zinc-700/60"
                        title="恢复此文件到修改前"
                      >
                        <RotateCcw size={10} />
                        <span>恢复原状</span>
                      </button>
                    )}
                  </div>
                </div>

                {/* 展开查看 Diff */}
                {isExpanded && change.latestPatch && (
                  <div className="border-t border-zinc-100 dark:border-[#2b2b30] bg-zinc-50 dark:bg-[#151518] p-2 text-[11px] font-mono overflow-x-auto max-h-56">
                    <pre className="text-zinc-700 dark:text-zinc-300 whitespace-pre-wrap leading-relaxed select-text">
                      {change.latestPatch.split('\n').map((line, idx) => {
                        const isAdd = line.startsWith('+') && !line.startsWith('+++')
                        const isDel = line.startsWith('-') && !line.startsWith('---')
                        const isHunk = line.startsWith('@@')

                        return (
                          <div
                            key={idx}
                            className={`${
                              isAdd
                                ? 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300 px-1 rounded-xs'
                                : isDel
                                ? 'bg-rose-500/15 text-rose-700 dark:text-rose-300 px-1 rounded-xs'
                                : isHunk
                                ? 'text-blue-500 dark:text-blue-400 py-0.5'
                                : 'text-zinc-600 dark:text-zinc-400'
                            }`}
                          >
                            {line || '\u00A0'}
                          </div>
                        )
                      })}
                    </pre>
                  </div>
                )}
              </div>
            )
          })
        )}
      </div>
    </div>
  )
}
