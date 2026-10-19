import React, { useState } from 'react'
import { Check, Copy } from 'lucide-react'
/** 复制按钮 */
export function CopyButton({ text, label = '复制' }: { text: string; label?: string }) {
  const [copied, setCopied] = useState(false)
  const handleCopy = (e: React.MouseEvent) => {
    e.stopPropagation()
    navigator.clipboard.writeText(text)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }
  return (
    <button
      onClick={handleCopy}
      className="flex items-center space-x-1 px-1.5 py-0.2 rounded text-[10px] text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors cursor-pointer"
      title={label}
    >
      {copied ? <Check size={10} className="text-emerald-500" /> : <Copy size={10} />}
      <span>{copied ? '已复制' : label}</span>
    </button>
  )
}
