import React, { useState, useEffect } from 'react'
import { CheckCircle2, AlertCircle, AlertTriangle, Info, X } from 'lucide-react'
import type { ToastItem, ToastLevel } from '../types'

let toastListener: ((toasts: ToastItem[]) => void) | null = null
let currentToasts: ToastItem[] = []

/** 全局弹出一条轻提示通知 */
export function notify(options: {
  message: string
  detail?: string
  level?: ToastLevel
  durationMs?: number
  action?: {
    label: string
    onClick: () => void
  }
}) {
  const id = `toast_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`
  const level = options.level || 'success'
  const durationMs = options.durationMs !== undefined ? options.durationMs : level === 'error' ? 6000 : 3500

  const item: ToastItem = {
    id,
    level,
    message: options.message,
    detail: options.detail,
    durationMs,
    action: options.action,
  }

  currentToasts = [item, ...currentToasts].slice(0, 5)
  toastListener?.([...currentToasts])

  if (durationMs > 0) {
    setTimeout(() => {
      dismissToast(id)
    }, durationMs)
  }
}

/** 关闭单条通知 */
export function dismissToast(id: string) {
  currentToasts = currentToasts.filter((t) => t.id !== id)
  toastListener?.([...currentToasts])
}

export const ToastHost: React.FC = () => {
  const [toasts, setToasts] = useState<ToastItem[]>([])

  useEffect(() => {
    toastListener = setToasts
    setToasts([...currentToasts])
    return () => {
      toastListener = null
    }
  }, [])

  if (toasts.length === 0) return null

  return (
    <div className="fixed bottom-4 right-4 z-50 flex flex-col space-y-2 pointer-events-none max-w-sm w-full select-none">
      {toasts.map((toast) => {
        const isError = toast.level === 'error'
        const isWarn = toast.level === 'warn'
        const isInfo = toast.level === 'info'

        return (
          <div
            key={toast.id}
            className={`pointer-events-auto flex items-start space-x-2.5 p-3 rounded-xl border shadow-xl backdrop-blur-md transition-all duration-200 animate-in fade-in slide-in-from-bottom-2 ${
              isError
                ? 'bg-rose-50/95 dark:bg-[#201013]/95 border-rose-300 dark:border-rose-900/60 text-rose-900 dark:text-rose-200'
                : isWarn
                ? 'bg-amber-50/95 dark:bg-[#221708]/95 border-amber-300 dark:border-amber-900/60 text-amber-900 dark:text-amber-200'
                : isInfo
                ? 'bg-blue-50/95 dark:bg-[#0c1626]/95 border-blue-300 dark:border-blue-900/60 text-blue-900 dark:text-blue-200'
                : 'bg-white/95 dark:bg-[#1a1b1e]/95 border-zinc-200 dark:border-zinc-800 text-zinc-900 dark:text-zinc-100'
            }`}
          >
            <div className="flex-shrink-0 mt-0.5">
              {isError && <AlertCircle size={16} className="text-rose-500" />}
              {isWarn && <AlertTriangle size={16} className="text-amber-500" />}
              {isInfo && <Info size={16} className="text-blue-500" />}
              {!isError && !isWarn && !isInfo && (
                <CheckCircle2 size={16} className="text-emerald-500" />
              )}
            </div>

            <div className="flex-1 min-w-0">
              <div className="text-xs font-semibold leading-tight">{toast.message}</div>
              {toast.detail && (
                <div className="text-[11px] text-zinc-500 dark:text-zinc-400 mt-1 leading-normal break-words font-mono">
                  {toast.detail}
                </div>
              )}
              {toast.action && (
                <button
                  onClick={() => {
                    toast.action?.onClick()
                    dismissToast(toast.id)
                  }}
                  className="mt-1.5 px-2 py-0.5 rounded bg-blue-600 hover:bg-blue-500 text-white text-[11px] font-medium transition-colors"
                >
                  {toast.action.label}
                </button>
              )}
            </div>

            <button
              onClick={() => dismissToast(toast.id)}
              className="text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200 transition-colors p-0.5 rounded cursor-pointer"
            >
              <X size={13} />
            </button>
          </div>
        )
      })}
    </div>
  )
}
