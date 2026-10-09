import React from 'react'
import { Sparkles, Terminal, ShieldAlert, CheckCircle2, Search, ListTodo, GitPullRequest, Layers, AlertCircle, Code2 } from 'lucide-react'
import type { AgentMode, ProviderConfig, ApprovalMode, Effort, Thread, QueuedItem } from '../types'
import { WorkspaceSelector } from './WorkspaceSelector'
import { Composer } from './Composer'
import { agentClient } from '../client/ws-client'

const QUICK_CODING_PROMPTS = [
  {
    icon: Search,
    title: '诊断技术栈与架构',
    prompt: '请分析当前工作区的技术栈架构与核心依赖配置，给出项目概览。',
  },
  {
    icon: CheckCircle2,
    title: '运行测试并修复',
    prompt: '请运行项目单元测试，若有失败用例请深入归因并提出修复方案。',
  },
  {
    icon: Terminal,
    title: '审查 Git 改动',
    prompt: '请检查工作区当前的 Git 改动与状态，评估变更风险。',
  },
  {
    icon: ShieldAlert,
    title: '执行准入门禁检查',
    prompt: '请依据代码质量与测试覆盖标准，对当前工作区改动执行 check_gate 门禁判定。',
  },
]

const QUICK_PM_PROMPTS = [
  {
    icon: ListTodo,
    title: '拆解产品目标与任务清单',
    prompt: '请分析当前工作区，将核心目标拆解为结构化、可落地的任务清单与验收标准。',
  },
  {
    icon: GitPullRequest,
    title: '委派任务给 Coding Agent',
    prompt: '请将当前的待办任务分派委派给 coding agent 进行代码实现，明确输入与验证要求。',
  },
  {
    icon: Layers,
    title: '梳理工程架构与里程碑',
    prompt: '请评估当前项目的模块依赖与架构状态，制定后续版本迭代的里程碑规划。',
  },
  {
    icon: AlertCircle,
    title: '检查项目风险与阻塞点',
    prompt: '请汇总当前未完成任务与改动进展，分析潜在的技术阻塞与交付风险。',
  },
]

interface EmptyConversationViewProps {
  thread?: Thread
  mode: AgentMode
  running: boolean
  providerConfig: ProviderConfig
  approvalMode?: ApprovalMode
  effort?: Effort
  currentWorkspace: string
  allWorkspaces: string[]
  onSelectWorkspace: (workspace: string) => void
  onRemoveWorkspace?: (workspace: string) => void
  onOpenWorkspacePicker: () => void
  onSend: (text: string, images?: string[]) => void
  onAbort: () => void
  onSetMode: (mode: AgentMode) => void
  onSetApprovalMode?: (mode: ApprovalMode) => void
  onSetEffort?: (effort: Effort) => void
  onOpenSettings: () => void
  onOpenPlugins?: () => void
  onOpenChanges?: () => void
  onOpenDebug?: () => void
  onNewThread?: () => void
  onCompact?: () => void
}

/**
 * 新建对话居中视图（EmptyConversationView）
 * 
 * 严格按照用户与现代化设计规范：
 * 1. 输入框在会话界面上下垂直居中；
 * 2. 界面仅保留三大核心元素：
 *    - “你好，我是 a_da 智能编码助手” 标题与徽标
 *    - 工作区快速展示与选择控件
 *    - 居中自适应大输入框（带加号扩展、模式药丸与发送按钮）
 */
export const EmptyConversationView: React.FC<EmptyConversationViewProps> = ({
  thread,
  mode,
  running,
  providerConfig,
  approvalMode,
  effort,
  currentWorkspace,
  allWorkspaces,
  onSelectWorkspace,
  onRemoveWorkspace,
  onOpenWorkspacePicker,
  onSend,
  onAbort,
  onSetMode,
  onSetApprovalMode,
  onSetEffort,
  onOpenSettings,
  onOpenPlugins,
  onOpenChanges,
  onOpenDebug,
  onNewThread,
  onCompact,
}) => {
  const isPm = mode === 'pm' || thread?.mode === 'pm' || thread?.agentId === 'pm-assistant'
  const agentName = isPm ? '项目管理助手' : (agentClient.productInfo?.name || 'a_da 智能编码助手')
  const agentId = isPm ? 'pm-assistant' : (agentClient.productInfo?.id || 'ada-coding')
  const agentPersona = isPm
    ? '把用户的目标拆成可执行任务，委派给 coding agent 去落地，汇总产出与阻塞。不直接改代码，专注拆解、分派、跟进与汇总。'
    : (agentClient.productInfo?.persona || '在工作区内执行代码编写、架构规划与终端命令')
  const quickPrompts = isPm ? QUICK_PM_PROMPTS : QUICK_CODING_PROMPTS

  return (
    <div className="flex-1 flex flex-col items-center justify-center px-4 md:px-8 py-6 select-none overflow-y-auto min-h-0 bg-white dark:bg-[#18181b]">
      <div className="w-full max-w-2xl flex flex-col items-center space-y-5 -translate-y-6 transition-all duration-300">
        {/* 1. 标题与徽标 */}
        <div className="flex flex-col items-center text-center space-y-2">
          <div className={`w-12 h-12 rounded-2xl ${
            isPm
              ? 'bg-amber-500/10 dark:bg-amber-500/15 border-amber-500/20 text-amber-600 dark:text-amber-400'
              : 'bg-blue-500/10 dark:bg-blue-500/15 border-blue-500/20 text-blue-600 dark:text-blue-400'
          } border flex items-center justify-center shadow-sm`}>
            {isPm ? <ListTodo size={24} /> : <Sparkles size={24} />}
          </div>
          <div className="flex items-center space-x-2">
            <h1 className="text-xl md:text-2xl font-bold text-zinc-900 dark:text-zinc-100 tracking-tight">
              {agentName}
            </h1>
            <span className={`text-[10px] px-1.5 py-0.5 rounded font-mono font-medium ${
              isPm
                ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400 border-amber-500/20'
                : 'bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/20'
            } border`}>
              {agentId}
            </span>
          </div>
          <p className="text-xs text-zinc-500 dark:text-zinc-400 max-w-md">
            {agentPersona}
          </p>
        </div>

        {/* 2. 模式切换滑块 (CODING ↔ PM) */}
        <div className="relative flex p-0.5 rounded-xl bg-zinc-100 dark:bg-zinc-800/80 border border-zinc-200/80 dark:border-zinc-700/60 select-none shadow-xs w-64 max-w-xs">
          {/* 滑动背景药丸指示物 */}
          <div
            className={`absolute top-0.5 bottom-0.5 w-[calc(50%-2px)] rounded-lg transition-all duration-200 ease-out shadow-xs ${
              isPm
                ? 'left-[calc(50%+1px)] bg-amber-500 dark:bg-amber-600 text-white shadow-amber-500/20'
                : 'left-0.5 bg-blue-600 dark:bg-blue-600 text-white shadow-blue-600/20'
            }`}
          />

          {/* CODING 模式按钮 */}
          <button
            type="button"
            onClick={() => onSetMode('code')}
            className={`relative z-10 flex-1 flex items-center justify-center space-x-1.5 py-1.5 rounded-lg text-xs font-medium transition-colors cursor-pointer ${
              !isPm
                ? 'text-white font-semibold'
                : 'text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200'
            }`}
            title="切换至 CODING 模式：连接 ada-coding Agent (全功能敏捷编码、终端执行)"
          >
            <Code2 size={13} className={!isPm ? 'text-white' : 'text-blue-500'} />
            <span>CODING</span>
          </button>

          {/* PM 模式按钮 */}
          <button
            type="button"
            onClick={() => onSetMode('pm')}
            className={`relative z-10 flex-1 flex items-center justify-center space-x-1.5 py-1.5 rounded-lg text-xs font-medium transition-colors cursor-pointer ${
              isPm
                ? 'text-white font-semibold'
                : 'text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200'
            }`}
            title="切换至 PM 模式：连接 pm-assistant Agent (目标拆解、任务委派与跟进)"
          >
            <ListTodo size={13} className={isPm ? 'text-white' : 'text-amber-500'} />
            <span>PM</span>
          </button>
        </div>

        {/* 3. 选择工作区 */}
        <div className="w-full flex justify-center">
          <WorkspaceSelector
            currentWorkspace={currentWorkspace}
            allWorkspaces={allWorkspaces}
            onSelectWorkspace={onSelectWorkspace}
            onRemoveWorkspace={onRemoveWorkspace}
            onOpenPicker={onOpenWorkspacePicker}
          />
        </div>

        {/* 3. 居中会话输入框 */}
        <div className="w-full pt-1">
          <Composer
            thread={thread}
            mode={mode}
            running={running}
            providerConfig={providerConfig}
            approvalMode={approvalMode}
            effort={effort}
            centered={true}
            onSend={onSend}
            onAbort={onAbort}
            onSetMode={onSetMode}
            onSetApprovalMode={onSetApprovalMode}
            onSetEffort={onSetEffort}
            onOpenSettings={onOpenSettings}
            onOpenPlugins={onOpenPlugins}
            onOpenChanges={onOpenChanges}
            onOpenDebug={onOpenDebug}
            onNewThread={onNewThread}
            onCompact={onCompact}
          />
        </div>

        {/* 4. 专属任务推荐卡片 */}
        <div className="w-full grid grid-cols-1 sm:grid-cols-2 gap-2 pt-1">
          {quickPrompts.map((item, idx) => (
            <button
              key={idx}
              onClick={() => onSend(item.prompt)}
              className="flex items-start space-x-2.5 p-2.5 rounded-xl border border-zinc-200/70 dark:border-zinc-800 bg-zinc-50/50 dark:bg-zinc-900/40 hover:bg-zinc-100/80 dark:hover:bg-zinc-800/60 text-left transition-colors cursor-pointer group"
            >
              <div className={`w-7 h-7 rounded-lg ${
                isPm
                  ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
                  : 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
              } flex items-center justify-center flex-shrink-0 mt-0.5 group-hover:scale-105 transition-transform`}>
                <item.icon size={13} />
              </div>
              <div className="truncate min-w-0 flex-1">
                <div className="text-xs font-medium text-zinc-800 dark:text-zinc-200 truncate">
                  {item.title}
                </div>
                <div className="text-[10.5px] text-zinc-400 dark:text-zinc-500 truncate">
                  {item.prompt}
                </div>
              </div>
            </button>
          ))}
        </div>
      </div>
    </div>
  )
}
