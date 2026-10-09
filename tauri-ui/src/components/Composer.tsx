import React, { useState, useRef, useEffect, useMemo } from 'react'
import {
  Plus,
  ArrowUp,
  Square,
  Sparkles,
  Code2,
  ListTodo,
  Compass,
  FileCode,
  Image as ImageIcon,
  AtSign,
  Slash,
  ChevronDown,
  X,
  Shield,
  Brain,
  Gauge,
  Database,
  Clock,
  PieChart,
  Send,
  HelpCircle,
  Paperclip,
  Bot,
  ArrowLeft,
  History,
  Terminal as TerminalIcon,
  Check,
  Sliders,
} from 'lucide-react'
import type { AgentMode, ProviderConfig, ApprovalMode, Effort, Thread, Item, QuestionData, QueuedItem } from '../types'
import { agentClient } from '../client/ws-client'
import { formatDuration, formatTokenShort } from './Transcript'
import { QueuedMessagesFloatingPanel } from './QueuedMessagesFloatingPanel'
import { ContextUsagePopover } from './ContextUsagePopover'
import { computeContextBreakdown } from '../utils/context-breakdown'
import { MentionMenu, type MentionItem } from './MentionMenu'
import { SlashCommandMenu, type SlashCommandItem } from './SlashCommandMenu'
import { FilePicker } from './FilePicker'

export interface ComposerProps {
  thread?: Thread
  mode: AgentMode
  running: boolean
  providerConfig: ProviderConfig
  approvalMode?: ApprovalMode
  effort?: Effort
  queue?: QueuedItem[]
  onSend: (text: string, images?: string[]) => void
  onAbort: () => void
  onSetMode: (mode: AgentMode) => void
  onSetApprovalMode?: (mode: ApprovalMode) => void
  onSetEffort?: (effort: Effort) => void
  onOpenSettings: () => void
  onAnswerQuestion?: (callId: string, choice?: string, text?: string) => void
  onPromoteQueueItem?: (index: number) => void
  onRemoveQueueItem?: (index: number) => void
  onClearQueue?: () => void
  onSwitchThread?: (threadId: string) => void
  onCompact?: () => void
  onOpenChanges?: () => void
  onOpenDebug?: () => void
  onOpenPlugins?: () => void
  onNewThread?: () => void
  activeChangeCount?: number
  centered?: boolean
}

/** 模式定义选项（对齐原版 GPUIX Composer） */
export const MODE_OPTIONS: { value: AgentMode; label: string; icon: any; desc: string }[] = [
  { value: 'code', label: 'Code 编码', icon: Code2, desc: '全能敏捷编码与工程构建 (默认)' },
  { value: 'pm', label: 'PM 项目管理', icon: ListTodo, desc: '目标拆解与分派，经网关驱动 Coding Agent 落地' },
  { value: 'plan', label: 'Plan 规划', icon: Compass, desc: '只读架构分析与实施计划设计 (只读防写)' },
  { value: 'create', label: 'Create 创造', icon: Sparkles, desc: '智能体自我进化与工具/技能 CRUD' },
]

/** 审批策略选项 */
export const APPROVAL_OPTIONS: { value: ApprovalMode; label: string; desc: string }[] = [
  { value: 'auto', label: '自动批准', desc: '读写文件和执行命令都不再询问' },
  { value: 'ask', label: '每次询问', desc: '每次敏感工具调用都等你确认' },
  { value: 'readonly', label: '严格只读', desc: '只允许读取操作，修改需手动确认' },
]

/** 思考力度选项 */
export const EFFORT_OPTIONS: { value: Effort; label: string; desc: string }[] = [
  { value: 'max', label: 'Max 最大思考', desc: '深度规划推理，最慢也最稳' },
  { value: 'high', label: 'High 高度思考', desc: '充分推理并权衡多种备选方案' },
  { value: 'medium', label: 'Medium 中度思考', desc: '平衡速度与推理质量' },
  { value: 'low', label: 'Low 轻度思考', desc: '快速直接响应，节省 Token' },
]

/** 快捷斜杠指令配置 */
const SLASH_COMMANDS: { command: string; label: string; desc: string }[] = [
  { command: '/plan', label: '架构规划', desc: '仅分析与生成多步骤实施计划' },
  { command: '/goal', label: '长期攻坚', desc: '保持持续专注直到彻底完成目标' },
  { command: '/boost', label: '深度思考', desc: '多视角严谨审视与验证代码' },
  { command: '/browser', label: '网页浏览', desc: '检索网络资料或网页抓取分析' },
]

export const Composer: React.FC<ComposerProps> = ({
  thread,
  mode,
  running,
  providerConfig,
  approvalMode = 'auto',
  effort = 'max',
  queue = [],
  onSend,
  onAbort,
  onSetMode,
  onSetApprovalMode,
  onSetEffort,
  onOpenSettings,
  onAnswerQuestion,
  onPromoteQueueItem,
  onRemoveQueueItem,
  onClearQueue,
  onSwitchThread,
  onCompact,
  onOpenChanges,
  onOpenDebug,
  onOpenPlugins,
  onNewThread,
  activeChangeCount = 0,
  centered = false,
}) => {
  const [text, setText] = useState('')
  const [images, setImages] = useState<string[]>([])
  const [selectedCommand, setSelectedCommand] = useState<string | null>(null)
  const [isFocused, setIsFocused] = useState(false)

  // 会话草稿隔离：保存与恢复每个会话独立的输入框草稿，防止 A 会话输入串到 B 会话
  const draftsRef = useRef<Map<string, { text: string; images: string[] }>>(new Map())
  const prevThreadIdRef = useRef<string | undefined>(thread?.id)

  useEffect(() => {
    const curId = thread?.id
    const prevId = prevThreadIdRef.current
    if (prevId && prevId !== curId) {
      draftsRef.current.set(prevId, { text, images })
    }
    if (curId && curId !== prevId) {
      const saved = draftsRef.current.get(curId)
      setText(saved?.text || '')
      setImages(saved?.images || [])
    }
    prevThreadIdRef.current = curId
  }, [thread?.id])

  // 下拉菜单与浮窗控制
  const [plusMenuOpen, setPlusMenuOpen] = useState(false)
  const [modeMenuOpen, setModeMenuOpen] = useState(false)
  const [approvalMenuOpen, setApprovalMenuOpen] = useState(false)
  const [effortMenuOpen, setEffortMenuOpen] = useState(false)
  const [contextPopoverOpen, setContextPopoverOpen] = useState(false)
  const [mentionOpen, setMentionOpen] = useState(false)
  const [mentionQuery, setMentionQuery] = useState('')
  const [slashMenuOpen, setSlashMenuOpen] = useState(false)
  const [slashQuery, setSlashQuery] = useState('')

  const [modelMenuOpen, setModelMenuOpen] = useState(false)

  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const plusMenuRef = useRef<HTMLDivElement>(null)
  const modeMenuRef = useRef<HTMLDivElement>(null)
  const modelMenuRef = useRef<HTMLDivElement>(null)
  const approvalMenuRef = useRef<HTMLDivElement>(null)
  const effortMenuRef = useRef<HTMLDivElement>(null)

  // 自建文件/图片选择浮窗状态（避免原生弹窗导致 H5 无法兼容）
  const [filePickerState, setFilePickerState] = useState<{
    isOpen: boolean
    title: string
    filterExts?: string[]
    type: 'image' | 'file'
  }>({
    isOpen: false,
    title: '',
    type: 'file',
  })

  // 点击外部关闭所有下拉菜单
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      const target = e.target as Node
      if (plusMenuRef.current && !plusMenuRef.current.contains(target)) setPlusMenuOpen(false)
      if (modeMenuRef.current && !modeMenuRef.current.contains(target)) setModeMenuOpen(false)
      if (modelMenuRef.current && !modelMenuRef.current.contains(target)) setModelMenuOpen(false)
      if (approvalMenuRef.current && !approvalMenuRef.current.contains(target)) setApprovalMenuOpen(false)
      if (effortMenuRef.current && !effortMenuRef.current.contains(target)) setEffortMenuOpen(false)
    }
    document.addEventListener('mousedown', handleClickOutside)
    return () => document.removeEventListener('mousedown', handleClickOutside)
  }, [])

  // 自动根据内容调整输入框高度
  useEffect(() => {
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto'
      textareaRef.current.style.height = `${Math.min(textareaRef.current.scrollHeight, 180)}px`
    }
  }, [text])

  // 处理输入文本与 @ 提及、/ 指令检测
  const handleTextChange = (val: string) => {
    setText(val)

    // 检测 @ 提及
    const lastAtIndex = val.lastIndexOf('@')
    if (lastAtIndex >= 0 && (lastAtIndex === 0 || /\s/.test(val[lastAtIndex - 1]))) {
      const query = val.slice(lastAtIndex + 1)
      if (!/\s/.test(query)) {
        setMentionOpen(true)
        setMentionQuery(query)
        setSlashMenuOpen(false)
        return
      }
    }
    setMentionOpen(false)

    // 检测 / 快捷指令
    if (val.startsWith('/') && !val.includes('\n')) {
      setSlashMenuOpen(true)
      setSlashQuery(val.slice(1))
      return
    }
    setSlashMenuOpen(false)
  }

  const handleSelectMention = (item: MentionItem) => {
    const lastAtIndex = text.lastIndexOf('@')
    if (lastAtIndex >= 0) {
      const prefix = text.slice(0, lastAtIndex)
      setText(`${prefix}${item.insertText} `)
    } else {
      insertPrefix(item.insertText)
    }
    setMentionOpen(false)
    textareaRef.current?.focus()
  }

  const handleSelectSlash = (item: SlashCommandItem) => {
    setSlashMenuOpen(false)
    if (item.action) {
      item.action()
      setText('')
    } else {
      setSelectedCommand(item.command)
      setText('')
    }
    textareaRef.current?.focus()
  }

  // 处理剪贴板粘贴（支持图片粘贴）
  const handlePaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const clipboardData = e.clipboardData
    if (!clipboardData) return

    const items = clipboardData.items
    for (let i = 0; i < items.length; i++) {
      const item = items[i]
      if (item.type.indexOf('image') !== -1) {
        const file = item.getAsFile()
        if (file) {
          const reader = new FileReader()
          reader.onload = (event) => {
            if (event.target?.result) {
              setImages((prev) => [...prev, event.target!.result as string])
            }
          }
          reader.readAsDataURL(file)
          e.preventDefault()
        }
      }
    }
  }

  // 拖拽放入文件或图片
  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault()
    const files = e.dataTransfer.files
    if (files && files.length > 0) {
      for (let i = 0; i < files.length; i++) {
        const file = files[i]
        if (file.type.startsWith('image/')) {
          const reader = new FileReader()
          reader.onload = (event) => {
            if (event.target?.result) {
              setImages((prev) => [...prev, event.target!.result as string])
            }
          }
          reader.readAsDataURL(file)
        } else {
          insertPrefix(`[文件: ${file.name}]`)
        }
      }
    }
  }

  // 键盘快捷键监听
  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      handleSend()
    } else if (e.key === 'Backspace' && !text && selectedCommand) {
      setSelectedCommand(null)
    }
  }

  // 触发发送消息（支持运行中输入追加排队）
  const handleSend = () => {
    const trimmed = text.trim()
    let finalPayload = trimmed
    if (selectedCommand) {
      finalPayload = finalPayload ? `${selectedCommand} ${finalPayload}` : selectedCommand
      setSelectedCommand(null)
    }

    if (!finalPayload && images.length === 0) return

    onSend(finalPayload, images.length > 0 ? images : undefined)
    setText('')
    setImages([])
    if (thread?.id) {
      draftsRef.current.delete(thread.id)
    }
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto'
    }
  }


  // 插入字符前缀
  const insertPrefix = (char: string) => {
    setText((prev) => (prev ? `${prev} ${char}` : char))
    setPlusMenuOpen(false)
    textareaRef.current?.focus()
  }

  // 查找是否有正在等待回答的提问（浮动置顶展示）
  const pendingQuestion = useMemo<QuestionData | null>(() => {
    if (!thread || !thread.items || thread.items.length === 0) return null
    for (let i = thread.items.length - 1; i >= 0; i--) {
      const it = thread.items[i]
      const isAwaiting = it.status === 'awaiting' || it.state === 'awaiting' || it.tool === 'ask_user'
      const q = it.question || (it.details as any)?.question
      if (q && isAwaiting && it.status !== 'done' && it.status !== 'error') {
        let options = Array.isArray(q.options) ? q.options : []
        if (options.length === 0 && Array.isArray((q as any).choices)) {
          options = (q as any).choices.map((c: any) => ({
            value: c.id || c.value,
            label: c.label || c.title || String(c),
          }))
        }
        return {
          ...q,
          callId: q.callId || it.callId || it.id,
          options,
        }
      }
      // 如果遇到最新的用户消息则说明提问已翻篇
      if (it.kind === 'user') break
    }
    return null
  }, [thread])

  // 计算当前会话的遥测指标（严格对齐原版 GPUIX 指标算法）
  const telemetry = useMemo(() => {
    const items = thread?.items || []
    const userItems = items.filter((it) => it.kind === 'user' || it.role === 'user')
    const toolItems = items.filter((it) => it.kind === 'toolCall' || it.toolCalls || it.callId)
    const assistantItems = items.filter((it) => it.kind === 'assistant' || it.role === 'assistant')

    const turns = userItems.length
    const steps = toolItems.length

    let latestAssistant = assistantItems.find((it) => it.streaming)
    if (!latestAssistant) {
      for (let i = assistantItems.length - 1; i >= 0; i--) {
        const a = assistantItems[i]
        if (a.usage || a.durationMs || a.turnDurationMs) {
          latestAssistant = a
          break
        }
      }
    }

    const promptTokens = latestAssistant?.usage?.promptTokens ?? 0
    const completionTokens = latestAssistant?.usage?.completionTokens ?? 0
    const cachedTokens = latestAssistant?.usage?.cachedTokens ?? 0
    const totalTokens = latestAssistant?.usage?.totalTokens ?? promptTokens + completionTokens

    let durationMs = 0
    if (latestAssistant?.streaming) {
      durationMs = Math.max(100, Date.now() - (latestAssistant.at || Date.now()))
    } else if (latestAssistant?.turnDurationMs) {
      durationMs = latestAssistant.turnDurationMs
    } else if (latestAssistant?.durationMs) {
      durationMs = latestAssistant.durationMs
    }

    const cacheHitRatio = promptTokens > 0 ? Math.min(100, Math.round((cachedTokens / promptTokens) * 100)) : 0

    let tokPerSec = 0
    if (durationMs > 200 && completionTokens > 0) {
      tokPerSec = Math.round(completionTokens / (durationMs / 1000))
    }

    const contextLimit = providerConfig.contextWindow || 128000
    const contextRatio = contextLimit > 0 ? Math.min(100, Math.round((promptTokens / contextLimit) * 100)) : 0

    const contextSummary = computeContextBreakdown({
      items,
      realPromptTokens: promptTokens,
      realCompletionTokens: completionTokens,
      realCachedTokens: cachedTokens,
      contextLimit,
    })

    return {
      turns,
      steps,
      tokPerSec,
      totalTokens,
      promptTokens,
      completionTokens,
      cachedTokens,
      cacheHitRatio,
      durationMs,
      contextLimit,
      contextRatio,
      contextSummary,
    }
  }, [thread, providerConfig])

  const currentModeOption = MODE_OPTIONS.find((m) => m.value === mode) || MODE_OPTIONS[0]
  const currentApprovalOption = APPROVAL_OPTIONS.find((a) => a.value === approvalMode) || APPROVAL_OPTIONS[0]
  const currentEffortOption = EFFORT_OPTIONS.find((e) => e.value === effort) || EFFORT_OPTIONS[0]

  // 子智能体独立会话：保持窗口对话 UI 风格，但不允许手动输入乱入
  if (thread?.isSubagent) {
    return (
      <div className="px-3 py-2 border-t border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] flex flex-col space-y-1.5 select-none relative transition-colors">
        <div className="flex items-center justify-between p-2.5 bg-white dark:bg-[#1a1a1e] border border-purple-200/80 dark:border-purple-900/40 rounded-xl shadow-xs">
          <div className="flex items-center space-x-2.5 min-w-0 flex-1">
            <div className="w-7 h-7 rounded-lg bg-purple-100 dark:bg-purple-950/60 flex items-center justify-center text-purple-600 dark:text-purple-400 flex-shrink-0">
              <Bot size={15} />
            </div>
            <div className="flex flex-col min-w-0">
              <div className="flex items-center space-x-1.5">
                <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                  子智能体专属执行会话
                </span>
                <span className="px-1.5 py-0.2 rounded text-[9.5px] bg-purple-100 dark:bg-purple-950/60 text-purple-700 dark:text-purple-300 border border-purple-200 dark:border-purple-800/50 font-medium">
                  独立工作区
                </span>
              </div>
              <span className="text-[11px] text-zinc-400 dark:text-zinc-500 truncate">
                由主 Agent 自动调度执行，保持会话独立只读，不允许手动输入
              </span>
            </div>
          </div>

          <div className="flex items-center space-x-2 flex-shrink-0 ml-2">
            {running ? (
              <button
                type="button"
                onClick={() => onAbort()}
                className="flex items-center space-x-1 px-2.5 py-1 bg-rose-600 hover:bg-rose-500 text-white rounded-lg transition-colors shadow-xs text-xs font-medium cursor-pointer"
                title="中止子智能体执行"
              >
                <Square size={11} className="fill-current" />
                <span>停止</span>
              </button>
            ) : null}
            {/* W6-T1：原"恢复执行"按钮已删除。
                它调的 `subagent.resume` 是桩（只回 {ok:true}），且它的
                `onResumeSubagent` 从来没有任何父组件传入——点了没反应。
                W4-T6 之后子智能体上下文刻意是临时的，语义上不存在"恢复"。 */}

            {thread.parentId && onSwitchThread && (
              <button
                type="button"
                onClick={() => onSwitchThread(thread.parentId!)}
                className="flex items-center space-x-1 px-2.5 py-1 bg-zinc-100 dark:bg-zinc-800 hover:bg-zinc-200 dark:hover:bg-zinc-700 text-zinc-700 dark:text-zinc-300 border border-zinc-200 dark:border-zinc-700 rounded-lg transition-colors text-xs font-medium cursor-pointer"
                title="返回所属主会话"
              >
                <ArrowLeft size={11} />
                <span>返回主会话</span>
              </button>
            )}
          </div>
        </div>

        {/* 底部遥测状态栏 */}
        <div className="flex items-center justify-start flex-wrap gap-x-2.5 gap-y-0.5 px-1.5 pt-0.5 pb-0.5 text-[10.5px] text-zinc-500 dark:text-zinc-400 font-sans select-none relative">
          <div className="flex items-center space-x-1 flex-shrink-0" title={`已进行 ${telemetry.turns} 轮对话`}>
            <Gauge size={12} className="text-zinc-400 dark:text-zinc-500" />
            <span>{telemetry.turns} 轮</span>
          </div>
          <span className="text-zinc-300 dark:text-zinc-700">|</span>
          <div className="flex items-center space-x-1 flex-shrink-0" title={`最后一次请求消耗的总 Token 数: ${telemetry.totalTokens.toLocaleString()} tok`}>
            <Database size={12} className="text-zinc-400 dark:text-zinc-500" />
            <span>{formatTokenShort(telemetry.totalTokens)} tok</span>
          </div>
          <span className="text-zinc-300 dark:text-zinc-700">|</span>
          <div className="flex items-center space-x-1 flex-shrink-0" title={`请求用时: ${telemetry.durationMs}ms`}>
            <Clock size={12} className="text-zinc-400 dark:text-zinc-500" />
            <span>用时 {formatDuration(telemetry.durationMs) || '0s'}</span>
          </div>
          <span className="text-zinc-300 dark:text-zinc-700">|</span>
          <div className="relative flex items-center">
            <button
              type="button"
              onClick={() => setContextPopoverOpen((prev) => !prev)}
              className={`flex items-center space-x-1.5 px-1.5 py-0.5 rounded transition-all cursor-pointer ${
                contextPopoverOpen
                  ? 'bg-zinc-200 dark:bg-zinc-800 text-zinc-900 dark:text-zinc-100 font-medium'
                  : 'hover:bg-zinc-100 dark:hover:bg-zinc-800/80 text-zinc-600 dark:text-zinc-400'
              }`}
              title={`当前提示词上下文占用 ${telemetry.promptTokens} / ${telemetry.contextLimit} Token (${telemetry.contextRatio}%)，点击查看细分构成与深度洞察`}
            >
              <PieChart
                size={12}
                className={
                  telemetry.contextRatio >= 85
                    ? 'text-rose-500'
                    : telemetry.contextRatio >= 75
                    ? 'text-amber-500'
                    : 'text-purple-500 dark:text-purple-400'
                }
              />
              <span className="font-medium">
                上下文 {telemetry.contextRatio === 0 && telemetry.promptTokens > 0 ? '<1%' : `${telemetry.contextRatio}%`}
              </span>
              <div className="w-6 h-1.5 rounded-full bg-zinc-200 dark:bg-zinc-800 overflow-hidden flex-shrink-0">
                <div
                  className={`h-full transition-all duration-300 ${
                    telemetry.contextRatio >= 85
                      ? 'bg-rose-500'
                      : telemetry.contextRatio >= 75
                      ? 'bg-amber-500'
                      : 'bg-purple-500 dark:bg-purple-400'
                  }`}
                  style={{ width: `${Math.max(4, Math.min(100, telemetry.contextRatio))}%` }}
                />
              </div>
            </button>

            {contextPopoverOpen && (
              <ContextUsagePopover
                summary={telemetry.contextSummary}
                telemetry={telemetry}
                onClose={() => setContextPopoverOpen(false)}
              />
            )}
          </div>
        </div>
      </div>
    )
  }

  return (
    <div
      className={`select-none relative transition-colors ${
        centered
          ? 'w-full flex flex-col space-y-2'
          : 'px-3 py-1.5 border-t border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] flex flex-col space-y-1'
      }`}
    >
      {/* 0. 会话排队发送队列面板 */}
      {!centered && (
        <QueuedMessagesFloatingPanel
          queue={queue}
          onPromote={(idx) => onPromoteQueueItem?.(idx)}
          onRemove={(idx) => onRemoveQueueItem?.(idx)}
          onClear={() => onClearQueue?.()}
          onEditItem={(editTxt, editImgs) => {
            setText(editTxt)
            if (editImgs && editImgs.length > 0) {
              setImages(editImgs)
            }
            textareaRef.current?.focus()
          }}
        />
      )}

      {/* 1. 待回答提问浮动置顶面板（对齐原版 PendingQuestionsFloatingPanel） */}
      {pendingQuestion && onAnswerQuestion && (
        <div className="p-2 bg-amber-50 dark:bg-amber-950/30 border border-amber-300 dark:border-amber-600/40 rounded-xl shadow-lg flex flex-col space-y-1.5 animate-in fade-in slide-in-from-bottom-2 duration-150">
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-2">
              <Sparkles size={13} className="text-amber-600 dark:text-amber-400" />
              <span className="text-xs font-semibold text-amber-900 dark:text-amber-200">智能体提问</span>
              <span className="px-1.5 py-0.2 rounded-full text-[10px] font-medium bg-amber-200/80 dark:bg-amber-500/20 text-amber-800 dark:text-amber-300">
                待你回答后继续执行
              </span>
            </div>
          </div>
          <div className="text-xs text-zinc-800 dark:text-zinc-200 font-medium">
            {pendingQuestion.question}
          </div>
          {pendingQuestion.options && pendingQuestion.options.length > 0 && (
            <div className="flex flex-wrap gap-1.5 pt-0.5">
              {pendingQuestion.options.map((opt) => (
                <button
                  key={opt.value}
                  onClick={() => onAnswerQuestion(pendingQuestion.callId, opt.value)}
                  className="px-2 py-1 rounded-lg bg-white dark:bg-amber-900/40 hover:bg-amber-100 dark:hover:bg-amber-800/50 border border-amber-300/80 dark:border-amber-700/50 text-amber-900 dark:text-amber-100 text-xs font-medium transition-colors shadow-xs"
                >
                  {opt.label}
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      {/* 2. 待发送图片预览条 */}
      {images.length > 0 && (
        <div className="flex items-center space-x-2 pb-0.5 overflow-x-auto">
          {images.map((img, idx) => (
            <div
              key={idx}
              className="relative group w-11 h-11 rounded-lg overflow-hidden border border-zinc-300 dark:border-zinc-700 flex-shrink-0 shadow-xs"
            >
              <img src={img} alt="preview" className="w-full h-full object-cover" />
              <button
                onClick={() => setImages((prev) => prev.filter((_, i) => i !== idx))}
                className="absolute top-0 right-0 p-0.5 bg-black/70 text-white rounded-bl opacity-0 group-hover:opacity-100 transition-opacity"
                title="移除图片"
              >
                <X size={10} />
              </button>
            </div>
          ))}
        </div>
      )}

      {/* 3. 主输入卡片容器 */}
      <div
        className={`relative flex flex-col bg-white dark:bg-[#1c1c20] border transition-all ${
          centered
            ? 'rounded-2xl shadow-xl shadow-zinc-200/50 dark:shadow-none'
            : 'rounded-xl shadow-xs'
        } ${
          isFocused
            ? 'border-blue-500/80 ring-2 ring-blue-500/15 dark:ring-blue-500/20'
            : 'border-zinc-300 dark:border-[#2f2f35] hover:border-zinc-400 dark:hover:border-zinc-600'
        }`}
        onDrop={handleDrop}
        onDragOver={(e) => e.preventDefault()}
      >
        {/* 动态 @ 提及搜索菜单 */}
        {mentionOpen && (
          <MentionMenu
            filterQuery={mentionQuery}
            onSelect={handleSelectMention}
            onClose={() => setMentionOpen(false)}
          />
        )}

        {/* 动态 / 快捷指令搜索菜单 */}
        {slashMenuOpen && (
          <SlashCommandMenu
            filterQuery={slashQuery}
            onSelect={handleSelectSlash}
            onClose={() => setSlashMenuOpen(false)}
            onOpenSettings={onOpenSettings}
            onOpenPlugins={onOpenPlugins || (() => {})}
            onOpenChanges={onOpenChanges || (() => {})}
            onCompact={onCompact || (() => {})}
            onNewThread={onNewThread || (() => {})}
            onSetMode={onSetMode}
          />
        )}

        {/* 选中的斜杠指令标签 */}
        {selectedCommand && (
          <div className="flex items-center space-x-1.5 mx-2.5 mt-1.5 px-2 py-0.2 rounded-md bg-blue-50 dark:bg-blue-900/30 border border-blue-200 dark:border-blue-700/50 w-fit text-blue-700 dark:text-blue-300 text-[11px] font-mono">
            <Slash size={10} />
            <span>{selectedCommand}</span>
            <button
              onClick={() => setSelectedCommand(null)}
              className="ml-0.5 hover:text-rose-500 transition-colors"
              title="移除指令"
            >
              <X size={10} />
            </button>
          </div>
        )}

        {/* 文本输入区域 */}
        <textarea
          ref={textareaRef}
          value={text}
          onChange={(e) => handleTextChange(e.target.value)}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          onFocus={() => setIsFocused(true)}
          onBlur={() => setIsFocused(false)}
          placeholder={
            running
              ? '当前任务正在执行中，可继续输入追加指令…'
              : mode === 'plan'
              ? '描述任务目标 (Plan 规划模式：仅作架构设计与只读分析)…'
              : mode === 'create'
              ? '描述任务目标 (Create 创造模式：探索架构创新与工具进化)…'
              : '向 a_da 提问、指派编程任务，或输入 / 唤起快捷指令…'
          }
          rows={centered ? 2 : 1}
          className={`w-full px-3.5 pt-2.5 pb-1.5 text-xs md:text-sm text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 dark:placeholder-zinc-500 bg-transparent resize-none outline-none leading-relaxed selectable ${
            centered ? 'min-h-[64px] max-h-[220px]' : 'max-h-[160px]'
          }`}
        />

        {/* 底部控制工具栏 */}
        <div className="flex items-center justify-between px-2 pb-1.5 pt-0.5 border-t border-zinc-100 dark:border-[#26262a]">
          {/* 左侧控制药丸群 */}
          <div className="flex items-center space-x-1.5 flex-wrap gap-y-1">
            {/* 加号功能菜单 */}
            <div className="relative" ref={plusMenuRef}>
              <button
                type="button"
                onClick={() => setPlusMenuOpen(!plusMenuOpen)}
                className={`p-1.5 rounded-lg text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors ${
                  plusMenuOpen ? 'bg-zinc-200 dark:bg-zinc-800 text-zinc-900 dark:text-white' : ''
                }`}
                title="选择模式、提及或添加附件"
              >
                <Plus size={15} />
              </button>

              {/* 加号弹出选择菜单 */}
              {plusMenuOpen && (
                <div className="absolute bottom-full left-0 mb-2 w-60 bg-white dark:bg-[#1f1f23] border border-zinc-200 dark:border-[#333338] rounded-xl shadow-2xl p-1.5 z-50 text-xs space-y-1 backdrop-blur-md">
                  <div className="px-2 py-1 text-[10px] font-semibold text-zinc-400 dark:text-zinc-500 uppercase tracking-wider">
                    协作模式切换
                  </div>
                  {MODE_OPTIONS.map((m) => {
                    const IconComp = m.icon
                    const isSelected = mode === m.value
                    return (
                      <button
                        key={m.value}
                        onClick={() => {
                          onSetMode(m.value)
                          setPlusMenuOpen(false)
                        }}
                        className={`w-full flex items-center space-x-2 px-2.5 py-1.5 rounded-lg text-left transition-colors ${
                          isSelected
                            ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium'
                            : 'text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                        }`}
                      >
                        <IconComp size={14} className="text-blue-500" />
                        <div className="flex-1">
                          <div>{m.label}</div>
                          <div className="text-[10px] text-zinc-400 dark:text-zinc-500">{m.desc}</div>
                        </div>
                      </button>
                    )
                  })}

                  <div className="h-[1px] bg-zinc-200 dark:bg-zinc-800 my-1" />

                  <div className="px-2 py-1 text-[10px] font-semibold text-zinc-400 dark:text-zinc-500 uppercase tracking-wider">
                    快捷指令与提及
                  </div>
                  {SLASH_COMMANDS.map((sc) => (
                    <button
                      key={sc.command}
                      onClick={() => {
                        setSelectedCommand(sc.command)
                        setPlusMenuOpen(false)
                        textareaRef.current?.focus()
                      }}
                      className="w-full flex items-center space-x-2 px-2.5 py-1.5 rounded-lg text-left text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
                    >
                      <Slash size={13} className="text-blue-500" />
                      <div className="flex-1">
                        <span className="font-mono">{sc.command}</span>
                        <span className="text-[10px] text-zinc-400 dark:text-zinc-500 ml-1.5">({sc.label})</span>
                      </div>
                    </button>
                  ))}

                  <button
                    onClick={() => {
                      insertPrefix('@')
                      setMentionOpen(true)
                      setMentionQuery('')
                      setPlusMenuOpen(false)
                    }}
                    className="w-full flex items-center space-x-2 px-2.5 py-1.5 rounded-lg text-left text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
                  >
                    <AtSign size={13} className="text-amber-500" />
                    <span>提及文件或技能 (@)</span>
                  </button>

                  <div className="h-[1px] bg-zinc-200 dark:bg-zinc-800 my-1" />

                  <div className="px-2 py-1 text-[10px] font-semibold text-zinc-400 dark:text-zinc-500 uppercase tracking-wider">
                    附件资源
                  </div>
                  <button
                    onClick={() => {
                      setPlusMenuOpen(false)
                      setFilePickerState({
                        isOpen: true,
                        title: '选择图片附件',
                        filterExts: ['.png', '.jpg', '.jpeg', '.gif', '.webp', '.svg', '.bmp', '.ico'],
                        type: 'image',
                      })
                    }}
                    className="w-full flex items-center space-x-2 px-2.5 py-1.5 rounded-lg text-left text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors cursor-pointer"
                  >
                    <ImageIcon size={13} className="text-indigo-500" />
                    <span>添加图片附件</span>
                  </button>

                  <button
                    onClick={() => {
                      setPlusMenuOpen(false)
                      setFilePickerState({
                        isOpen: true,
                        title: '选择引用本地文件',
                        type: 'file',
                      })
                    }}
                    className="w-full flex items-center space-x-2 px-2.5 py-1.5 rounded-lg text-left text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors cursor-pointer"
                  >
                    <FileCode size={13} className="text-cyan-500" />
                    <span>引用本地文件</span>
                  </button>
                </div>
              )}
            </div>

            {/* 模式切换快捷药丸（仅展示 SVG 图标，适配 H5 布局） */}
            <div className="relative" ref={modeMenuRef}>
              <button
                type="button"
                onClick={() => setModeMenuOpen(!modeMenuOpen)}
                className="p-1.5 rounded-md bg-zinc-100 dark:bg-zinc-800/80 hover:bg-zinc-200 dark:hover:bg-zinc-700/80 text-zinc-700 dark:text-zinc-300 transition-colors border border-zinc-200 dark:border-zinc-700/50 cursor-pointer"
                title={`当前模式: ${currentModeOption.label}。点击切换`}
                aria-label={`当前模式: ${currentModeOption.label}`}
              >
                <currentModeOption.icon size={13} className="text-blue-500" />
              </button>

              {modeMenuOpen && (
                <div className="absolute bottom-full left-0 mb-2 w-48 bg-white dark:bg-[#1f1f23] border border-zinc-200 dark:border-[#333338] rounded-xl shadow-2xl p-1 z-50 text-xs space-y-0.5">
                  {MODE_OPTIONS.map((m) => (
                    <button
                      key={m.value}
                      onClick={() => {
                        onSetMode(m.value)
                        setModeMenuOpen(false)
                      }}
                      className={`w-full flex items-center space-x-2 px-2 py-1.5 rounded-lg text-left ${
                        mode === m.value
                          ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium'
                          : 'text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                      }`}
                    >
                      <m.icon size={13} className="text-blue-500" />
                      <span>{m.label}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>

            {/* 模型选择配置药丸（仅展示 SVG 图标，适配 H5 布局） */}
            <div className="relative" ref={modelMenuRef}>
              <button
                type="button"
                onClick={() => setModelMenuOpen(!modelMenuOpen)}
                className="p-1.5 rounded-md bg-zinc-100 dark:bg-zinc-800/80 hover:bg-zinc-200 dark:hover:bg-zinc-700/80 text-zinc-700 dark:text-zinc-300 transition-colors border border-zinc-200 dark:border-zinc-700/50 cursor-pointer"
                title={`当前模型: ${providerConfig.model || 'gpt-4o'}。点击快速切换或调整`}
                aria-label={`当前模型: ${providerConfig.model || 'gpt-4o'}`}
              >
                <Sparkles size={13} className="text-blue-500" />
              </button>

              {modelMenuOpen && (
                <div className="absolute bottom-full left-0 mb-2 w-56 bg-white dark:bg-[#1f1f23] border border-zinc-200 dark:border-[#333338] rounded-xl shadow-2xl p-1 z-50 text-xs space-y-0.5">
                  {(() => {
                    const provs = agentClient.snapshot.providers || []
                    const activePId = agentClient.snapshot.activeProviderId || ''
                    const curProv = provs.find((p) => p.id === activePId) || provs[0]
                    const models = curProv?.models || []
                    return (
                      <>
                        <div className="px-2.5 py-1 text-[10px] text-zinc-400 dark:text-zinc-500 font-medium border-b border-zinc-100 dark:border-zinc-800 flex items-center justify-between">
                          <span className="truncate">供应商: {curProv?.name || '默认'}</span>
                          <span className="text-[9px] uppercase px-1 rounded bg-zinc-100 dark:bg-zinc-800 text-zinc-500">
                            {curProv?.protocol === 'anthropic' ? 'Anthropic' : curProv?.protocol === 'openai_responses' ? 'Responses' : 'Chat'}
                          </span>
                        </div>

                        <div className="max-h-40 overflow-y-auto space-y-0.5 py-0.5">
                          {models.map((m) => {
                            const isSelected = providerConfig.model === m.id
                            return (
                              <button
                                key={m.id}
                                onClick={async () => {
                                  if (curProv) {
                                    await agentClient.setActiveProvider(curProv.id, m.id)
                                  }
                                  setModelMenuOpen(false)
                                }}
                                className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-left transition-colors ${
                                  isSelected
                                    ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium'
                                    : 'text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                                }`}
                              >
                                <span className="font-mono truncate">{m.id}</span>
                                {isSelected && <Check size={11} className="text-blue-500" />}
                              </button>
                            )
                          })}
                        </div>

                        <div className="pt-1 border-t border-zinc-100 dark:border-zinc-800">
                          <button
                            onClick={() => {
                              setModelMenuOpen(false)
                              onOpenSettings()
                            }}
                            className="w-full flex items-center space-x-1.5 px-2.5 py-1.5 rounded-lg text-left text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
                          >
                            <Sliders size={12} className="text-zinc-500" />
                            <span>管理大模型供应商...</span>
                          </button>
                        </div>
                      </>
                    )
                  })()}
                </div>
              )}
            </div>

            {/* 权限审批策略药丸（仅展示 SVG 图标，适配 H5 布局） */}
            <div className="relative" ref={approvalMenuRef}>
              <button
                type="button"
                onClick={() => setApprovalMenuOpen(!approvalMenuOpen)}
                className="p-1.5 rounded-md bg-zinc-100 dark:bg-zinc-800/80 hover:bg-zinc-200 dark:hover:bg-zinc-700/80 text-zinc-700 dark:text-zinc-300 transition-colors border border-zinc-200 dark:border-zinc-700/50 cursor-pointer"
                title={`审批权限: ${currentApprovalOption.label}。点击切换`}
                aria-label={`审批权限: ${currentApprovalOption.label}`}
              >
                <Shield size={13} className="text-emerald-500" />
              </button>

              {approvalMenuOpen && (
                <div className="absolute bottom-full left-0 mb-2 w-52 bg-white dark:bg-[#1f1f23] border border-zinc-200 dark:border-[#333338] rounded-xl shadow-2xl p-1 z-50 text-xs space-y-0.5">
                  {APPROVAL_OPTIONS.map((a) => (
                    <button
                      key={a.value}
                      onClick={() => {
                        onSetApprovalMode?.(a.value)
                        setApprovalMenuOpen(false)
                      }}
                      className={`w-full flex items-center space-x-2 px-2 py-1.5 rounded-lg text-left ${
                        approvalMode === a.value
                          ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium'
                          : 'text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                      }`}
                    >
                      <Shield size={12} className="text-emerald-500" />
                      <div>
                        <div>{a.label}</div>
                        <div className="text-[10px] text-zinc-400 dark:text-zinc-500">{a.desc}</div>
                      </div>
                    </button>
                  ))}
                </div>
              )}
            </div>

            {/* 思考深度药丸（仅展示 SVG 图标，适配 H5 布局） */}
            <div className="relative" ref={effortMenuRef}>
              <button
                type="button"
                onClick={() => setEffortMenuOpen(!effortMenuOpen)}
                className="p-1.5 rounded-md bg-zinc-100 dark:bg-zinc-800/80 hover:bg-zinc-200 dark:hover:bg-zinc-700/80 text-zinc-700 dark:text-zinc-300 transition-colors border border-zinc-200 dark:border-zinc-700/50 cursor-pointer"
                title={`思考深度: ${currentEffortOption.label}。点击切换`}
                aria-label={`思考深度: ${currentEffortOption.label}`}
              >
                <Brain size={13} className="text-purple-500" />
              </button>

              {effortMenuOpen && (
                <div className="absolute bottom-full left-0 mb-2 w-52 bg-white dark:bg-[#1f1f23] border border-zinc-200 dark:border-[#333338] rounded-xl shadow-2xl p-1 z-50 text-xs space-y-0.5">
                  {EFFORT_OPTIONS.map((e) => (
                    <button
                      key={e.value}
                      onClick={() => {
                        onSetEffort?.(e.value)
                        setEffortMenuOpen(false)
                      }}
                      className={`w-full flex items-center space-x-2 px-2 py-1.5 rounded-lg text-left ${
                        effort === e.value
                          ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium'
                          : 'text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                      }`}
                    >
                      <Brain size={12} className="text-purple-500" />
                      <div>
                        <div>{e.label}</div>
                        <div className="text-[10px] text-zinc-400 dark:text-zinc-500">{e.desc}</div>
                      </div>
                    </button>
                  ))}
                </div>
              )}
            </div>
          </div>

          {/* 右侧：发送 / 排队与停止操作区（参考 src/ui 原版设计：运行中同时显示停止与排队发送按钮） */}
          <div className="flex items-center space-x-1.5 ml-2">
            {running && (
              <button
                type="button"
                onClick={() => onAbort()}
                className="p-1.5 bg-rose-500/15 hover:bg-rose-500/25 text-rose-600 dark:text-rose-400 border border-rose-500/30 rounded-lg transition-colors cursor-pointer shadow-2xs flex items-center justify-center"
                title="中止当前执行 (ESC)"
                aria-label="中止当前执行"
              >
                <Square size={13} className="fill-current" />
              </button>
            )}

            <button
              type="button"
              onClick={handleSend}
              disabled={!text.trim() && images.length === 0 && !selectedCommand}
              className={`p-1.5 rounded-lg transition-all flex items-center justify-center shadow-sm ${
                text.trim() || images.length > 0 || selectedCommand
                  ? 'bg-blue-600 hover:bg-blue-500 text-white cursor-pointer active:scale-95'
                  : 'bg-zinc-200 dark:bg-zinc-800 text-zinc-400 dark:text-zinc-500 cursor-not-allowed'
              }`}
              title={running ? '排队这条指令 (Enter)' : '发送指令 (Enter)'}
              aria-label={running ? '排队这条指令' : '发送'}
            >
              <ArrowUp size={15} strokeWidth={2.5} />
            </button>
          </div>
        </div>
      </div>

      {/* 4. 底部遥测状态栏（展示轮次、tok、用时，仅非居中时展示） */}
      {!centered && (
        <div className="flex items-center justify-start flex-wrap gap-x-2.5 gap-y-0.5 px-1.5 pt-0.5 pb-0.5 text-[10.5px] text-zinc-500 dark:text-zinc-400 font-sans select-none relative">
          {/* 1. 轮次 */}
          <div
            className="flex items-center space-x-1 flex-shrink-0"
            title={`已进行 ${telemetry.turns} 轮对话`}
          >
            <Gauge size={12} className="text-zinc-400 dark:text-zinc-500" />
            <span>{telemetry.turns} 轮</span>
          </div>

          <span className="text-zinc-300 dark:text-zinc-700">|</span>

          {/* 2. tok */}
          <div
            className="flex items-center space-x-1 flex-shrink-0"
            title={`最后一次请求消耗的总 Token 数: ${telemetry.totalTokens.toLocaleString()} tok`}
          >
            <Database size={12} className="text-zinc-400 dark:text-zinc-500" />
            <span>{formatTokenShort(telemetry.totalTokens)} tok</span>
          </div>

          <span className="text-zinc-300 dark:text-zinc-700">|</span>

          {/* 3. 用时 */}
          <div
            className="flex items-center space-x-1 flex-shrink-0"
            title={`最后一次生成请求所用时间: ${telemetry.durationMs}ms`}
          >
            <Clock size={12} className="text-zinc-400 dark:text-zinc-500" />
            <span>用时 {formatDuration(telemetry.durationMs) || '0s'}</span>
          </div>

          <span className="text-zinc-300 dark:text-zinc-700">|</span>

          {/* 4. 上下文入口（点击展开浮窗展示详细指标与深度洞察） */}
          <div className="relative flex items-center">
            <button
              type="button"
              data-testid="telemetry-context-ratio"
              onClick={() => setContextPopoverOpen((prev) => !prev)}
              className={`flex items-center space-x-1.5 px-1.5 py-0.5 rounded transition-all cursor-pointer ${
                contextPopoverOpen
                  ? 'bg-zinc-200 dark:bg-zinc-800 text-zinc-900 dark:text-zinc-100 font-medium'
                  : 'hover:bg-zinc-100 dark:hover:bg-zinc-800/80 text-zinc-600 dark:text-zinc-400'
              }`}
              title={`当前提示词上下文占用 ${telemetry.promptTokens} / ${telemetry.contextLimit} Token (${telemetry.contextRatio}%)，点击查看构成细分与深度洞察`}
            >
              <PieChart
                size={12}
                className={
                  telemetry.contextRatio >= 85
                    ? 'text-rose-500'
                    : telemetry.contextRatio >= 75
                    ? 'text-amber-500'
                    : 'text-blue-500 dark:text-blue-400'
                }
              />
              <span className="font-medium">
                上下文 {telemetry.contextRatio === 0 && telemetry.promptTokens > 0 ? '<1%' : `${telemetry.contextRatio}%`}
              </span>
              {/* 微型进度条 */}
              <div className="w-7 h-1.5 rounded-full bg-zinc-200 dark:bg-zinc-800 overflow-hidden flex-shrink-0">
                <div
                  className={`h-full transition-all duration-300 ${
                    telemetry.contextRatio >= 85
                      ? 'bg-rose-500'
                      : telemetry.contextRatio >= 75
                      ? 'bg-amber-500'
                      : 'bg-blue-500 dark:bg-blue-400'
                  }`}
                  style={{ width: `${Math.max(4, Math.min(100, telemetry.contextRatio))}%` }}
                />
              </div>
            </button>

            {/* 上下文深度洞察悬浮卡片 */}
            {contextPopoverOpen && (
              <ContextUsagePopover
                summary={telemetry.contextSummary}
                telemetry={telemetry}
                onClose={() => setContextPopoverOpen(false)}
                onCompact={onCompact}
              />
            )}
          </div>

          {/* 改动审查快捷按钮 */}
          {onOpenChanges && (
            <>
              <span className="text-zinc-300 dark:text-zinc-700">|</span>
              <button
                type="button"
                onClick={onOpenChanges}
                className={`flex items-center space-x-1 px-1.5 py-0.5 rounded transition-all cursor-pointer ${
                  activeChangeCount > 0
                    ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium hover:bg-blue-500/20'
                    : 'text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                }`}
                title="打开改动审查面板"
              >
                <History size={12} className={activeChangeCount > 0 ? 'text-blue-500' : 'text-zinc-400'} />
                <span>改动{activeChangeCount > 0 ? ` (${activeChangeCount})` : ''}</span>
              </button>
            </>
          )}

          {/* 调试面板快捷按钮 */}
          {onOpenDebug && (
            <>
              <span className="text-zinc-300 dark:text-zinc-700">|</span>
              <button
                type="button"
                onClick={onOpenDebug}
                className="flex items-center space-x-1 px-1.5 py-0.5 rounded text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-all cursor-pointer"
                title="打开通信与事件调试面板"
              >
                <TerminalIcon size={12} className="text-zinc-400" />
                <span>调试</span>
              </button>
            </>
          )}
        </div>
      )}

      {/* 跨平台自建文件/图片附件选择浮窗（避免原生弹窗导致 H5 无法兼容） */}
      {filePickerState.isOpen && (
        <FilePicker
          isOpen={filePickerState.isOpen}
          mode="files"
          title={filePickerState.title}
          filterExts={filePickerState.filterExts}
          startPath={thread?.workspace}
          onPicked={async (paths) => {
            if (filePickerState.type === 'image') {
              for (const p of paths) {
                try {
                  const res = await agentClient.readFileBase64(p)
                  if (res?.dataUri) {
                    setImages((prev) => [...prev, res.dataUri])
                  }
                } catch (err: any) {
                  console.error('读取图片附件失败:', err)
                }
              }
            } else {
              if (paths.length > 0) {
                const ws = thread?.workspace
                const formatted = paths.map((p) => {
                  if (ws && p.startsWith(ws)) {
                    const rel = p.slice(ws.length).replace(/^[\\/]+/, '')
                    return rel || p
                  }
                  return p
                })
                const tags = formatted.map((f) => `[文件: ${f}]`).join(' ')
                insertPrefix(tags)
              }
            }
            setFilePickerState((prev) => ({ ...prev, isOpen: false }))
          }}
          onClose={() => setFilePickerState((prev) => ({ ...prev, isOpen: false }))}
        />
      )}
    </div>
  )
}
