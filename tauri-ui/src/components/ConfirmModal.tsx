import React, { useEffect, useRef, useState } from 'react'
import { AlertTriangle, AlertCircle, Info, X } from 'lucide-react'

export interface ConfirmModalProps {
  isOpen: boolean
  title?: string
  message: string
  subMessage?: string
  confirmText?: string
  cancelText?: string
  variant?: 'danger' | 'warning' | 'primary'
  promptMode?: boolean
  promptDefaultValue?: string
  promptPlaceholder?: string
  onConfirm: (value?: string) => void | Promise<void>
  onCancel: () => void
}

export const ConfirmModal: React.FC<ConfirmModalProps> = ({
  isOpen,
  title = '确认操作',
  message,
  subMessage,
  confirmText = '确定',
  cancelText = '取消',
  variant = 'danger',
  promptMode = false,
  promptDefaultValue = '',
  promptPlaceholder = '',
  onConfirm,
  onCancel,
}) => {
  const [inputValue, setInputValue] = useState(promptDefaultValue)
  const [loading, setLoading] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (isOpen) {
      setInputValue(promptDefaultValue)
      setLoading(false)
      if (promptMode) {
        setTimeout(() => inputRef.current?.focus(), 60)
      }
    }
  }, [isOpen, promptMode, promptDefaultValue])

  const handleAction = async () => {
    if (loading) return
    setLoading(true)
    try {
      await onConfirm(promptMode ? inputValue : undefined)
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    if (!isOpen) return
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onCancel()
      } else if (e.key === 'Enter') {
        e.preventDefault()
        handleAction()
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [isOpen, onCancel, inputValue, promptMode, loading])

  if (!isOpen) return null

  const iconMap = {
    danger: <AlertTriangle size={18} className="text-red-500 dark:text-red-400" />,
    warning: <AlertCircle size={18} className="text-amber-500 dark:text-amber-400" />,
    primary: <Info size={18} className="text-blue-500 dark:text-blue-400" />,
  }

  const iconBgMap = {
    danger: 'bg-red-50 dark:bg-red-950/40 border-red-200/80 dark:border-red-900/40',
    warning: 'bg-amber-50 dark:bg-amber-950/40 border-amber-200/80 dark:border-amber-900/40',
    primary: 'bg-blue-50 dark:bg-blue-950/40 border-blue-200/80 dark:border-blue-900/40',
  }

  const confirmBtnBgMap = {
    danger: 'bg-red-600 hover:bg-red-700 active:bg-red-800 text-white shadow-sm',
    warning: 'bg-amber-600 hover:bg-amber-700 active:bg-amber-800 text-white shadow-sm',
    primary: 'bg-blue-600 hover:bg-blue-700 active:bg-blue-800 text-white shadow-sm',
  }

  return (
    <div
      className="fixed inset-0 bg-black/40 dark:bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4 select-none animate-in fade-in duration-150"
      onClick={onCancel}
    >
      <div
        className="w-full max-w-[420px] bg-white dark:bg-[#18181b] border border-zinc-200 dark:border-[#2e2e33] rounded-2xl shadow-2xl overflow-hidden flex flex-col text-xs transition-colors"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 头部 */}
        <div className="flex items-center justify-between px-5 pt-4 pb-2">
          <div className="flex items-center space-x-2.5">
            <div className={`p-1.5 rounded-xl border ${iconBgMap[variant]}`}>
              {iconMap[variant]}
            </div>
            <h3 className="font-semibold text-zinc-900 dark:text-zinc-100 text-sm">{title}</h3>
          </div>
          <button
            onClick={onCancel}
            disabled={loading}
            className="p-1 text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200 hover:bg-zinc-100 dark:hover:bg-zinc-800 rounded-lg transition-colors"
          >
            <X size={15} />
          </button>
        </div>

        {/* 内容主体 */}
        <div className="px-5 py-3 space-y-2.5">
          <p className="text-xs text-zinc-700 dark:text-zinc-300 font-medium leading-relaxed whitespace-pre-wrap">
            {message}
          </p>

          {subMessage && (
            <div className="bg-zinc-50 dark:bg-[#202024] p-3 rounded-xl border border-zinc-100 dark:border-[#27272a] text-[11px] text-zinc-500 dark:text-zinc-400 leading-normal">
              {subMessage}
            </div>
          )}

          {promptMode && (
            <div className="pt-1">
              <input
                ref={inputRef}
                type="text"
                value={inputValue}
                onChange={(e) => setInputValue(e.target.value)}
                placeholder={promptPlaceholder}
                className="w-full px-3 py-2 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-xl text-xs text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 focus:outline-none focus:ring-1 focus:ring-blue-500 transition-colors"
              />
            </div>
          )}
        </div>

        {/* 底部按钮组 */}
        <div className="flex items-center justify-end space-x-2 px-5 py-3.5 bg-zinc-50/70 dark:bg-[#151518] border-t border-zinc-100 dark:border-[#232326]">
          <button
            type="button"
            onClick={onCancel}
            disabled={loading}
            className="px-3.5 py-1.5 rounded-xl border border-zinc-200 dark:border-[#2e2e33] text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 text-xs font-medium transition-colors"
          >
            {cancelText}
          </button>
          <button
            type="button"
            onClick={handleAction}
            disabled={loading || (promptMode && !inputValue.trim())}
            className={`px-4 py-1.5 rounded-xl text-xs font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed ${confirmBtnBgMap[variant]}`}
          >
            {loading ? '处理中...' : confirmText}
          </button>
        </div>
      </div>
    </div>
  )
}
