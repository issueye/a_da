import React from 'react'
import { Sparkles } from 'lucide-react'
import type { AgentMode, ProviderConfig, ApprovalMode, Effort, Thread, QueuedItem } from '../types'
import { WorkspaceSelector } from './WorkspaceSelector'
import { Composer } from './Composer'

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
          <h1 className="text-xl md:text-2xl font-bold text-zinc-900 dark:text-zinc-100 tracking-tight">
            你好，我是 a_da 智能编码助手
          </h1>
          <p className="text-xs text-zinc-500 dark:text-zinc-400 max-w-md">
            在工作区内执行代码编写、架构规划与终端命令
          </p>
        </div>

        {/* 2. 选择工作区 */}
        <div className="w-full flex justify-center">
          <WorkspaceSelector
            currentWorkspace={currentWorkspace}
            allWorkspaces={allWorkspaces}
            onSelectWorkspace={onSelectWorkspace}
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
      </div>
    </div>
  )
}
