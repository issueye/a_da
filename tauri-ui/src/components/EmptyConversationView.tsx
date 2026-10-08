import React from 'react'
import { Sparkles, Terminal, ShieldAlert, CheckCircle2, Search } from 'lucide-react'
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
  return (
    <div className="flex-1 flex flex-col items-center justify-center px-4 md:px-8 py-6 select-none overflow-y-auto min-h-0 bg-white dark:bg-[#18181b]">
      <div className="w-full max-w-2xl flex flex-col items-center space-y-5 -translate-y-6 transition-all duration-300">
        {/* 1. 标题与徽标 */}
        <div className="flex flex-col items-center text-center space-y-2">
          <div className="w-12 h-12 rounded-2xl bg-blue-500/10 dark:bg-blue-500/15 border border-blue-500/20 flex items-center justify-center text-blue-600 dark:text-blue-400 shadow-sm">
            <Sparkles size={24} />
          </div>
          <div className="flex items-center space-x-2">
            <h1 className="text-xl md:text-2xl font-bold text-zinc-900 dark:text-zinc-100 tracking-tight">
              {agentClient.productInfo?.name || 'a_da 智能编码助手'}
            </h1>
            <span className="text-[10px] px-1.5 py-0.5 rounded font-mono font-medium bg-blue-500/10 text-blue-600 dark:text-blue-400 border border-blue-500/20">
              {agentClient.productInfo?.id || 'ada-coding'}
            </span>
          </div>
          <p className="text-xs text-zinc-500 dark:text-zinc-400 max-w-md">
            {agentClient.productInfo?.persona || '在工作区内执行代码编写、架构规划与终端命令'}
          </p>
        </div>

        {/* 2. 选择工作区 */}
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

        {/* 4. 专属编码任务推荐卡片 */}
        <div className="w-full grid grid-cols-1 sm:grid-cols-2 gap-2 pt-1">
          {QUICK_CODING_PROMPTS.map((item, idx) => (
            <button
              key={idx}
              onClick={() => onSend(item.prompt)}
              className="flex items-start space-x-2.5 p-2.5 rounded-xl border border-zinc-200/70 dark:border-zinc-800 bg-zinc-50/50 dark:bg-zinc-900/40 hover:bg-zinc-100/80 dark:hover:bg-zinc-800/60 text-left transition-colors cursor-pointer group"
            >
              <div className="w-7 h-7 rounded-lg bg-blue-500/10 text-blue-600 dark:text-blue-400 flex items-center justify-center flex-shrink-0 mt-0.5 group-hover:scale-105 transition-transform">
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
