/**
 * 对话流视图（Transcript）
 *
 * 深度参考并对齐 E:\codes\rust_projects\a_da\src\ui\Transcript.tsx 的设计与交互：
 * - 紧凑行级工具调用展示（一行排下三角、图标、工具名、目标、改动量、状态，展开才出盒子）
 * - 路径工具智能拆分（文件名正文高亮，所在目录暗色小字）
 * - 结构化思考折叠卡片（带垂直导引线、流式预览与多行折叠）
 * - 用户消息气泡（右对齐、图片胶囊、悬浮复制与原地修改重新发送）
 * - 执行过程聚合折叠条（ProcessGroup：步骤计数、推理耗时、改动量汇总）
 * - 助手回复底栏遥测（总耗时、Token 统计、复制全文）
 * - 审批授权栏（醒目警示色、批准与拒绝、撤销此次改动回滚条）
 * - 居中空会话欢迎视图（EmptyConversationView）
 */

import React, { useState, useEffect, useLayoutEffect, useRef, useMemo } from 'react'
import {
  Terminal,
  FileText,
  Folder,
  FolderGit2,
  Search,
  Code,
  ListTodo,
  Bot,
  Sparkles,
  Brain,
  Clock,
  Copy,
  Check,
  RotateCcw,
  RotateCw,
  Edit3,
  PackagePlus,
  AlertTriangle,
  X,
  ChevronDown,
  ChevronRight,
  Image as ImageIcon,
  CheckCircle2,
  XCircle,
  ShieldAlert,
  ArrowUp,
  ArrowDown,
  Circle,
  ExternalLink,
} from 'lucide-react'
import type { Item, ToolCallItem, QuestionData, AgentMode } from '../types'
import { agentClient } from '../client/ws-client'
import { normalizeQuestionData } from '../utils/question-data'
import { MarkdownRenderer } from './MarkdownRenderer'
import { AskUserQuestionRecord } from './AskUserPrompt'
import { TodoFloatingPanel } from './TodoFloatingPanel'
import { TOOL_LABEL } from '../utils/tool-labels'
import { approvalPreviewOf } from '../utils/pending-approvals'
import { notify } from './ToastHost'
import { TRANSCRIPT_MAX_WIDTH } from '../utils/layout'
import { formatDuration, formatTokenShort } from '../utils/format'
import { CopyButton } from './CopyButton'
import { ToolCard } from './TranscriptToolCard'

// 一次渲染多少个 item（长会话按窗口渲染，更早的折在"显示更早"后面）。
// 依据：实测一条长会话 822 个 item → 单帧快照 1.85 MB、渲染进程堆 1.5 GB。
// 没人会同时读 800 张卡片，全铺成 DOM 只是白烧内存。
const TRANSCRIPT_WINDOW = 60

interface TranscriptProps {
  threadId?: string
  items: Item[]
  running: boolean
  activeWorkspace?: string
  currentMode?: AgentMode
  onRetry?: () => void
  onSelectThread?: (threadId: string) => void
}

/** 工具名称映射见 `../utils/tool-labels`（与审批浮窗共用同一份，避免两处漂移） */

/** 工具图标渲染 */

const threadScrollMap = new Map<string, number>()

export const Transcript: React.FC<TranscriptProps> = ({
  threadId,
  items,
  running,
  activeWorkspace,
  currentMode = 'code',
  onRetry,
  onSelectThread,
}) => {
  const containerRef = useRef<HTMLDivElement>(null)
  const bottomRef = useRef<HTMLDivElement>(null)
  const isAutoScrollEnabledRef = useRef<boolean>(true)
  const [showScrollBottomBtn, setShowScrollBottomBtn] = useState<boolean>(false)
  const prevItemsCountRef = useRef<number>(items.length)
  const isInitialMountRef = useRef<boolean>(true)

  // 监听容器滚动，计算距离底部的间距
  const handleScroll = () => {
    const el = containerRef.current
    if (!el) return
    const distanceToBottom = el.scrollHeight - el.scrollTop - el.clientHeight
    const atBottom = distanceToBottom <= 80
    isAutoScrollEnabledRef.current = atBottom
    setShowScrollBottomBtn(!atBottom)

    if (threadId) {
      threadScrollMap.set(threadId, el.scrollTop)
    }
  }

  // 切换会话或初次挂载时，恢复该会话的历史滚动高度；若无历史记录则滚到底部
  useEffect(() => {
    const el = containerRef.current
    if (!el) return

    if (threadId && threadScrollMap.has(threadId)) {
      el.scrollTop = threadScrollMap.get(threadId)!
      const distanceToBottom = el.scrollHeight - el.scrollTop - el.clientHeight
      const atBottom = distanceToBottom <= 80
      isAutoScrollEnabledRef.current = atBottom
      setShowScrollBottomBtn(!atBottom)
    } else {
      el.scrollTop = el.scrollHeight
      isAutoScrollEnabledRef.current = true
      setShowScrollBottomBtn(false)
    }
    isInitialMountRef.current = false
  }, [threadId])

  // 当 items 内容增加或模型流式输出时
  useEffect(() => {
    if (isInitialMountRef.current) return
    const el = containerRef.current
    if (!el) return

    const itemsGrew = items.length > prevItemsCountRef.current
    prevItemsCountRef.current = items.length

    // 若有新消息加入（如用户发送新消息），且用户在底部附近，吸附到底部
    if (itemsGrew && isAutoScrollEnabledRef.current) {
      el.scrollTop = el.scrollHeight
      return
    }

    // 只有在当前会话真正处于运行中 (running)，且用户当前保持在底部 (isAutoScrollEnabled) 时，才执行流式吐字自动跟随到底部。
    // 绝不拉扯未在运行的会话或用户已经向上翻阅历史的会话！
    if (running && isAutoScrollEnabledRef.current) {
      el.scrollTop = el.scrollHeight
    }
  }, [items, running])

  // 手动点击悬浮“跳至底部”按钮
  const scrollToBottom = () => {
    const el = containerRef.current
    if (!el) return
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' })
    isAutoScrollEnabledRef.current = true
    setShowScrollBottomBtn(false)
    if (threadId) {
      threadScrollMap.set(threadId, el.scrollHeight)
    }
  }

  // 空对话状态由外层 EmptyConversationView / App 统一接管
  if (items.length === 0) {
    return null
  }

  // 对相邻连续的 assistant 碎片进行防御性聚合（兼容修复历史会话或极端流式碎块）
  const mergedItems = React.useMemo(() => {
    const list: Item[] = []
    for (const cur of items) {
      const prev = list[list.length - 1]
      const isCurAsst = cur.kind === 'assistant' || (!cur.kind && cur.role === 'assistant')
      const isPrevAsst = prev && (prev.kind === 'assistant' || (!prev.kind && prev.role === 'assistant'))

      if (isCurAsst && isPrevAsst) {
        list[list.length - 1] = {
          ...prev,
          text: (prev.text || '') + (cur.text || ''),
          streaming: cur.streaming ?? prev.streaming,
          turnDurationMs: cur.turnDurationMs ?? prev.turnDurationMs,
          durationMs: cur.durationMs ?? prev.durationMs,
          usage: cur.usage ?? prev.usage,
        }
      } else {
        list.push(cur)
      }
    }
    return list
  }, [items])

  // 对话项目流：按窗口渲染（见下方注释）；更早的折叠在"显示更早"后面
  const [visibleCount, setVisibleCount] = useState(TRANSCRIPT_WINDOW)
  const expandAnchorRef = useRef<{ height: number; top: number } | null>(null)

  // 切会话就回到默认窗口（别把上一条会话的展开状态带过去）
  useEffect(() => {
    setVisibleCount(TRANSCRIPT_WINDOW)
  }, [threadId])

  const { rows, hiddenCount } = useMemo(() => {
    if (mergedItems.length <= visibleCount) {
      return { rows: mergedItems, hiddenCount: 0 }
    }
    const tail = mergedItems.slice(-visibleCount)
    const tailIds = new Set(tail.map((it) => it.id))
    // 正在等你操作 / 正在跑的卡片：无论多早都必须留在渲染里
    const pinned = mergedItems.slice(0, mergedItems.length - visibleCount).filter((it) => {
      if (tailIds.has(it.id)) return false
      const s = it.status || it.state
      return s === 'running' || s === 'awaiting' || s === 'waiting_approval'
    })
    return { rows: [...pinned, ...tail], hiddenCount: mergedItems.length - tail.length - pinned.length }
  }, [mergedItems, visibleCount])

  // 展开更早内容时保持视线不跳：渲染前记下高度，渲染后按增量补偿 scrollTop
  const expandOlderItems = () => {
    const el = containerRef.current
    if (el) expandAnchorRef.current = { height: el.scrollHeight, top: el.scrollTop }
    setVisibleCount((c) => c + TRANSCRIPT_WINDOW)
  }
  const showAllItems = () => {
    const el = containerRef.current
    if (el) expandAnchorRef.current = { height: el.scrollHeight, top: el.scrollTop }
    setVisibleCount(mergedItems.length)
  }
  useLayoutEffect(() => {
    const anchor = expandAnchorRef.current
    const el = containerRef.current
    if (!anchor || !el) return
    expandAnchorRef.current = null
    el.scrollTop = anchor.top + (el.scrollHeight - anchor.height)
  }, [visibleCount])

  // 对话项目流：紧凑的时间线排版（人说的话留白，工具和思考行紧密贴合）
  //
  // 🔴 **按窗口渲染**：合帧是**全量快照**，一条长会话能到 800+ 个 item（实测 822 条、
  // 单帧 1.85 MB）。全部铺成 DOM 时，光这些卡片就能把渲染进程堆推到 GB 级
  // （实测 1.5 GB）——而且没人会同时读 800 张卡。这里只渲染最近 `TRANSCRIPT_WINDOW` 条，
  // 更早的折在"显示更早"后面；**正在等你操作/正在跑的卡片一律置顶保留**，
  // 免得审批卡或提问卡被窗口挡在外面。
  return (
    <div className="relative flex-1 flex flex-col h-full overflow-hidden">
      {/* 任务规划步骤独立收缩悬浮框（右上角常驻） */}
      <TodoFloatingPanel items={mergedItems} />

      <div
        ref={containerRef}
        onScroll={handleScroll}
        className="flex-1 overflow-y-auto px-3 md:px-6 py-2.5 select-text relative [scrollbar-gutter:stable]"
      >
        {/* 🔴 **阅读列**：所有条目统一收在一条居中的最大宽度里。
            没有它时助手正文会顶到容器右边缘（窗口一宽就成了 90+ 字一行的"横条"），
            右上角的待办浮层还会压在这段超宽文字上。收窄后两侧留白，
            浮卡落在留白里，正文也不再横跨整屏。 */}
        <div
          className="mx-auto w-full space-y-1.5"
          style={{ maxWidth: `${TRANSCRIPT_MAX_WIDTH}px` }}
        >
          {hiddenCount > 0 && (
            <div className="flex items-center justify-center gap-2 py-1.5">
              <button
                type="button"
                onClick={expandOlderItems}
                className="px-2.5 py-1 rounded-lg border border-zinc-300 dark:border-zinc-600 bg-white dark:bg-zinc-900 hover:bg-zinc-100 dark:hover:bg-zinc-800 text-zinc-600 dark:text-zinc-300 text-[11px] transition-colors"
              >
                显示更早的 {Math.min(TRANSCRIPT_WINDOW, hiddenCount)} 条
              </button>
              <span className="text-[11px] text-zinc-500 dark:text-zinc-500">
                还有 {hiddenCount} 条更早的内容未渲染（不影响会话本身）
              </span>
              <button
                type="button"
                onClick={showAllItems}
                className="px-2.5 py-1 rounded-lg border border-transparent hover:border-zinc-300 dark:hover:border-zinc-600 text-zinc-500 dark:text-zinc-400 text-[11px] transition-colors"
              >
                全部显示
              </button>
            </div>
          )}

          {rows.map((item, index) => (
            <TranscriptItemRow
              key={item.id || index}
              item={item}
              running={running}
              threadId={threadId}
              onRetry={onRetry}
              onSelectThread={onSelectThread}
            />
          ))}

          {running && (
            <div className="flex items-center space-x-2 text-zinc-600 dark:text-zinc-400 text-xs py-1.5 px-3 bg-zinc-100 dark:bg-zinc-800/50 border border-zinc-200 dark:border-zinc-700/60 rounded-full w-fit animate-pulse shadow-sm">
              <div className="w-2 h-2 rounded-full bg-blue-500 animate-ping" />
              <span>正在分析并执行操作...</span>
            </div>
          )}

          <div ref={bottomRef} className="h-1" />
        </div>
      </div>

      {/* 悬浮的快速跳至底部按钮 */}
      {showScrollBottomBtn && (
        <button
          type="button"
          onClick={scrollToBottom}
          className="absolute right-6 bottom-4 flex items-center space-x-1.5 px-3 py-1.5 bg-white/95 dark:bg-zinc-800/95 hover:bg-zinc-100 dark:hover:bg-zinc-700 text-zinc-700 dark:text-zinc-200 border border-zinc-200 dark:border-zinc-700 rounded-full shadow-lg text-xs font-medium backdrop-blur transition-all duration-150 hover:scale-105 z-20 cursor-pointer animate-in fade-in"
          title="回到底部"
        >
          <ArrowDown size={14} className="text-blue-500" />
          <span>跳至底部</span>
        </button>
      )}
    </div>
  )
}

/** 分发每项 Item 行渲染 */
const TranscriptItemRow: React.FC<{
  item: Item
  running: boolean
  threadId?: string
  onRetry?: () => void
  onSelectThread?: (threadId: string) => void
}> = ({ item, running, threadId, onRetry, onSelectThread }) => {
  // 1. 思考过程块
  if (item.kind === 'thinking') {
    return <ThinkingRow item={item} />
  }

  // 2. 工具调用行/卡片
  if (item.kind === 'toolCall' || item.kind === 'tool' || item.role === 'tool' || item.tool || item.callId) {
    return (
      <ToolCard
        item={item}
        onSelectThread={onSelectThread}
        currentThreadId={threadId}
      />
    )
  }

  // 3. 独立提问卡片
  if (item.kind === 'question' && item.question) {
    return (
      <div className="w-full pl-2 pr-2 my-2">
        <QuestionCardItem question={item.question} />
      </div>
    )
  }

  // 4. 用户输入（对齐 UserRow：右对齐气泡，带复制与原地修改重新发送）
  if (item.kind === 'user' || item.role === 'user') {
    return <UserRow item={item} />
  }

  // 5. 提示卡片（Notice）：面向人的独立提示，**不在助手正文里**
  //    ——模型请求失败这类信息走这里，所以它不会劈开一段连贯回答，
  //    也不会被当成"模型说过的话"回传给模型。
  if (item.kind === 'notice') {
    return <NoticeRow item={item} running={running} threadId={threadId} onRetry={onRetry} />
  }

  // 6. 模型助手回复（对齐 AssistantRow：Markdown 渲染 + 请求异常警示与重试 + 底部遥测状态条）
  return <AssistantRow item={item} running={running} threadId={threadId} onRetry={onRetry} />
}

/**
 * 提示卡片（NoticeRow）
 *
 * 承载"模型请求失败 / 对话已断开"这类**只给人看**的信息。它是一等卡片，
 * 与助手回复并列，而不是拼进正文——这是 `Item::Notice` 存在的理由
 * （落盘侧 `fold_messages` 会跳过它，故永不进入模型上下文）。
 *
 * `error` 级提示保留「请求重试」按钮：这个入口原先长在助手错误卡上，
 * 报错改走 Notice 之后必须跟着搬过来，否则用户就只剩"手打一遍"这一条路了。
 */
const NoticeRow: React.FC<{
  item: Item
  running: boolean
  threadId?: string
  onRetry?: () => void
}> = ({ item, running, threadId, onRetry }) => {
  const level = item.level || 'info'
  const isError = level === 'error'

  const handleRetry = (e: React.MouseEvent) => {
    e.stopPropagation()
    if (onRetry) {
      onRetry()
    } else {
      agentClient.retry(threadId)
    }
  }

  // 提示文本约定是「首行=结论，其余行=补充说明」（见宿主 dispatch 的拼装处）。
  const lines = (item.text || '').split('\n').filter((l) => l.trim())
  const headline = lines[0] || ''
  const detail = lines.slice(1).join('\n')

  const palette = isError
    ? 'bg-red-50/80 dark:bg-red-950/25 border-red-200/80 dark:border-red-900/50 text-red-700 dark:text-red-300'
    : level === 'warn'
      ? 'bg-amber-50/80 dark:bg-amber-950/25 border-amber-200/80 dark:border-amber-900/50 text-amber-700 dark:text-amber-300'
      : 'bg-blue-50/80 dark:bg-blue-950/25 border-blue-200/80 dark:border-blue-900/50 text-blue-700 dark:text-blue-300'

  return (
    <div className={`flex flex-col sm:flex-row sm:items-center justify-between gap-2.5 p-3 rounded-xl border text-xs my-1.5 shadow-2xs ${palette}`}>
      <div className="flex items-start space-x-2 min-w-0 flex-1">
        <AlertTriangle size={15} className="shrink-0 mt-0.5" />
        <div className="flex-1 min-w-0">
          <span className="font-semibold mr-1.5">请求异常:</span>
          <span className="font-mono text-[11.5px] break-all leading-relaxed">{headline}</span>
          {detail && (
            <div className="mt-1.5 font-sans text-[11px] opacity-90 whitespace-pre-wrap">{detail}</div>
          )}
        </div>
      </div>

      {isError && (
        <button
          type="button"
          onClick={handleRetry}
          disabled={running}
          className="flex items-center justify-center space-x-1.5 px-3 py-1.5 bg-red-600 hover:bg-red-500 disabled:opacity-50 text-white rounded-lg text-xs font-medium transition-all shadow-xs shrink-0 cursor-pointer active:scale-95 self-start sm:self-center"
          title="重新发起此模型请求"
        >
          <RotateCw size={12} className={running ? 'animate-spin' : ''} />
          <span>请求重试</span>
        </button>
      )}
    </div>
  )
}

/**
 * 用户消息气泡（UserRow）
 * 严格对齐 src/ui/Transcript.tsx UserRow：
 * - 右对齐聊天气泡，最大宽度 560px
 * - 图片素材胶囊条
 * - 悬停展示操作栏：复制（带状态）、编辑（内联编辑与重新发送）
 */
const UserRow: React.FC<{ item: Item }> = ({ item }) => {
  const [editing, setEditing] = useState(false)
  const [editText, setEditText] = useState(item.text || '')
  const [copied, setCopied] = useState(false)

  const handleCopy = (e: React.MouseEvent) => {
    e.stopPropagation()
    navigator.clipboard.writeText(item.text || '')
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  const handleStartEdit = () => {
    setEditText(item.text || '')
    setEditing(true)
  }

  const handleConfirmResend = () => {
    const trimmed = editText.trim()
    if (!trimmed && (!item.images || item.images.length === 0)) return
    agentClient.editAndResend(
      agentClient.snapshot.activeThreadId,
      item.id,
      trimmed,
      item.images
    )
    setEditing(false)
  }

  // 编辑模式视图（对齐原版 user-edit-box）
  if (editing) {
    return (
      <div className="flex flex-col w-full max-w-[600px] ml-auto bg-zinc-50 dark:bg-[#202123] border border-blue-500/80 rounded-xl p-3 gap-2.5 shadow-md">
        <div className="flex items-center justify-between text-xs">
          <div className="flex items-center space-x-1.5 font-medium text-zinc-900 dark:text-zinc-100">
            <Edit3 size={13} className="text-blue-500" />
            <span>编辑并重新发送</span>
            <span className="text-[11px] text-zinc-500 dark:text-zinc-400 font-normal">
              (将丢弃此消息之后的所有对话记录)
            </span>
          </div>
        </div>

        {item.images && item.images.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {item.images.map((img, idx) => (
              <div
                key={idx}
                className="flex items-center space-x-1 px-2 py-0.5 rounded bg-zinc-200 dark:bg-zinc-800 text-[11px] text-zinc-700 dark:text-zinc-300 font-mono"
              >
                <ImageIcon size={11} className="text-blue-500" />
                <span>{img.startsWith('data:') ? '图片素材' : img.split(/[/\\]/).pop()}</span>
              </div>
            ))}
          </div>
        )}

        <textarea
          value={editText}
          onChange={(e) => setEditText(e.target.value)}
          rows={3}
          className="w-full text-xs bg-white dark:bg-[#18181b] border border-zinc-200 dark:border-zinc-700 rounded-lg p-2.5 text-zinc-900 dark:text-zinc-100 outline-none focus:border-blue-500 leading-relaxed font-sans"
          onKeyDown={(e) => {
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
              e.preventDefault()
              handleConfirmResend()
            } else if (e.key === 'Escape') {
              setEditing(false)
            }
          }}
        />

        <div className="flex items-center justify-end space-x-2 pt-0.5">
          <button
            onClick={() => setEditing(false)}
            className="px-3 py-1 rounded text-xs text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200 dark:hover:bg-zinc-800 transition-colors"
          >
            取消 (Esc)
          </button>
          <button
            onClick={handleConfirmResend}
            className="flex items-center space-x-1 px-3 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white text-xs font-medium transition-colors shadow-sm"
          >
            <ArrowUp size={12} strokeWidth={2.5} />
            <span>重新发送 (Ctrl+Enter)</span>
          </button>
        </div>
      </div>
    )
  }

  const lines = (item.text || '').split('\n')

  return (
    <div className="flex flex-col items-end w-full group/user gap-0.5 my-0.5">
      {/* 气泡本体（对齐原版 C.user：暗色 #252629 / 浅色 #F7F7F5，带圆角 10px） */}
      <div className="flex flex-col max-w-[560px] py-1.5 px-3 bg-zinc-100 dark:bg-[#252629] border border-zinc-200/80 dark:border-[#2e3033] rounded-[10px] shadow-xs select-text text-xs leading-normal text-zinc-900 dark:text-zinc-100">
        {item.images && item.images.length > 0 && (
          <div className="flex flex-wrap gap-1.5 mb-2">
            {item.images.map((img, idx) => (
              <div
                key={idx}
                className="flex items-center space-x-1 px-2 py-0.5 rounded bg-white dark:bg-zinc-800/80 border border-zinc-200 dark:border-zinc-700 text-[11px] text-zinc-700 dark:text-zinc-300 font-mono shadow-xs"
              >
                <ImageIcon size={11} className="text-blue-500" />
                <span className="truncate max-w-[180px]">
                  {img.startsWith('data:') ? '图片素材' : img.split(/[/\\]/).pop()}
                </span>
              </div>
            ))}
          </div>
        )}

        <div className="whitespace-pre-wrap font-sans">
          {lines.map((line, idx) => (
            <div key={idx}>{line || '\u00A0'}</div>
          ))}
        </div>
      </div>

      {/* 底部微型操作栏：复制与编辑（对齐原版） */}
      <div className="flex items-center space-x-1.5 pr-1 opacity-0 group-hover/user:opacity-100 transition-opacity">
        <button
          onClick={handleCopy}
          className="flex items-center space-x-1 px-1.5 py-0.5 rounded text-[10px] text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-200/60 dark:hover:bg-zinc-800 transition-colors"
          title="复制文本"
        >
          {copied ? <Check size={10} className="text-emerald-500" /> : <Copy size={10} />}
          <span>{copied ? '已复制' : '复制'}</span>
        </button>

        <button
          onClick={handleStartEdit}
          className="flex items-center space-x-1 px-1.5 py-0.5 rounded text-[10px] text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-200/60 dark:hover:bg-zinc-800 transition-colors"
          title="编辑此条消息"
        >
          <Edit3 size={10} />
          <span>编辑</span>
        </button>

        {item.queued && (
          <span className="text-[10px] text-zinc-400 dark:text-zinc-500 font-mono">排队中</span>
        )}
      </div>
    </div>
  )
}

/**
 * 结构化思考卡片（ThinkingRow & ThinkingBody）
 * 严格对齐 src/ui/Transcript.tsx ThinkingRow：
 * - 单行紧凑设计：三角 + 脑图 + 「思考」+ 持续时长 + 流式预览
 * - 展开后左侧垂直导轨线，带「推理分析」标题栏与快捷复制
 */
const ThinkingRow: React.FC<{ item: Item }> = ({ item }) => {
  const [open, setOpen] = useState(false)
  const [full, setFull] = useState(false)
  // 与原版一致的判定：endedAt 未落 = 仍在推理；结束后的时长按 endedAt - at 计算
  const isStreaming = item.endedAt === undefined
  const text = item.text || item.thinking || ''

  const startedAt = item.at ?? item.createdAt ?? Date.now()
  const seconds =
    item.endedAt === undefined ? null : Math.max(1, Math.round((item.endedAt - startedAt) / 1000))

  // 流式预览末行摘要（对齐原版）
  const streamingPreview = (() => {
    if (!isStreaming) return null
    const lines = text.split('\n').map((l) => l.trim()).filter(Boolean)
    if (!lines.length) return null
    const last = lines[lines.length - 1]
    return last.length > 42 ? `${last.slice(0, 42)}…` : last
  })()

  if (!text.trim() && !isStreaming) return null

  const lines = text.split('\n')
  const limit = full ? lines.length : Math.min(lines.length, 16)

  return (
    <div className="flex flex-col w-full my-0.5">
      {/* 紧凑思考头部单行：采用浅淡低调字色 */}
      <div
        onClick={() => setOpen(!open)}
        className="flex items-center space-x-1.5 h-6 px-2 rounded-md cursor-pointer hover:bg-zinc-100/70 dark:hover:bg-zinc-800/30 transition-colors text-[11px] select-none text-zinc-400 dark:text-zinc-500"
      >
        <span className="text-zinc-400/80 dark:text-zinc-600">
          {open ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
        </span>

        <Brain
          size={11}
          className={isStreaming ? 'text-blue-400 animate-pulse' : 'text-zinc-400 dark:text-zinc-500'}
        />

        <span
          className={`font-normal ${isStreaming ? 'text-blue-500 dark:text-blue-400' : 'text-zinc-500 dark:text-zinc-400'}`}
        >
          思考
        </span>

        <span className="text-[10.5px] text-zinc-400/70 dark:text-zinc-600 font-mono">
          {isStreaming ? '· 思考中…' : seconds ? `· 持续 ${seconds} 秒` : ''}
        </span>

        {!open && streamingPreview && (
          <span className="text-[10.5px] text-zinc-400 dark:text-zinc-500/80 truncate max-w-sm italic">
            · "{streamingPreview}"
          </span>
        )}
      </div>

      {/* 展开后的推理原文：柔和淡导线 + 浅微底面板 + 浅灰色文字 */}
      {open && (
        <div className="ml-2 pl-2.5 border-l border-blue-400/30 dark:border-blue-500/20 mt-0.5 mb-1">
          <div className="bg-zinc-50/50 dark:bg-[#1a1b1e]/50 border border-zinc-200/50 dark:border-zinc-800/40 rounded-lg p-2.5 space-y-1.5 text-[11.5px] shadow-none">
            <div className="flex items-center justify-between pb-1 border-b border-zinc-200/50 dark:border-zinc-800/40 text-[10.5px]">
              <span className="font-normal text-zinc-400 dark:text-zinc-500">
                {isStreaming ? '正在推理…' : '推理分析'}
              </span>
              <CopyButton text={text} label="复制思考" />
            </div>

            <div className="font-sans leading-relaxed text-zinc-500 dark:text-zinc-400 whitespace-pre-wrap select-text space-y-0.5">
              {lines.slice(0, limit).map((line, idx) => (
                <div key={idx}>{line || '\u00A0'}</div>
              ))}
            </div>

            {lines.length > limit && (
              <button
                onClick={() => setFull(true)}
                className="text-[10.5px] text-blue-500/80 dark:text-blue-400/70 hover:underline pt-0.5"
              >
                展开其余 {lines.length - limit} 行
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  )
}



/**
 * 助手回复文本行（AssistantRow）
 * 严格对齐 src/ui/Transcript.tsx AssistantRow：
 * - 纯粹的 Markdown 排版
 * - 流式状态指示（脉动点 + 已用时）
 * - 每轮对话底栏遥测（总耗时、Token 统计、复制全文）
 */
const AssistantRow: React.FC<{
  item: Item
  running: boolean
  threadId?: string
  onRetry?: () => void
}> = ({ item, running, threadId, onRetry }) => {
  const text = item.text || ''
  const isStreaming = Boolean(item.streaming) && running
  const durationText = formatDuration(item.turnDurationMs || item.durationMs)

  // 从助手正文里分离出「请求异常」——**只为读老会话**。
  //
  // 新产生的失败信息已经改走 `Item::Notice`（见 NoticeRow），不再落进正文。
  // 但 2026-10 之前写下的会话文件里，那段 `**请求异常**…` 确实存在于助手文本中，
  // 删掉这段解析会让历史会话的回答里裸露出一串 JSON 报错。
  const { normalText, errorDetail } = React.useMemo(() => {
    if (item.error) {
      return { normalText: text, errorDetail: item.error }
    }
    // 匹配末尾出现的异常模式，允许前面包含已经流式输出的正常文字
    const markerRegex = /(?:\r?\n)*(\*{0,2}(?:请求异常|模型请求失败|发起模型请求失败|请求失败|执行异常)\*{0,2}[：:]\s*[\s\S]+)$/
    const match = text.match(markerRegex)
    if (match && match.index !== undefined) {
      const normal = text.slice(0, match.index).trim()
      const rawError = match[1]
      // 提取核心错误信息
      const detail = rawError
        .replace(/^\*{0,2}(?:请求异常|模型请求失败|发起模型请求失败|请求失败|执行异常)\*{0,2}[：:]\s*/, '')
        .trim()
      return { normalText: normal, errorDetail: detail || rawError }
    }
    // 兼容首部直接为请求异常的情况
    if (text.startsWith('请求异常') || text.startsWith('**请求异常**')) {
      const detail = text
        .replace(/^\*{0,2}(?:请求异常|模型请求失败|发起模型请求失败|请求失败|执行异常)\*{0,2}[：:]\s*/, '')
        .trim()
      return { normalText: '', errorDetail: detail || text }
    }
    return { normalText: text, errorDetail: null }
  }, [text, item.error])

  const handleRetry = (e: React.MouseEvent) => {
    e.stopPropagation()
    if (onRetry) {
      onRetry()
    } else {
      agentClient.retry(threadId)
    }
  }

  return (
    <div className="flex flex-col w-full my-1 space-y-1 select-text">
      {/* Markdown 正文排版：全功能渲染（若存在正常部分） */}
      {normalText ? (
        <MarkdownRenderer content={normalText} />
      ) : null}

      {/* 请求异常警示卡片与请求重试按钮（严格对齐设计与报错截图） */}
      {errorDetail && (
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-2.5 p-3 rounded-xl bg-red-50/80 dark:bg-red-950/25 border border-red-200/80 dark:border-red-900/50 text-xs my-1.5 shadow-2xs">
          <div className="flex items-start space-x-2 min-w-0 flex-1">
            <AlertTriangle size={15} className="text-red-500 shrink-0 mt-0.5" />
            <div className="flex-1 min-w-0 text-red-700 dark:text-red-300">
              <span className="font-semibold text-red-600 dark:text-red-400 mr-1.5">
                请求异常:
              </span>
              <span className="font-mono text-[11.5px] break-all leading-relaxed">
                {errorDetail}
              </span>
              {/* 自动重试已用尽：补一句"去哪儿改"，断开的结论本身由宿主写在正文里 */}
              {(errorDetail.includes('已重试') || errorDetail.includes('未重试')) && (
                <div className="mt-1.5 font-sans text-[11px] text-red-600/90 dark:text-red-400/90">
                  可到「设置 → 运行环境 → 请求重试次数」调大次数后，再点右侧「请求重试」。
                </div>
              )}
            </div>
          </div>

          <button
            type="button"
            onClick={handleRetry}
            disabled={running}
            className="flex items-center justify-center space-x-1.5 px-3 py-1.5 bg-red-600 hover:bg-red-500 disabled:opacity-50 text-white rounded-lg text-xs font-medium transition-all shadow-xs shrink-0 cursor-pointer active:scale-95 self-start sm:self-center"
            title="重新发起此模型请求"
          >
            <RotateCw size={12} className={running ? 'animate-spin' : ''} />
            <span>请求重试</span>
          </button>
        </div>
      )}

      {/* 正在流式生成指示条（对齐原版） */}
      {isStreaming && (
        <div className="flex items-center space-x-2 text-zinc-500 text-xs pt-0.5">
          <Circle size={7} className="fill-blue-500 text-blue-500 animate-ping" />
          <span className="text-blue-500 font-medium">正在生成…</span>
          {durationText && (
            <span className="text-[10px] text-zinc-400 font-mono">
              已用时 {durationText}
            </span>
          )}
        </div>
      )}

      {/* 跑完后的底栏指标条（对齐原版 turn-stats） */}
      {!isStreaming && (normalText.trim() || errorDetail) && (
        <div className="flex items-center justify-between pt-1 mt-1 border-t border-zinc-200/70 dark:border-zinc-800/80 text-[10.5px] text-zinc-500">
          <div className="flex items-center space-x-2.5">
            {durationText ? (
              <div className="flex items-center space-x-1 px-1.5 py-0.2 rounded bg-zinc-100 dark:bg-zinc-800/60 font-mono text-zinc-600 dark:text-zinc-400">
                <Clock size={10} className="text-zinc-400" />
                <span>总耗时 {durationText}</span>
              </div>
            ) : null}

            {item.usage && (item.usage.totalTokens > 0 || item.usage.promptTokens > 0) ? (
              <div className="flex items-center space-x-1 px-1.5 py-0.2 rounded bg-zinc-100 dark:bg-zinc-800/60 font-mono text-zinc-600 dark:text-zinc-400">
                <Sparkles size={10} className="text-zinc-400" />
                <span>
                  {formatTokenShort(item.usage.totalTokens || (item.usage.promptTokens + item.usage.completionTokens))}{' '}
                  Tokens
                </span>
              </div>
            ) : null}
          </div>

          <div className="flex items-center space-x-2">
            {errorDetail && (
              <button
                type="button"
                onClick={handleRetry}
                disabled={running}
                className="flex items-center space-x-1 px-1.5 py-0.2 rounded text-[10px] text-red-600 dark:text-red-400 hover:bg-red-50 dark:hover:bg-red-950/40 border border-red-200 dark:border-red-900/40 transition-colors cursor-pointer"
                title="重新发起此请求"
              >
                <RotateCw size={10} className={running ? 'animate-spin' : ''} />
                <span>重试</span>
              </button>
            )}
            <CopyButton text={text} label="复制全文" />
          </div>
        </div>
      )}
    </div>
  )
}


/** 提问卡片（对话流里的**只读记录**）。
 *
 * 作答在输入框上方的提问浮窗（`AskUserFloatingPanel`）里进行——这里只把
 * "问了什么、有哪些选项"留档，不再摆第二套按钮。
 * 走到这里的可能是线上原样数据（选择题键是 `choices`）——归一化后再渲染，
 * 否则多题的选项会因为键名对不上而整片消失。
 */
const QuestionCardItem: React.FC<{ question: QuestionData }> = ({ question }) => {
  const q = normalizeQuestionData(question, question.callId)
  return (
    <AskUserQuestionRecord
      question={q}
      className="p-2.5 border border-blue-500/30 bg-blue-50/40 dark:bg-blue-950/20 rounded-xl w-full shadow-xs"
      hint="在输入框上方的提问浮窗作答"
    />
  )
}

