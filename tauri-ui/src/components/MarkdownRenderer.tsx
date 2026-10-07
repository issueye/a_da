import React, { useMemo, useCallback } from 'react'
import { Marked } from 'marked'

/** 转义 HTML 特殊字符 */
function escapeHtml(str: string): string {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

/** 复制按钮 SVG 图标 */
const COPY_SVG = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>`
const CHECK_SVG = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="#10b981" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>`

/** 创建定制 marked 解析实例，紧凑排版，收紧上下边距 */
function createCustomMarked() {
  const instance = new Marked({
    gfm: true,
    breaks: true,
  })

  instance.use({
    renderer: {
      heading(this: any, token: any) {
        const text = this.parser.parseInline(token.tokens)
        const depth = token.depth
        const sizeClasses = [
          'text-base font-bold mt-2.5 mb-1',
          'text-sm font-bold mt-2 mb-1',
          'text-xs font-semibold mt-1.5 mb-0.5',
          'text-xs font-semibold mt-1 mb-0.5',
          'text-xs font-medium mt-1 mb-0.5',
          'text-xs font-medium mt-1 mb-0.5',
        ][depth - 1] || 'text-xs font-bold mt-1 mb-0.5'

        return `<h${depth} class="${sizeClasses} text-zinc-900 dark:text-zinc-100 tracking-tight leading-tight">${text}</h${depth}>`
      },

      paragraph(this: any, token: any) {
        const text = this.parser.parseInline(token.tokens)
        return `<p class="my-1 leading-relaxed text-zinc-800 dark:text-zinc-200 text-xs">${text}</p>`
      },

      list(this: any, token: any) {
        const listTag = token.ordered ? 'ol' : 'ul'
        const listClass = token.ordered ? 'list-decimal' : 'list-disc'
        let body = ''
        for (const item of token.items) {
          body += this.listitem(item)
        }
        return `<${listTag} class="${listClass} pl-4 my-1 space-y-0.5 text-xs text-zinc-800 dark:text-zinc-200">${body}</${listTag}>`
      },

      listitem(this: any, item: any) {
        let text = ''
        if (item.tokens) {
          text = this.parser.parse(item.tokens)
        } else {
          text = item.text || ''
        }
        return `<li class="leading-relaxed">${text}</li>`
      },

      blockquote(this: any, token: any) {
        const text = this.parser.parse(token.tokens)
        return `<blockquote class="border-l-2 border-blue-500/70 pl-2.5 my-1 text-zinc-600 dark:text-zinc-400 italic bg-blue-500/5 py-0.5 rounded-r">${text}</blockquote>`
      },

      codespan(this: any, token: any) {
        return `<code class="px-1.5 py-0.2 mx-0.5 rounded bg-zinc-100 dark:bg-zinc-800 text-[11px] font-mono text-pink-600 dark:text-pink-400 border border-zinc-200/70 dark:border-zinc-700/60">${escapeHtml(token.text)}</code>`
      },

      code(this: any, token: any) {
        const language = (token.lang || 'text').split(/\s+/)[0]
        const escaped = escapeHtml(token.text)
        const encoded = encodeURIComponent(token.text)

        return `
<div class="code-block-wrapper my-1.5 rounded-lg border border-zinc-200 dark:border-[#333338] bg-zinc-50 dark:bg-[#121214] overflow-hidden shadow-xs">
  <div class="flex items-center justify-between px-3 py-1 bg-zinc-100/90 dark:bg-[#18181c] border-b border-zinc-200/80 dark:border-[#28282c] text-[10.5px] text-zinc-500 dark:text-zinc-400 font-mono select-none">
    <span class="font-medium">${language}</span>
    <button type="button" class="copy-code-btn flex items-center space-x-1 px-1.5 py-0.5 rounded hover:bg-zinc-200 dark:hover:bg-zinc-700/60 hover:text-zinc-900 dark:hover:text-white transition-all cursor-pointer" data-code="${encoded}">
      <span class="btn-icon">${COPY_SVG}</span>
      <span class="btn-text">复制</span>
    </button>
  </div>
  <pre class="p-2.5 text-[11px] font-mono text-zinc-800 dark:text-zinc-200 overflow-x-auto leading-normal whitespace-pre"><code class="language-${language}">${escaped}</code></pre>
</div>`
      },

      table(this: any, token: any) {
        let headerHtml = ''
        for (const cell of token.header) {
          headerHtml += `<th class="px-2.5 py-1 font-semibold text-zinc-900 dark:text-zinc-100 border-b border-zinc-200 dark:border-zinc-700/80 bg-zinc-100/60 dark:bg-zinc-800/40">${this.parser.parseInline(cell.tokens)}</th>`
        }

        let rowsHtml = ''
        for (const row of token.rows) {
          let rowCells = ''
          for (const cell of row) {
            rowCells += `<td class="px-2.5 py-1 border-b border-zinc-100 dark:border-zinc-800/60 text-zinc-800 dark:text-zinc-200">${this.parser.parseInline(cell.tokens)}</td>`
          }
          rowsHtml += `<tr class="hover:bg-zinc-50 dark:hover:bg-zinc-800/20 transition-colors">${rowCells}</tr>`
        }

        return `
<div class="overflow-x-auto my-1.5 rounded-lg border border-zinc-200 dark:border-zinc-700/70 shadow-xs">
  <table class="min-w-full text-xs text-left border-collapse">
    <thead><tr>${headerHtml}</tr></thead>
    <tbody>${rowsHtml}</tbody>
  </table>
</div>`
      },

      hr(this: any) {
        return `<hr class="my-2 border-t border-zinc-200 dark:border-zinc-800/80" />`
      },

      link(this: any, token: any) {
        const text = this.parser.parseInline(token.tokens)
        const titleAttr = token.title ? ` title="${escapeHtml(token.title)}"` : ''
        return `<a href="${escapeHtml(token.href)}"${titleAttr} target="_blank" rel="noopener noreferrer" class="text-blue-600 dark:text-blue-400 hover:underline underline-offset-2 break-all">${text}</a>`
      },
    },
  })

  return instance
}

const markedParser = createCustomMarked()

export interface MarkdownRendererProps {
  content: string
  className?: string
}

export const MarkdownRenderer: React.FC<MarkdownRendererProps> = ({ content, className = '' }) => {
  // 解析 Markdown 生成紧凑 HTML
  const html = useMemo(() => {
    if (!content) return ''
    try {
      return markedParser.parse(content) as string
    } catch (e) {
      console.error('[MarkdownRenderer] 解析失败:', e)
      return `<p class="text-xs text-zinc-800 dark:text-zinc-200 whitespace-pre-wrap">${escapeHtml(content)}</p>`
    }
  }, [content])

  // 事件委托：高效处理所有代码块复制按钮的交互与反馈
  const handleClick = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement
    const btn = target.closest('.copy-code-btn') as HTMLButtonElement | null
    if (!btn) return

    e.preventDefault()
    e.stopPropagation()

    const rawCode = btn.getAttribute('data-code')
    if (!rawCode) return

    const decoded = decodeURIComponent(rawCode)
    navigator.clipboard.writeText(decoded).then(() => {
      const textSpan = btn.querySelector('.btn-text')
      const iconSpan = btn.querySelector('.btn-icon')

      if (textSpan) textSpan.textContent = '已复制'
      if (iconSpan) iconSpan.innerHTML = CHECK_SVG
      btn.classList.add('text-emerald-500')

      setTimeout(() => {
        if (textSpan) textSpan.textContent = '复制'
        if (iconSpan) iconSpan.innerHTML = COPY_SVG
        btn.classList.remove('text-emerald-500')
      }, 1500)
    })
  }, [])

  return (
    <div
      onClick={handleClick}
      dangerouslySetInnerHTML={{ __html: html }}
      className={`markdown-body select-text text-xs leading-relaxed text-zinc-900 dark:text-zinc-100 ${className}`}
    />
  )
}
