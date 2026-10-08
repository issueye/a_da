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

import React, { useState, useEffect, useRef } from 'react'
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
  Compass,
} from 'lucide-react'
import type { Item, ToolCallItem, QuestionData, AgentMode } from '../types'
import { agentClient } from '../client/ws-client'
import { MarkdownRenderer } from './MarkdownRenderer'
import { TodoFloatingPanel } from './TodoFloatingPanel'

interface TranscriptProps {
  threadId?: string
  items: Item[]
  running: boolean
  activeWorkspace?: string
  currentMode?: AgentMode
  onAnswerQuestion: (callId: string, choice?: string, text?: string) => void
  onDecideApproval: (toolItemId: string, approved: boolean) => void
  onRetry?: () => void
}

/** 工具名称映射（对齐产品工具集与内置插件） */
const TOOL_LABEL: Record<string, string> = {
  list_files: '列出文件',
  read_file: '读取文件',
  search_files: '搜索代码',
  find_symbol: '查找符号',
  write_file: '写入文件',
  edit_file: '修改文件',
  run_command: '执行命令',
  run_background: '后台命令',
  check_task: '查看后台任务',
  kill_task: '停止后台任务',
  todo: '任务规划',
  invoke_subagent: '委派子智能体',
  check_subagent: '查询子智能体',
  ask_user: '向用户提问',
  decide: '结构化决策',
  check_gate: '验收门禁',
  git_status: 'Git 状态',
  git_diff: 'Git 变更',
  git_log: 'Git 历史',
  code_outline: '代码大纲',
  run_tests: '运行测试',
  batch_write: '批量写入',
  batch_replace: '批量替换',
  project_inspect: '项目诊断',
  read_url_content: '网页抓取',
}

/** 工具图标渲染 */
function renderToolIcon(name: string, size = 13, className = '') {
  switch (name) {
    case 'list_files':
      return <Folder size={size} className={className} />
    case 'read_file':
    case 'write_file':
    case 'edit_file':
    case 'batch_write':
    case 'code_outline':
      return <FileText size={size} className={className} />
    case 'batch_replace':
      return <Edit3 size={size} className={className} />
    case 'search_files':
    case 'project_inspect':
      return <Search size={size} className={className} />
    case 'find_symbol':
      return <Code size={size} className={className} />
    case 'run_command':
    case 'run_background':
      return <Terminal size={size} className={className} />
    case 'run_tests':
      return <CheckCircle2 size={size} className={className} />
    case 'check_task':
      return <Clock size={size} className={className} />
    case 'kill_task':
      return <XCircle size={size} className={className} />
    case 'todo':
      return <ListTodo size={size} className={className} />
    case 'invoke_subagent':
    case 'check_subagent':
      return <Bot size={size} className={className} />
    case 'ask_user':
      return <Sparkles size={size} className={className} />
    case 'check_gate':
      return <ShieldAlert size={size} className={className} />
    case 'decide':
      return <Compass size={size} className={className} />
    case 'git_status':
      return <FolderGit2 size={size} className={className} />
    case 'git_diff':
      return <Code size={size} className={className} />
    case 'git_log':
      return <Clock size={size} className={className} />
    default:
      return <Terminal size={size} className={className} />
  }
}

/** 路径工具集合 */
const PATH_TOOLS = new Set(['read_file', 'write_file', 'edit_file', 'list_files', 'code_outline'])

/** 智能拆分目标路径：文件名是正文，目录是暗色小字 */
function toolTarget(name: string, args: any): { target: string; dir: string } {
  if (name === 'check_gate') {
    const crit = args?.criteria || ''
    return { target: crit ? `标准: ${crit}` : '验收门禁检查', dir: '' }
  }
  if (name === 'decide') {
    const qCount = args?.questions ? Object.keys(args.questions).length : 0
    return { target: qCount > 0 ? `${qCount} 个判断问题` : '结构化决策', dir: '' }
  }
  if (name === 'git_status') return { target: '检查仓库工作区改动', dir: '' }
  if (name === 'git_diff') return { target: args?.file || args?.path || '工作区未暂存差异', dir: '' }
  if (name === 'git_log') return { target: `近期 ${args?.limit || 10} 次提交`, dir: '' }
  if (name === 'run_tests') return { target: '执行测试套件', dir: '' }
  if (name === 'project_inspect') return { target: '探测技术栈与项目环境', dir: '' }
  if (name === 'batch_write') {
    const count = Array.isArray(args?.files) ? args.files.length : 0
    return { target: `写入 ${count} 个文件`, dir: '' }
  }
  if (name === 'batch_replace') {
    const count = Array.isArray(args?.files) ? args.files.length : 0
    return { target: `在 ${count} 个文件中替换`, dir: '' }
  }

  const rawPath =
    typeof args === 'string'
      ? args
      : args?.path || args?.file || args?.filePath || args?.command || args?.task || ''
  const summary = String(rawPath).replace(/\r?\n+/g, ' ').trim()
  if (!PATH_TOOLS.has(name)) return { target: summary, dir: '' }
  const norm = summary.replace(/\\/g, '/')
  const cut = norm.lastIndexOf('/')
  if (cut < 0) return { target: summary, dir: '' }
  return { target: norm.slice(cut + 1), dir: norm.slice(0, cut + 1) }
}

/** 格式化耗时 */
export function formatDuration(ms?: number): string {
  if (ms === undefined || ms <= 0) return ''
  if (ms < 1000) return `${(ms / 1000).toFixed(1)}s`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  const minutes = Math.floor(ms / 60_000)
  const seconds = Math.round((ms % 60_000) / 1000)
  return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`
}

/** 格式化 Token 简写 */
export function formatTokenShort(n: number): string {
  if (n < 1000) return String(n)
  if (n < 10_000) return `${(n / 1000).toFixed(1)}k`
  if (n < 1_000_000) return `${Math.round(n / 1000)}k`
  const m = n / 1_000_000
  return m >= 10 ? `${Math.round(m)}M` : `${m.toFixed(1).replace(/\.0$/, '')}M`
}

/** 每个会话的独立滚动高度记忆，防止切换标签页时丢失阅读位置 */
const threadScrollMap = new Map<string, number>()

export const Transcript: React.FC<TranscriptProps> = ({
  threadId,
  items,
  running,
  activeWorkspace,
  currentMode = 'code',
  onAnswerQuestion,
  onDecideApproval,
  onRetry,
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

  // 对话项目流：紧凑的时间线排版（人说的话留白，工具和思考行紧密贴合）
  return (
    <div className="relative flex-1 flex flex-col h-full overflow-hidden">
      {/* 任务规划步骤独立收缩悬浮框（右上角常驻） */}
      <TodoFloatingPanel items={mergedItems} />

      <div
        ref={containerRef}
        onScroll={handleScroll}
        className="flex-1 overflow-y-auto px-3 md:px-6 py-2.5 space-y-1.5 select-text relative [scrollbar-gutter:stable]"
      >
        {mergedItems.map((item, index) => (
          <TranscriptItemRow
            key={item.id || index}
            item={item}
            running={running}
            threadId={threadId}
            onAnswerQuestion={onAnswerQuestion}
            onDecideApproval={onDecideApproval}
            onRetry={onRetry}
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
  onAnswerQuestion: (callId: string, choice?: string, text?: string) => void
  onDecideApproval: (toolItemId: string, approved: boolean) => void
  onRetry?: () => void
}> = ({ item, running, threadId, onAnswerQuestion, onDecideApproval, onRetry }) => {
  // 1. 思考过程块
  if (item.kind === 'thinking') {
    return <ThinkingRow item={item} />
  }

  // 2. 工具调用行/卡片
  if (item.kind === 'toolCall' || item.kind === 'tool' || item.role === 'tool' || item.tool || item.callId) {
    return (
      <ToolCard
        item={item}
        onDecideApproval={onDecideApproval}
        onAnswerQuestion={onAnswerQuestion}
      />
    )
  }

  // 3. 独立提问卡片
  if (item.kind === 'question' && item.question) {
    return (
      <div className="w-full pl-2 pr-2 my-2">
        <QuestionCardItem question={item.question} onAnswer={onAnswerQuestion} />
      </div>
    )
  }

  // 4. 用户输入（对齐 UserRow：右对齐气泡，带复制与原地修改重新发送）
  if (item.kind === 'user' || item.role === 'user') {
    return <UserRow item={item} />
  }

  // 5. 模型助手回复（对齐 AssistantRow：Markdown 渲染 + 请求异常警示与重试 + 底部遥测状态条）
  return <AssistantRow item={item} running={running} threadId={threadId} onRetry={onRetry} />
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

function formatTimeMs(ts?: number): string {
  if (!ts) return ''
  try {
    const d = new Date(ts)
    if (isNaN(d.getTime())) return String(ts)
    const pad = (n: number, z = 2) => String(n).padStart(z, '0')
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`
  } catch {
    return String(ts)
  }
}

/**
 * 工具调用卡片（ToolCard）
 * 严格对齐 src/ui/Transcript.tsx ToolCard 哲学：
 * 采用浅色系次要字色与弱边框，和正文回复形成明晰的视觉层级区分
 */
const ToolCard: React.FC<{
  item: Item
  onDecideApproval: (toolItemId: string, approved: boolean) => void
  onAnswerQuestion: (callId: string, choice?: string, text?: string) => void
}> = ({ item, onDecideApproval, onAnswerQuestion }) => {
  const [open, setOpen] = useState(false)
  const [replyText, setReplyText] = useState('')
  const toolName = item.tool || item.name || 'tool'
  const toolStatus = item.state || item.status || 'done'
  const isAwaiting = toolStatus === 'waiting_approval' || toolStatus === 'awaiting'
  const isRunning = toolStatus === 'running'
  const isError = toolStatus === 'failed' || toolStatus === 'error'

  // 解析工具返回的 JSON 结构化数据
  const structuredData = (() => {
    const raw = item.result || item.output
    if (!raw) return null
    if (typeof raw === 'object') return raw as Record<string, any>
    if (typeof raw === 'string' && raw.trim().startsWith('{')) {
      try {
        const parsed = JSON.parse(raw)
        if (parsed && typeof parsed === 'object') return parsed as Record<string, any>
      } catch {}
    }
    return null
  })()

  const durationMs = item.durationMs ?? structuredData?.duration_ms ?? structuredData?.durationMs
  const startedAt = item.startedAt ?? structuredData?.started_at ?? structuredData?.startedAt
  const finishedAt = item.finishedAt ?? structuredData?.finished_at ?? structuredData?.finishedAt
  const resultStatus = structuredData?.status || (item.state || item.status)

  const rawQuestion = item.question || (item.details as any)?.question
  const isQuestionTool = toolName === 'ask_user' || Boolean(rawQuestion)

  // 提取归一化问题对象
  const questionData = (() => {
    if (!isQuestionTool) return null
    const qText = rawQuestion?.question || (typeof item.args === 'object' ? item.args?.question : '') || '请回复智能体的提问'
    let options = Array.isArray(rawQuestion?.options) ? rawQuestion.options : []
    if (options.length === 0 && Array.isArray(rawQuestion?.choices)) {
      options = rawQuestion.choices.map((c: any) => ({
        value: c.id || c.value,
        label: c.label || c.title || String(c),
      }))
    }
    return {
      callId: rawQuestion?.callId || item.callId || item.id,
      question: qText,
      options,
    }
  })()

  const parsedTarget = toolTarget(toolName, item.args)
  const target = parsedTarget.target
  const dir = parsedTarget.dir

  // 状态与色彩映射：采用更加轻淡柔和的微色调
  const statusBadge = (() => {
    if (isAwaiting) {
      if (isQuestionTool) {
        return { label: '待答复', color: 'text-blue-500/90 bg-blue-500/10 border-blue-500/20' }
      }
      return { label: '等待批准', color: 'text-amber-500/90 bg-amber-500/10 border-amber-500/20' }
    }
    if (isRunning) return { label: '执行中', color: 'text-blue-500/90 bg-blue-500/10 border-blue-500/20' }
    if (toolName === 'check_gate') {
      const outputText = typeof item.output === 'string' ? item.output : String((item.result as any)?.output || '')
      if (outputText.includes('通过 [PASS]') || structuredData?.passed === true) {
        return { label: '门禁通过', color: 'text-emerald-600 dark:text-emerald-400 bg-emerald-500/10 border-emerald-500/20' }
      }
      if (outputText.includes('未通过 [BLOCKED]') || structuredData?.passed === false) {
        return { label: '门禁未通过', color: 'text-rose-600 dark:text-rose-400 bg-rose-500/10 border-rose-500/20' }
      }
    }
    if (isError) return { label: '失败', color: 'text-rose-500/90 bg-rose-500/10 border-rose-500/20' }
    return null
  })()

  const hasDetail = Boolean(item.result || item.output || item.error || item.patch || item.args || (isQuestionTool && isAwaiting))

  return (
    <div className="flex flex-col w-full my-0.5">
      {/* 紧凑工具单行：浅色系字色，不喧宾夺主 */}
      <div
        onClick={hasDetail ? () => setOpen(!open) : undefined}
        className={`flex items-center space-x-1.5 h-6 px-2 rounded-md text-[11px] select-none transition-colors ${
          hasDetail ? 'cursor-pointer hover:bg-zinc-100/60 dark:hover:bg-zinc-800/30' : 'cursor-default'
        }`}
      >
        <span className="text-zinc-400/80 dark:text-zinc-600 flex-shrink-0">
          {hasDetail ? open ? <ChevronDown size={10} /> : <ChevronRight size={10} /> : <span className="w-2.5" />}
        </span>

        <span className={isRunning ? 'text-blue-400' : 'text-zinc-400 dark:text-zinc-500'}>
          {renderToolIcon(toolName, 12)}
        </span>

        {/* 工具类型标签：浅灰色次要字 */}
        <span
          className={`flex-shrink-0 ${
            isRunning ? 'text-blue-500 dark:text-blue-400 font-medium' : 'text-zinc-500 dark:text-zinc-400 font-normal'
          }`}
        >
          {TOOL_LABEL[toolName] || toolName}
        </span>

        {/* 目标对象（文件名）与目录：柔和浅灰色，非深黑 */}
        <div className="flex items-center space-x-1 truncate font-mono text-[11px] min-w-0 flex-1">
          <span className="text-zinc-600 dark:text-zinc-400 font-normal truncate">
            {target}
          </span>
          {dir && (
            <span className="text-zinc-400/70 dark:text-zinc-600 text-[10.5px] truncate">
              {dir}
            </span>
          )}
        </div>

        {/* 耗时与状态指示：浅灰色 */}
        <div className="flex items-center space-x-1.5 flex-shrink-0">
          {durationMs ? (
            <span className="text-[9.5px] text-zinc-400/80 dark:text-zinc-500 font-mono">
              {durationMs}ms
            </span>
          ) : null}

          {statusBadge && (
            <span
              className={`px-1.5 py-0.2 rounded text-[9.5px] font-medium border ${statusBadge.color}`}
            >
              {statusBadge.label}
            </span>
          )}

          {item.reverted && (
            <span className="text-[9.5px] text-emerald-600/80 dark:text-emerald-400/80 bg-emerald-500/10 px-1.5 py-0.2 rounded">
              已撤销
            </span>
          )}
        </div>
      </div>

      {/* 展开盒体：浅微底 + 浅灰参数与输出 */}
      {(open || isAwaiting) && (
        <div className="ml-2 pl-2.5 border-l border-zinc-200/80 dark:border-zinc-800/80 my-0.5">
          <div className="bg-zinc-50/60 dark:bg-[#18191c]/60 border border-zinc-200/50 dark:border-zinc-800/40 rounded-lg overflow-hidden shadow-none text-xs">
            {/* 1. 提问作答交互条（ask_user 处于 awaiting 时展示） */}
            {isAwaiting && isQuestionTool && questionData && (
              <div className="p-3 bg-blue-50/70 dark:bg-blue-950/25 border-b border-blue-200/60 dark:border-blue-900/40 space-y-2 text-xs">
                <div className="flex items-center justify-between">
                  <div className="flex items-center space-x-1.5 font-medium text-blue-900 dark:text-blue-200">
                    <Sparkles size={13} className="text-blue-600 dark:text-blue-400" />
                    <span>智能体向你提问</span>
                  </div>
                  <span className="text-[10px] text-blue-600/80 dark:text-blue-400/80 bg-blue-100/70 dark:bg-blue-900/40 px-1.5 py-0.2 rounded">
                    等待答复后继续
                  </span>
                </div>

                <div className="text-xs text-zinc-800 dark:text-zinc-200 font-medium leading-relaxed">
                  {questionData.question}
                </div>

                {questionData.options && questionData.options.length > 0 && (
                  <div className="flex flex-wrap gap-1.5 pt-0.5">
                    {questionData.options.map((opt: any) => (
                      <button
                        key={opt.value}
                        onClick={() => onAnswerQuestion(questionData.callId, opt.value)}
                        className="px-2.5 py-1 rounded-lg bg-white dark:bg-blue-900/40 hover:bg-blue-50 dark:hover:bg-blue-800/50 border border-blue-200 dark:border-blue-700/50 text-blue-700 dark:text-blue-200 text-xs font-medium transition-colors shadow-xs"
                      >
                        {opt.label}
                      </button>
                    ))}
                  </div>
                )}

                <div className="flex items-center space-x-1.5 pt-1">
                  <input
                    type="text"
                    value={replyText}
                    onChange={(e) => setReplyText(e.target.value)}
                    placeholder="输入自定义答复或说明..."
                    className="flex-1 bg-white dark:bg-black/30 border border-blue-200 dark:border-blue-900/50 rounded-lg px-2.5 py-1 text-zinc-900 dark:text-zinc-100 text-xs outline-none focus:border-blue-500 shadow-xs"
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && replyText.trim()) {
                        onAnswerQuestion(questionData.callId, undefined, replyText.trim())
                        setReplyText('')
                      }
                    }}
                  />
                  <button
                    onClick={() => {
                      if (replyText.trim()) {
                        onAnswerQuestion(questionData.callId, undefined, replyText.trim())
                        setReplyText('')
                      }
                    }}
                    className="px-3 py-1 bg-blue-600 hover:bg-blue-500 text-white rounded-lg text-xs font-medium transition-colors shadow-xs"
                  >
                    提交
                  </button>
                  <button
                    onClick={() => onAnswerQuestion(questionData.callId, undefined, '用户确认继续')}
                    className="px-2.5 py-1 rounded-lg bg-zinc-200/70 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 hover:bg-zinc-300 dark:hover:bg-zinc-700 text-xs transition-colors"
                  >
                    跳过
                  </button>
                </div>
              </div>
            )}

            {/* 2. 真正的写工作区审批授权操作条（非提问工具且处于 awaiting 时展示） */}
            {isAwaiting && !isQuestionTool && (
              <div className="flex items-center justify-between p-2.5 bg-amber-500/10 border-b border-amber-500/20 text-xs">
                <div className="flex items-center space-x-2 text-amber-700 dark:text-amber-300 font-medium">
                  <ShieldAlert size={13} />
                  <span>这次调用会修改工作区，是否执行？</span>
                </div>
                <div className="flex items-center space-x-1.5">
                  <button
                    onClick={() => onDecideApproval(item.callId || item.id, false)}
                    className="px-2.5 py-0.5 rounded bg-zinc-200/80 dark:bg-zinc-800 hover:bg-zinc-300 dark:hover:bg-zinc-700 text-zinc-700 dark:text-zinc-300 text-xs transition-colors"
                  >
                    拒绝
                  </button>
                  <button
                    onClick={() => onDecideApproval(item.callId || item.id, true)}
                    className="px-2.5 py-0.5 rounded bg-blue-600 hover:bg-blue-500 text-white text-xs font-medium transition-colors shadow-xs"
                  >
                    批准
                  </button>
                </div>
              </div>
            )}

            {/* 2. 命令执行详情 */}
            {toolName === 'run_command' && item.args && (
              <div className="flex items-center justify-between px-2.5 py-1 bg-zinc-100/60 dark:bg-zinc-800/30 border-b border-zinc-200/50 dark:border-zinc-800/40 font-mono text-[10.5px] text-zinc-500 dark:text-zinc-400">
                <span className="truncate">
                  $ {typeof item.args === 'string' ? item.args : item.args.command || target}
                </span>
                <CopyButton
                  text={typeof item.args === 'string' ? item.args : item.args.command || target}
                  label="复制命令"
                />
              </div>
            )}

            {/* 3. 参数与输出展示：采用更浅更清爽的字色 */}
            <div className="p-2 space-y-1.5 select-text font-mono text-[11px]">
              {/* 结构化运行信息：执行状态、开始时间、结束时间、耗时 */}
              {(durationMs != null || startedAt != null || finishedAt != null || structuredData) && (
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1 py-1 px-2.5 rounded bg-zinc-100/60 dark:bg-black/30 text-[10px] text-zinc-500 dark:text-zinc-400 border border-zinc-200/40 dark:border-zinc-800/40 font-mono">
                  {resultStatus && (
                    <span className="flex items-center space-x-1">
                      <span className="text-zinc-400 dark:text-zinc-500 font-sans">状态:</span>
                      <span
                        className={
                          resultStatus === 'success' || resultStatus === 'done'
                            ? 'text-emerald-600 dark:text-emerald-400 font-medium'
                            : resultStatus === 'running'
                            ? 'text-blue-500 font-medium'
                            : 'text-rose-500 font-medium'
                        }
                      >
                        {resultStatus === 'success' || resultStatus === 'done' ? '✓ 成功' : resultStatus === 'running' ? '● 执行中' : '✕ 失败'}
                      </span>
                    </span>
                  )}
                  {startedAt != null && (
                    <span className="flex items-center space-x-1">
                      <span className="text-zinc-400 dark:text-zinc-500 font-sans">开始:</span>
                      <span>{formatTimeMs(startedAt)}</span>
                    </span>
                  )}
                  {finishedAt != null && (
                    <span className="flex items-center space-x-1">
                      <span className="text-zinc-400 dark:text-zinc-500 font-sans">结束:</span>
                      <span>{formatTimeMs(finishedAt)}</span>
                    </span>
                  )}
                  {durationMs != null && (
                    <span className="flex items-center space-x-1">
                      <span className="text-zinc-400 dark:text-zinc-500 font-sans">耗时:</span>
                      <span className="text-blue-600 dark:text-blue-400 font-semibold">{durationMs}ms</span>
                    </span>
                  )}
                </div>
              )}

              {item.args && toolName !== 'run_command' && (
                <div>
                  <span className="text-zinc-400 dark:text-zinc-500 font-sans text-[10px]">输入参数：</span>
                  <pre className="text-zinc-500 dark:text-zinc-400 whitespace-pre-wrap mt-0.5 bg-zinc-100/50 dark:bg-black/20 p-2 rounded border border-zinc-200/40 dark:border-zinc-800/40">
                    {typeof item.args === 'string' ? item.args : JSON.stringify(item.args, null, 2)}
                  </pre>
                </div>
              )}

              {(item.result || item.output) && (
                <div>
                  <div className="flex items-center justify-between">
                    <span className="text-zinc-400 dark:text-zinc-500 font-sans text-[10px]">
                      {isQuestionTool ? '用户答复：' : '执行输出：'}
                    </span>
                    <CopyButton
                      text={
                        typeof (item.result || item.output) === 'string'
                          ? String(item.result || item.output)
                          : JSON.stringify(item.result || item.output, null, 2)
                      }
                      label={isQuestionTool ? '复制答复' : '复制输出'}
                    />
                  </div>
                  <pre className="text-zinc-600 dark:text-zinc-300 whitespace-pre-wrap mt-0.5 max-h-56 overflow-y-auto leading-normal bg-zinc-100/50 dark:bg-black/20 p-2 rounded border border-zinc-200/40 dark:border-zinc-800/40">
                    {typeof (item.result || item.output) === 'string'
                      ? String(item.result || item.output)
                      : JSON.stringify(item.result || item.output, null, 2)}
                  </pre>
                </div>
              )}

              {item.error && (
                <div className="text-rose-500/90 dark:text-rose-400/90">
                  <span className="font-sans text-[10px]">执行异常：</span>
                  <pre className="whitespace-pre-wrap mt-0.5 bg-rose-500/5 p-2 rounded border border-rose-500/20">{item.error}</pre>
                </div>
              )}
            </div>

            {/* 4. 改动回滚条 */}
            {(toolName === 'write_file' || toolName === 'edit_file') &&
              item.checkpointId &&
              toolStatus === 'done' && (
                <div className="flex items-center justify-between px-2.5 py-1.5 bg-zinc-100/50 dark:bg-zinc-800/30 border-t border-zinc-200/50 dark:border-zinc-800/40 text-[10.5px]">
                  <span className="text-zinc-400 dark:text-zinc-500">
                    {item.reverted
                      ? '此次改动已撤销，文件已恢复原状。'
                      : '此次改动已写入工作区，可随时撤销。'}
                  </span>
                  {!item.reverted && (
                    <button
                      onClick={() =>
                        agentClient.revertCard(agentClient.snapshot.activeThreadId, item.id)
                      }
                      className="flex items-center space-x-1 px-2 py-0.2 rounded text-rose-500/90 dark:text-rose-400/90 hover:bg-rose-500/10 transition-colors"
                    >
                      <RotateCcw size={10} />
                      <span>撤销此次改动</span>
                    </button>
                  )}
                </div>
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

  // 识别并分离请求异常内容（如 "**请求异常**：发起模型请求失败: ..." 或 "请求异常: ..." 或 item.error）
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

/** 复制按钮 */
function CopyButton({ text, label = '复制' }: { text: string; label?: string }) {
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

/** 提问卡片组件（QuestionCard） */
const QuestionCardItem: React.FC<{
  question: QuestionData
  onAnswer: (callId: string, choice?: string, text?: string) => void
}> = ({ question, onAnswer }) => {
  const [customText, setCustomText] = useState('')

  return (
    <div className="p-2.5 border border-blue-500/30 bg-blue-50/40 dark:bg-blue-950/20 rounded-xl space-y-2 text-xs w-full shadow-xs">
      <div className="font-semibold text-blue-900 dark:text-blue-200">
        {question.question}
      </div>

      {question.options && question.options.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {question.options.map((opt) => (
            <button
              key={opt.value}
              onClick={() => onAnswer(question.callId, opt.value)}
              className="px-2.5 py-1 rounded-lg bg-white dark:bg-blue-600/30 hover:bg-blue-50 dark:hover:bg-blue-600/50 border border-blue-200 dark:border-blue-500/40 text-blue-700 dark:text-blue-100 text-xs font-medium transition-colors shadow-xs"
            >
              {opt.label}
            </button>
          ))}
        </div>
      )}

      <div className="flex items-center space-x-1.5 pt-0.5">
        <input
          type="text"
          value={customText}
          onChange={(e) => setCustomText(e.target.value)}
          placeholder="输入自定义答复..."
          className="flex-1 bg-white dark:bg-black/40 border border-blue-200 dark:border-blue-500/30 rounded-lg px-2.5 py-1 text-zinc-900 dark:text-zinc-100 text-xs outline-none focus:border-blue-500 shadow-xs"
          onKeyDown={(e) => {
            if (e.key === 'Enter' && customText.trim()) {
              onAnswer(question.callId, undefined, customText.trim())
              setCustomText('')
            }
          }}
        />
        <button
          onClick={() => {
            if (customText.trim()) {
              onAnswer(question.callId, undefined, customText.trim())
              setCustomText('')
            }
          }}
          className="px-3 py-1 bg-blue-600 hover:bg-blue-500 text-white rounded-lg text-xs font-medium transition-colors shadow-sm"
        >
          提交
        </button>
      </div>
    </div>
  )
}

