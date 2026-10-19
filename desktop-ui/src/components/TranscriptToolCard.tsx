import React, { useEffect, useMemo, useState } from 'react'
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
  Clock,
  RotateCcw,
  CheckCircle2,
  XCircle,
  Edit3,
  PackagePlus,
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  ShieldAlert,
  ArrowUp,
  ExternalLink,
} from 'lucide-react'
import type { Item } from '../types'
import { agentClient } from '../client/ws-client'
import { normalizeQuestionData } from '../utils/question-data'
import { MarkdownRenderer } from './MarkdownRenderer'
import { AskUserQuestionRecord } from './AskUserPrompt'
import { TOOL_LABEL } from '../utils/tool-labels'
import { approvalPreviewOf } from '../utils/pending-approvals'
import { notify } from './ToastHost'
import { CopyButton } from './CopyButton'
import { formatTokenShort } from '../utils/format'
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
    case 'create_plugin':
      return <PackagePlus size={size} className={className} />
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
    case 'git':
      return <FolderGit2 size={size} className={className} />
    default:
      return <Terminal size={size} className={className} />
  }
}

/** 路径工具集合 */
const PATH_TOOLS = new Set(['read_file', 'write_file', 'edit_file', 'list_files', 'code_outline'])

/** 智能拆分目标路径：文件名是正文，目录是暗色小字 */
function toolTarget(name: string, args: any): { target: string; dir: string } {
  // git 是**一个**工具，读哪一行取决于 `action`：卡片上写"git"等于什么都没说，
  // 用户要看到的是"它在看状态、还是要把我的改动提交掉"。
  if (name === 'git') {
    const action = String(args?.action || 'status').toLowerCase()
    const file = args?.file || args?.path
    const ref = args?.ref || args?.branch || ''
    const message = String(args?.message || '').slice(0, 40)
    const label: Record<string, string> = {
      status: '查看仓库改动',
      diff: file ? `查看 ${file} 的改动` : '查看未暂存差异',
      log: `近期 ${args?.limit || 10} 次提交`,
      show: `查看提交 ${ref || 'HEAD'}`,
      blame: `追溯 ${file || '文件'} 的逐行作者`,
      add: file ? `暂存 ${file}` : '暂存全部改动',
      commit: message ? `提交：${message}` : '创建提交',
      checkout: `切换到 ${ref || '分支'}`,
      restore: `恢复 ${file || '文件'}`,
      reset: `重置到 ${ref || 'HEAD'}`,
      stash: `stash ${args?.sub || 'list'}`,
      branch: ref ? `分支 ${ref}` : '列出分支',
      merge: `合并 ${ref}`,
      push: `推送到 ${args?.remote || 'origin'}`,
      pull: `拉取 ${args?.remote || 'origin'}`,
    }
    return { target: label[action] || `git ${action}`, dir: '' }
  }
  if (name === 'run_tests') return { target: '执行测试套件', dir: '' }
  if (name === 'project_inspect') return { target: '探测技术栈与项目环境', dir: '' }
  if (name === 'create_plugin') return { target: args?.id ? `插件 ${args.id}` : '生成插件骨架', dir: '' }
  if (name === 'batch_write') {
    const count = Array.isArray(args?.files) ? args.files.length : 0
    return { target: `写入 ${count} 个文件`, dir: '' }
  }
  if (name === 'batch_replace') {
    const count = Array.isArray(args?.files) ? args.files.length : 0
    return { target: `在 ${count} 个文件中替换`, dir: '' }
  }
  if (name === 'invoke_subagent') {
    const role = args?.subagent_id || args?.role || '子智能体'
    const task = args?.task ? (String(args.task).length > 40 ? String(args.task).slice(0, 40) + '…' : String(args.task)) : ''
    const ws = args?.workspace ? ` (${String(args.workspace).replace(/\\/g, '/').split('/').pop() || args.workspace})` : ''
    return { target: `[${role}] ${task}${ws}`, dir: '' }
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
 * 运行中卡片的**实时计时**。
 *
 * 委派出去的子智能体动辄跑几分钟，中间可能长时间没有新内容落盘。没有这个秒表，
 * 用户看到的就是一张完全静止的卡片——分不清"它在干活"还是"它卡死了"。
 */
const LiveElapsed: React.FC<{ since?: number; className?: string }> = ({ since, className }) => {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [])
  if (!since) return null
  const secs = Math.max(0, Math.round((now - since) / 1000))
  const text =
    secs < 60 ? `${secs}s` : `${Math.floor(secs / 60)}m${String(secs % 60).padStart(2, '0')}s`
  return <span className={className}>已运行 {text}</span>
}

/**
 * 工具调用卡片（ToolCard）
 * 严格对齐 src/ui/Transcript.tsx ToolCard 哲学：
 * 采用浅色系次要字色与弱边框，和正文回复形成明晰的视觉层级区分
 */
export const ToolCard: React.FC<{
  item: Item
  onSelectThread?: (threadId: string) => void
  currentThreadId?: string
}> = ({ item, onSelectThread, currentThreadId }) => {
  const toolName = item.tool || item.name || 'tool'
  // 默认**收缩**：子智能体卡片内容很长（任务书 + 步骤 + 汇报），展开交给用户点。
  // 注意等待答复的工具（ask_user / 待批准）由下面的 `(open || isAwaiting)` 自动展开，
  // 所以收缩默认值不会把"要用户操作的卡片"藏起来。
  const [open, setOpen] = useState(false)
  const [showSubagentSteps, setShowSubagentSteps] = useState(false)
  const [replyText, setReplyText] = useState('')
  const toolStatus = item.state || item.status || 'done'
  const isAwaiting = toolStatus === 'waiting_approval' || toolStatus === 'awaiting'
  const isRunning = toolStatus === 'running'
  // `timeout` = 引擎不再等这个工具（策略超时切断）——必须当失败渲染，
  // 否则界面上会显示"执行完成"，而实际上工具根本没跑完。
  const isTimedOut = toolStatus === 'timeout'
  const isError =
    toolStatus === 'failed' || toolStatus === 'error' || isTimedOut || Boolean(item.error)

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

  // 子智能体回执里的两个关键诊断字段（`SubagentRunResult` 序列化在 details / result 上）：
  // - `errorMessage`：为什么失败（模型调用失败：<原文> / 步数上限 / 被取消 …）
  // - `promptTokens`：它跑过的最大 prompt 用量（上下文撑爆是首要失败根因）
  const subagentDetails: any = (item.details as any) || structuredData || {}
  const subagentErrorReason: string =
    (typeof subagentDetails.errorMessage === 'string' && subagentDetails.errorMessage.trim()) ||
    (typeof item.error === 'string' && item.error.trim()) ||
    ''
  const subagentPromptTokens: number | undefined =
    typeof subagentDetails.promptTokens === 'number' ? subagentDetails.promptTokens : undefined

  const rawQuestion = item.question || (item.details as any)?.question
  const isQuestionTool = toolName === 'ask_user' || Boolean(rawQuestion)

  // 提取子智能体委派相关属性
  const subagentRole =
    (item.args as any)?.subagent_id ||
    (item.args as any)?.role ||
    (item.details as any)?.subagent_id ||
    '子智能体'
  const subagentTask = (item.args as any)?.task || ''
  const subagentWorkspace =
    (item.args as any)?.workspace ||
    (item.details as any)?.workspace ||
    structuredData?.workspace

  // 解析并匹配对应的子智能体会话 ID
  //
  // 这里的兜底顺序是"点谁都是同一个会话"的老病根：早先有两条过宽的回落——
  // `!t.parentId`（任何没有父关系的子会话都算）与"同工作区的 coding 会话"（任何会话都算），
  // 于是一排子智能体卡片点进去可能全是同一个。现在的规则是**父子锚定 + 认领互斥**：
  // 只认本卡片名下（`parentId === 当前父会话`）且没有被别的卡片认领过的子会话；
  // 定位不到就如实返回 undefined（调用方给提示），绝不乱跳。
  const subagentThreadId = (() => {
    if (toolName !== 'invoke_subagent') return undefined
    const threads = agentClient.snapshot.threads || []

    // 1) 回执/监视任务里带了确切 id（最可靠）
    //
    // 🔴 但必须校验它确实是一条**存在**的会话：卡片回执里的 id 可能已经失效（子会话已回收），
    // 直接拿去开页签就会在标签栏里留下一个指向不存在会话的"无效子智能体"页签。
    const explicitId =
      (item.details as any)?.threadId ||
      (item.details as any)?.subagent_thread_id ||
      structuredData?.threadId ||
      structuredData?.subagent_thread_id ||
      (item.args as any)?.thread_id ||
      (item.args as any)?.threadId
    if (explicitId && threads.some((t) => t.id === explicitId)) return explicitId

    // 2) 已被**别的委派卡片**认领的子会话，本卡片不得再认领
    const claimed = new Set<string>()
    const parentThread = threads.find((t) => t.id === currentThreadId)
    for (const it of parentThread?.items || []) {
      if (it.id === item.id) continue
      const d: any = (it as any).details
      const bound = d?.threadId || d?.subagent_thread_id
      if (typeof bound === 'string' && bound) claimed.add(bound)
    }

    const approxTime = startedAt || item.createdAt || Date.now()
    const isMyChild = (t: any) =>
      Boolean(t.isSubagent) &&
      !claimed.has(t.id) &&
      ((currentThreadId && t.parentId === currentThreadId) ||
        (item.id && t.parentId === item.id))

    // 时间最接近本卡片开始时刻的那个（同一父会话可能先后派过多个子智能体）
    const nearestByTime = (list: any[]) =>
      list
        .slice()
        .sort(
          (a, b) =>
            Math.abs((a.createdAt || 0) - approxTime) - Math.abs((b.createdAt || 0) - approxTime)
        )[0]

    // 3) 本卡片名下：角色吻合优先，其次按时间取最近的
    const sameParent = threads.filter(isMyChild)
    const roleMatch = sameParent.filter(
      (t) => subagentRole && (t.subagentId === subagentRole || t.title?.includes(subagentRole))
    )
    if (roleMatch.length) return nearestByTime(roleMatch)?.id
    if (sameParent.length) return nearestByTime(sameParent)?.id

    // 4) 老数据兜底：父关系没落盘，只能靠"角色吻合 + 时间接近"（仍要求是子智能体会话且未被认领）
    const roleRecent = threads.filter(
      (t) =>
        Boolean(t.isSubagent) &&
        !claimed.has(t.id) &&
        subagentRole &&
        (t.subagentId === subagentRole || t.title?.includes(subagentRole)) &&
        Math.abs((t.createdAt || 0) - approxTime) < 30 * 60 * 1000
    )
    return nearestByTime(roleRecent)?.id
  })()

  // 独立打开或切换至子智能体会话
  const handleOpenSubagentThread = () => {
    if (!onSelectThread) return
    if (subagentThreadId) {
      onSelectThread(subagentThreadId)
      return
    }
    // 定位不到就**如实说**，绝不"随便挑一个子会话跳过去"——那正是"点谁都是同一个"的来源。
    // 同时催一次会话列表刷新（子会话可能刚落盘、还没进快照）。
    if ((agentClient as any).refreshThreads) {
      ;(agentClient as any).refreshThreads()
    }
    notify({ message: '还没定位到该子智能体的独立会话（可能尚未落盘），稍后再试', level: 'info' })
  }

  // 提取子智能体独立会话实体
  const subagentThread = subagentThreadId
    ? agentClient.snapshot.threads.find((t) => t.id === subagentThreadId) || null
    : null

  // 提取子智能体执行过程中调用的工具明细
  const subagentToolItems = useMemo(() => {
    if (!subagentThread) return []
    return subagentThread.items.filter(
      (it) => it.kind === 'toolCall' || it.kind === 'tool' || it.role === 'tool' || it.tool || it.callId
    )
  }, [subagentThread])

  // 提取子智能体产出的分析汇报文本
  const subagentReport = useMemo(() => {
    if (subagentThread) {
      const lastAssistant = subagentThread.items
        .slice()
        .reverse()
        .find(
          (it) =>
            it.kind === 'assistant' ||
            it.role === 'assistant' ||
            (it.text && it.kind !== 'thinking' && it.kind !== 'user' && it.kind !== 'toolCall')
        )
      if (lastAssistant?.text && lastAssistant.text.trim()) {
        return lastAssistant.text.trim()
      }
    }
    const raw = item.output || item.result
    if (typeof raw === 'string' && raw.trim()) return raw.trim()
    if (structuredData?.summary && typeof structuredData.summary === 'string' && structuredData.summary.trim()) {
      return structuredData.summary.trim()
    }
    return ''
  }, [subagentThread, item.output, item.result, structuredData])

  // 提取归一化问题对象
  const questionData = (() => {
    if (!isQuestionTool) return null
    // 台账里可能只有 args（提问事件还没到），拼一份最小形态给归一化器
    const raw = rawQuestion || (typeof item.args === 'object' && item.args ? item.args : {})
    const qText =
      raw?.question || '请回复智能体的提问'
    return normalizeQuestionData(
      { ...raw, question: qText },
      raw?.callId || item.callId || item.id
    )
  })()

  const parsedTarget = toolTarget(toolName, item.args)
  const target = parsedTarget.target
  const dir = parsedTarget.dir

  /// 审批条上"要批准什么"的预览（危险命令原文 / 目标路径 / 参数原文）。
  ///
  /// 与输入框上方的**审批浮窗**共用 `approvalPreviewOf`（`../utils/pending-approvals`）：
  /// 两处各写一份必然漂移，而"看不到命令的二次确认等于让人闭眼点批准"。
  const approvalPreview = isAwaiting && !isQuestionTool ? approvalPreviewOf(item) : ''

  // 状态与色彩映射：采用更加轻淡柔和的微色调
  const statusBadge = (() => {
    if (toolName === 'invoke_subagent') {
      if (isRunning) {
        return { label: '子代理执行中', color: 'text-purple-600 dark:text-purple-400 bg-purple-500/10 border-purple-500/20' }
      }
      if (isError) {
        return { label: '子代理失败', color: 'text-rose-500/90 bg-rose-500/10 border-rose-500/20' }
      }
      return { label: '子代理完成', color: 'text-emerald-600 dark:text-emerald-400 bg-emerald-500/10 border-emerald-500/20' }
    }
    if (isAwaiting) {
      if (isQuestionTool) {
        return { label: '待答复', color: 'text-blue-500/90 bg-blue-500/10 border-blue-500/20' }
      }
      return { label: '等待批准', color: 'text-amber-500/90 bg-amber-500/10 border-amber-500/20' }
    }
    if (isRunning) return { label: '执行中', color: 'text-blue-500/90 bg-blue-500/10 border-blue-500/20' }
    if (isError) return { label: '失败', color: 'text-rose-500/90 bg-rose-500/10 border-rose-500/20' }
    return null
  })()

  const hasDetail = Boolean(
    item.result ||
      item.output ||
      item.error ||
      item.patch ||
      item.args ||
      (isQuestionTool && isAwaiting) ||
      toolName === 'invoke_subagent'
  )

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
          {toolName === 'invoke_subagent' && onSelectThread && (
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                handleOpenSubagentThread()
              }}
              className="flex items-center space-x-1 px-1.5 py-0.5 rounded text-[10px] font-medium text-purple-600 dark:text-purple-400 hover:bg-purple-500/15 border border-purple-500/25 transition-colors cursor-pointer"
              title="切换至子智能体独立对话"
            >
              <span>查看子对话</span>
              <ExternalLink size={9} />
            </button>
          )}

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
            {/* 1. 提问卡：**操作面已上移到输入框上方的提问浮窗**，这里只留对话流里的记录
                   （问了什么、有哪些选项）。同审批：同一件事摆两套按钮，
                   用户会怀疑"点哪个才算数"——所以按钮只在一处。 */}
            {isAwaiting && isQuestionTool && questionData && (
              <AskUserQuestionRecord
                question={questionData}
                className="p-2.5 bg-blue-50/70 dark:bg-blue-950/25 border-b border-blue-200/60 dark:border-blue-900/40"
                hint="等待答复中 —— 请在输入框上方的提问浮窗作答"
              />
            )}

            {/* 2. 写工作区审批：**操作面已上移到输入框上方的浮动审批窗口**，
                   这里只留对话流里的记录（批的是什么 + 谁在处理）。
                   同一件事摆两套按钮，用户会怀疑"点哪个才算数"——所以按钮只在一处。 */}
            {isAwaiting && !isQuestionTool && (
              <div className="flex flex-col gap-2 p-2.5 bg-amber-500/10 border-b border-amber-500/20 text-xs">
                <div className="flex items-center space-x-2 text-amber-700 dark:text-amber-300 font-medium">
                  <ShieldAlert size={13} />
                  <span>这次调用会修改工作区，等待批准</span>
                </div>
                {/* 🔴 批准前必须能看清"批的是什么"：危险命令的原文 / 要动的目标。
                    宿主把参数原文随审批事件一起送来了（ApprovalRequested.args），
                    这里只负责摆出来——看不到命令的"二次确认"等于让人闭眼点批准。 */}
                {approvalPreview && (
                  <pre className="m-0 px-2 py-1.5 rounded bg-amber-950/10 dark:bg-black/30 border border-amber-500/20 text-[11px] font-mono whitespace-pre-wrap break-all text-amber-900 dark:text-amber-200">
                    {approvalPreview}
                  </pre>
                )}
                <div className="flex items-center space-x-1.5 text-[10.5px] text-amber-700/90 dark:text-amber-300/80">
                  <ArrowUp size={11} />
                  <span>请在输入框上方的审批浮窗中批准或拒绝（超时未处理将按安全方向自动拒绝）。</span>
                </div>
              </div>
            )}

            {/* 3. 子智能体委派专有详情卡片 */}
            {toolName === 'invoke_subagent' && (
              <div className="p-3 bg-purple-50/50 dark:bg-purple-950/20 border-b border-purple-200/50 dark:border-purple-900/30 space-y-3">
                <div className="flex items-center justify-between">
                  <div className="flex items-center space-x-2">
                    <div className="p-1.5 rounded-lg bg-purple-500/15 text-purple-600 dark:text-purple-400">
                      <Bot size={16} />
                    </div>
                    <div>
                      <div className="flex items-center space-x-1.5 text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                        <span>委派子智能体:</span>
                        <span className="font-mono text-purple-600 dark:text-purple-400 bg-purple-100/70 dark:bg-purple-900/40 px-1.5 py-0.5 rounded text-[11px]">
                          {subagentRole}
                        </span>
                        {/* 唯一短 id：同 profile 连派多个时，卡片之间靠它区分（title 里也是同一个 id） */}
                        {subagentThreadId && (
                          <span
                            className="font-mono text-[10px] text-zinc-500 dark:text-zinc-400 bg-zinc-100 dark:bg-zinc-800 px-1.5 py-0.5 rounded"
                            title={`子会话：${subagentThreadId}`}
                          >
                            #{subagentThreadId.slice(-6)}
                          </span>
                        )}
                      </div>
                      {subagentWorkspace && (
                        <div className="text-[10px] text-zinc-500 dark:text-zinc-400 flex items-center space-x-1 mt-0.5">
                          <Folder size={10} className="text-zinc-400" />
                          <span className="font-mono">{subagentWorkspace}</span>
                        </div>
                      )}
                    </div>
                  </div>

                  <div className="flex items-center space-x-2">
                    {isRunning ? (
                      <div className="flex items-center space-x-1.5 px-2 py-0.5 rounded-full bg-purple-100 dark:bg-purple-900/40 text-purple-600 dark:text-purple-300 text-[10.5px] font-medium animate-pulse">
                        <span className="w-1.5 h-1.5 rounded-full bg-purple-500 animate-ping" />
                        <span>正在独立执行中...</span>
                        <LiveElapsed since={startedAt || item.at} className="font-mono opacity-80" />
                      </div>
                    ) : isError ? (
                      <span className="px-2 py-0.5 rounded text-[10.5px] font-medium bg-rose-500/10 text-rose-600 dark:text-rose-400 border border-rose-500/20">
                        ✕ 执行失败
                      </span>
                    ) : (
                      <span className="px-2 py-0.5 rounded text-[10.5px] font-medium bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20">
                        ✓ 执行完成
                      </span>
                    )}

                    {onSelectThread && (
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation()
                          handleOpenSubagentThread()
                        }}
                        className="flex items-center space-x-1 px-2.5 py-1 rounded-md bg-purple-600 hover:bg-purple-500 text-white text-xs font-medium transition-colors shadow-xs hover:shadow cursor-pointer"
                        title="在顶部标签页中独立打开并查看该子智能体"
                      >
                        <Bot size={13} />
                        <span>在标签独立查看</span>
                        <ExternalLink size={11} />
                      </button>
                    )}
                  </div>
                </div>

                {subagentTask && (
                  <div className="bg-white/90 dark:bg-zinc-900/90 p-2.5 rounded-lg border border-purple-200/40 dark:border-purple-800/30 text-xs text-zinc-700 dark:text-zinc-300 font-sans leading-relaxed">
                    <div className="text-[10px] text-purple-600/80 dark:text-purple-400/80 font-mono font-medium mb-1 flex items-center space-x-1">
                      <Sparkles size={11} />
                      <span>任务目标与指令</span>
                    </div>
                    <div className="whitespace-pre-wrap">{subagentTask}</div>
                  </div>
                )}

                {/* 子智能体执行进展与工具调用列表 */}
                {subagentToolItems.length > 0 && (
                  <div className="border border-purple-200/50 dark:border-purple-900/40 rounded-lg overflow-hidden bg-white/70 dark:bg-black/25">
                    <button
                      type="button"
                      onClick={() => setShowSubagentSteps(!showSubagentSteps)}
                      className="w-full flex items-center justify-between px-2.5 py-1.5 text-xs text-zinc-700 dark:text-zinc-300 hover:bg-purple-50/60 dark:hover:bg-purple-950/30 transition-colors cursor-pointer"
                    >
                      <div className="flex items-center space-x-1.5">
                        <span className="text-zinc-400">
                          {showSubagentSteps ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                        </span>
                        <span className="font-medium">执行过程与工具调用明细</span>
                        <span className="px-1.5 py-0.2 rounded-full text-[10px] bg-purple-100 dark:bg-purple-900/40 text-purple-600 dark:text-purple-300 font-mono">
                          {subagentToolItems.length} 次操作
                        </span>
                      </div>
                      <span className="text-[10px] text-purple-600 dark:text-purple-400">
                        {showSubagentSteps ? '点击收起' : '展开查看调用步骤'}
                      </span>
                    </button>
                    {showSubagentSteps && (
                      <div className="p-2 space-y-1 max-h-56 overflow-y-auto border-t border-purple-100 dark:border-purple-900/30 bg-zinc-50/50 dark:bg-black/40">
                        {subagentToolItems.map((toolIt: any, idx: number) => {
                          const tName = toolIt.tool || toolIt.name || 'tool'
                          const tTarget = toolTarget(tName, toolIt.args).target
                          const tDuration = toolIt.durationMs
                          return (
                            <div
                              key={toolIt.id || idx}
                              className="flex items-center justify-between px-2 py-1 rounded bg-white dark:bg-zinc-900/90 border border-zinc-200/50 dark:border-zinc-800/50 text-[11px] font-mono"
                            >
                              <div className="flex items-center space-x-1.5 truncate min-w-0">
                                <span className="text-zinc-400">{renderToolIcon(tName, 11)}</span>
                                <span className="text-purple-700 dark:text-purple-300 font-medium">
                                  {TOOL_LABEL[tName] || tName}
                                </span>
                                {tTarget && (
                                  <span className="text-zinc-500 truncate text-[10.5px]">
                                    {tTarget}
                                  </span>
                                )}
                              </div>
                              <div className="flex items-center space-x-1.5 flex-shrink-0 text-[10px] text-zinc-400">
                                {tDuration && <span>{tDuration}ms</span>}
                                <span className="text-emerald-500">✓</span>
                              </div>
                            </div>
                          )
                        })}
                      </div>
                    )}
                  </div>
                )}

                {/* 子智能体产出的分析汇报正文（Markdown 渲染） */}
                {subagentReport ? (
                  <div className="space-y-1.5 pt-1">
                    <div className="flex items-center justify-between">
                      <div className="flex items-center space-x-1.5 text-xs font-semibold text-purple-700 dark:text-purple-300">
                        <Sparkles size={13} className="text-purple-500" />
                        <span>子智能体执行总结与汇报</span>
                      </div>
                      <div className="flex items-center space-x-1.5">
                        <CopyButton text={subagentReport} label="复制汇报内容" />
                        {subagentThreadId && onSelectThread && (
                          <button
                            type="button"
                            onClick={(e) => {
                              e.stopPropagation()
                              onSelectThread(subagentThreadId)
                            }}
                            className="flex items-center space-x-1 px-2 py-0.5 rounded text-[11px] font-medium bg-purple-600 hover:bg-purple-500 text-white transition-colors cursor-pointer shadow-xs"
                          >
                            <span>在独立会话中查看完整流</span>
                            <ExternalLink size={11} />
                          </button>
                        )}
                      </div>
                    </div>
                    <div className="p-3 bg-white/95 dark:bg-zinc-900/95 rounded-lg border border-purple-200/50 dark:border-purple-800/40 text-xs text-zinc-800 dark:text-zinc-200 select-text overflow-y-auto max-h-[460px] leading-relaxed">
                      <MarkdownRenderer content={subagentReport} />
                    </div>
                  </div>
                ) : isRunning ? (
                  <div className="p-3 bg-white/60 dark:bg-zinc-900/50 rounded-lg border border-purple-200/40 dark:border-purple-900/30 flex items-center justify-between text-xs text-purple-700 dark:text-purple-300">
                    <div className="flex items-center space-x-2">
                      <span className="w-2 h-2 rounded-full bg-purple-500 animate-ping" />
                      <span>
                        {subagentToolItems.length > 0
                          ? `子智能体已执行 ${subagentToolItems.length} 次工具操作，正在组织分析汇报...`
                          : '子智能体正在自主探索与分析中...'}
                      </span>
                    </div>
                    {subagentThreadId && onSelectThread && (
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation()
                          onSelectThread(subagentThreadId)
                        }}
                        className="text-[11px] underline text-purple-600 dark:text-purple-400 hover:text-purple-500 cursor-pointer"
                      >
                        实时查看子会话
                      </button>
                    )}
                  </div>
                ) : null}
              </div>
            )}

            {/* 4. 命令执行详情 */}
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

            {/* 5. 参数与输出展示：采用更浅更清爽的字色 */}
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
                  {subagentPromptTokens != null && (
                    <span className="flex items-center space-x-1" title="子智能体跑过的最大 prompt 用量（上下文规模）">
                      <span className="text-zinc-400 dark:text-zinc-500 font-sans">末轮上下文:</span>
                      <span
                        className={
                          subagentPromptTokens >= 200_000
                            ? 'text-amber-600 dark:text-amber-400 font-semibold'
                            : 'text-zinc-600 dark:text-zinc-300'
                        }
                      >
                        {formatTokenShort(subagentPromptTokens)}
                      </span>
                    </span>
                  )}
                </div>
              )}

              {/* 失败原因：必须直接写在卡片上，别让人去翻日志猜 */}
              {isError && subagentErrorReason && (
                <div className="flex items-start space-x-1.5 p-2 rounded bg-rose-500/10 border border-rose-500/20 text-[11px] text-rose-700 dark:text-rose-300">
                  <AlertTriangle size={12} className="mt-0.5 flex-shrink-0" />
                  <span className="font-sans break-all">
                    <span className="font-medium">失败原因：</span>
                    {subagentErrorReason}
                  </span>
                </div>
              )}

              {item.args && toolName !== 'run_command' && toolName !== 'invoke_subagent' && (
                <div>
                  <span className="text-zinc-400 dark:text-zinc-500 font-sans text-[10px]">输入参数：</span>
                  <pre className="text-zinc-500 dark:text-zinc-400 whitespace-pre-wrap mt-0.5 bg-zinc-100/50 dark:bg-black/20 p-2 rounded border border-zinc-200/40 dark:border-zinc-800/40">
                    {typeof item.args === 'string' ? item.args : JSON.stringify(item.args, null, 2)}
                  </pre>
                </div>
              )}

              {(item.result || item.output) && toolName !== 'invoke_subagent' && (
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
