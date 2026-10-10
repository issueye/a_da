import React, { useState, useEffect } from 'react'
import { agentClient } from './client/ws-client'
import type { ClientSnapshot, AgentMode, ProviderConfig } from './types'
import { TitleBar } from './components/TitleBar'
import { Sidebar } from './components/Sidebar'
import { TabStrip } from './components/TabStrip'
import { Transcript } from './components/Transcript'
import { Composer } from './components/Composer'
import { EmptyConversationView } from './components/EmptyConversationView'
import { SettingsModal } from './components/SettingsModal'
import { PluginsModal } from './components/PluginsModal'
import { ChangesPanel } from './components/ChangesPanel'
import { CommandPalette } from './components/CommandPalette'
import { FilePicker } from './components/FilePicker'
import { DebugPanel } from './components/DebugPanel'
import { ToastHost, notify } from './components/ToastHost'
import { ConfirmModal } from './components/ConfirmModal'
import { ProcessModal } from './components/ProcessModal'
import { deriveActiveChangeCount } from './utils/derive-changes'

export function App() {
  const [snapshot, setSnapshot] = useState<ClientSnapshot>(agentClient.snapshot)
  const [connected, setConnected] = useState(agentClient.connected)
  const [isDark, setIsDark] = useState<boolean>(() => {
    const saved = localStorage.getItem('a_da_theme')
    return saved !== null ? saved === 'dark' : true
  })
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [pluginsOpen, setPluginsOpen] = useState(false)
  const [processOpen, setProcessOpen] = useState(false)
  const [changesOpen, setChangesOpen] = useState(false)
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false)
  const [debugOpen, setDebugOpen] = useState(false)
  const [filePickerOpen, setFilePickerOpen] = useState(false)
  const [filePickerMode, setFilePickerMode] = useState<'directory' | 'files'>('directory')
  const [openTabIds, setOpenTabIds] = useState<string[]>([])
  const [confirmModal, setConfirmModal] = useState<{
    isOpen: boolean
    title?: string
    message: string
    subMessage?: string
    confirmText?: string
    cancelText?: string
    variant?: 'danger' | 'warning' | 'primary'
    onConfirm: (val?: string) => void | Promise<void>
  }>({
    isOpen: false,
    message: '',
    onConfirm: () => {},
  })

  // 同步激活会话至打开的标签页列表中
  useEffect(() => {
    if (snapshot.activeThreadId) {
      setOpenTabIds((prev) => {
        if (!prev.includes(snapshot.activeThreadId)) {
          return [...prev, snapshot.activeThreadId]
        }
        return prev
      })
    }
  }, [snapshot.activeThreadId])

  // 同步暗亮主题至 HTML 根节点与 LocalStorage
  useEffect(() => {
    const root = document.documentElement
    if (isDark) {
      root.classList.add('dark')
      root.classList.remove('light')
      localStorage.setItem('a_da_theme', 'dark')
    } else {
      root.classList.remove('dark')
      root.classList.add('light')
      localStorage.setItem('a_da_theme', 'light')
    }
  }, [isDark])

  useEffect(() => {
    const unsub = agentClient.subscribe((snap) => {
      setSnapshot({ ...snap })
      setConnected(agentClient.connected)
    })
    return unsub
  }, [])

  // 如果是在 Tauri 容器中，尝试从窗口环境获取本地动态端口
  useEffect(() => {
    // 监听 URL 参数或环境变量中的 ws 端口
    const params = new URLSearchParams(window.location.search)
    const port = params.get('port')
    const token = params.get('token')
    if (port) {
      const url = `ws://127.0.0.1:${port}/rpc`
      agentClient.setConnection(url, token || '')
    }
  }, [])

  const activeThread =
    snapshot.threads.find((t) => t.id === snapshot.activeThreadId) ||
    snapshot.threads[0]

  const handleSelectThread = (threadId: string) => {
    setOpenTabIds((prev) => (prev.includes(threadId) ? prev : [...prev, threadId]))
    agentClient.setActiveThread(threadId)
    const target = snapshot.threads.find((t) => t.id === threadId)
    if (target?.workspace) {
      agentClient.setActiveProject(target.workspace)
    }
  }

  const handleCreateThread = (workspace?: string, mode?: AgentMode) => {
    if (workspace) {
      agentClient.setActiveProject(workspace)
    }
    agentClient.createThread(workspace, mode)
  }

  const handleDeleteThread = (threadId: string) => {
    setOpenTabIds((prev) => prev.filter((id) => id !== threadId))
    agentClient.deleteThread(threadId)
  }

  const handleCloseTab = (threadId: string) => {
    setOpenTabIds((prev) => {
      const next = prev.filter((id) => id !== threadId)
      if (threadId === snapshot.activeThreadId && next.length > 0) {
        agentClient.setActiveThread(next[next.length - 1])
      }
      return next
    })
  }

  const handleSend = (text: string, images?: string[]) => {
    agentClient.sendPrompt(text, images)
  }

  const handleAbort = (targetThreadId?: unknown) => {
    const tid = typeof targetThreadId === 'string' && targetThreadId.trim().length > 0
      ? targetThreadId.trim()
      : activeThread?.id
    if (tid) {
      agentClient.abortCurrent(tid)
    }
  }

  const handleRetry = (targetThreadId?: string) => {
    const tid = targetThreadId || activeThread?.id
    if (tid) {
      agentClient.retry(tid)
    }
  }

  const handleSetMode = (mode: AgentMode) => {
    agentClient.setMode(mode)
  }

  const handleSaveSettings = (config: ProviderConfig) => {
    agentClient.setProvider(config)
  }

  const handleAnswerQuestion = (callId: string, choice?: string, text?: string) => {
    agentClient.answerQuestion(callId, choice, text)
  }

  const handleDecideApproval = (toolItemId: string, approved: boolean) => {
    agentClient.decideApproval(toolItemId, approved)
  }

  const handleCompact = () => {
    agentClient.compactThread(activeThread?.id)
  }

  // 统计当前会话未撤销的文件改动数量
  const activeChangeCount = deriveActiveChangeCount(activeThread?.items || [])

  // 聚合所有已知的工作区工程路径
  const allWorkspaces = React.useMemo(() => {
    const set = new Set<string>()
    if (snapshot.activeWorkspace) set.add(snapshot.activeWorkspace)
    for (const t of snapshot.threads) {
      if (t.workspace) set.add(t.workspace)
    }
    return Array.from(set)
  }, [snapshot.threads, snapshot.activeWorkspace])

  // 切换已有工作区
  const handleSelectWorkspace = (targetWs: string) => {
    if (!targetWs) return
    agentClient.setActiveProject(targetWs)
    if (activeThread && activeThread.items.length === 0) {
      agentClient.deleteThread(activeThread.id)
    }
    agentClient.createThread(targetWs)
    notify({ message: `已切换工作区: ${targetWs}`, level: 'success' })
  }

  // 选择新工作区目录
  const handlePickedWorkspace = (paths: string[]) => {
    if (paths.length > 0) {
      const targetWs = paths[0]
      agentClient.setActiveProject(targetWs)
      if (activeThread && activeThread.items.length === 0) {
        agentClient.deleteThread(activeThread.id)
      }
      agentClient.createThread(targetWs)
      notify({ message: `已切换工作区: ${targetWs}`, level: 'success' })
    }
  }

  // 移除指定工作区
  const handleRemoveWorkspace = async (targetWs: string) => {
    if (!targetWs) return
    if (allWorkspaces.length <= 1) {
      notify({ message: '至少保留一个工作区', level: 'warn' })
      return
    }
    const runningInWs = snapshot.threads.some(
      (t) => t.workspace === targetWs && (snapshot.runningThreadIds || []).includes(t.id)
    )
    if (runningInWs) {
      notify({ message: '该工作区内有会话正在运行，先停止再移除', level: 'warn' })
      return
    }
    const dirName = targetWs.split(/[\\/]/).filter(Boolean).pop() || targetWs
    setConfirmModal({
      isOpen: true,
      title: '移除工作区',
      message: `确定要从列表中移除工作区「${dirName}」吗？`,
      subMessage: '该工作区下的所有会话记录将被清除，但本地实际代码文件不会被删除。',
      confirmText: '确定移除',
      cancelText: '取消',
      variant: 'danger',
      onConfirm: async () => {
        try {
          await agentClient.removeWorkspace(targetWs)
          // 若当前打开的标签页属于被移除的工作区，从 openTabIds 中清理
          setOpenTabIds((prev) => {
            const remainingIds = new Set(snapshot.threads.filter((t) => t.workspace !== targetWs).map((t) => t.id))
            return prev.filter((id) => remainingIds.has(id))
          })
          notify({ message: `已移除工作区: ${dirName}`, level: 'success' })
        } catch (err: any) {
          notify({ message: err?.message || '移除工作区失败', level: 'error' })
        } finally {
          setConfirmModal((prev) => ({ ...prev, isOpen: false }))
        }
      },
    })
  }

  // 全局键盘快捷键监听
  useEffect(() => {
    const handleGlobalKeyDown = (e: KeyboardEvent) => {
      // Ctrl+K 或 Ctrl+Shift+P 唤起命令面板
      if (
        (e.ctrlKey || e.metaKey) &&
        (e.key === 'k' || e.key === 'K' || (e.shiftKey && (e.key === 'p' || e.key === 'P')))
      ) {
        e.preventDefault()
        setCommandPaletteOpen((prev) => !prev)
        return
      }

      // Ctrl+N 新建会话
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && (e.key === 'n' || e.key === 'N')) {
        e.preventDefault()
        handleCreateThread(activeThread?.workspace || snapshot.activeWorkspace)
        return
      }

      // Ctrl+W 关闭当前标签页
      if ((e.ctrlKey || e.metaKey) && (e.key === 'w' || e.key === 'W')) {
        e.preventDefault()
        if (activeThread?.id) {
          handleCloseTab(activeThread.id)
        }
        return
      }

      // Ctrl+, 打开设置
      if ((e.ctrlKey || e.metaKey) && e.key === ',') {
        e.preventDefault()
        setSettingsOpen(true)
        return
      }

      // Ctrl+Shift+X 打开插件中心
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && (e.key === 'x' || e.key === 'X')) {
        e.preventDefault()
        setPluginsOpen((prev) => !prev)
        return
      }

      // Ctrl+Shift+C 打开改动审查
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && (e.key === 'c' || e.key === 'C')) {
        e.preventDefault()
        setChangesOpen((prev) => !prev)
        return
      }

      // Ctrl+Shift+D 打开调试面板
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && (e.key === 'd' || e.key === 'D')) {
        e.preventDefault()
        setDebugOpen((prev) => !prev)
        return
      }

      // Esc 关闭模态层；若无模态层且当前会话正在运行，则中止当前执行
      if (e.key === 'Escape') {
        if (commandPaletteOpen) setCommandPaletteOpen(false)
        else if (changesOpen) setChangesOpen(false)
        else if (debugOpen) setDebugOpen(false)
        else if (filePickerOpen) setFilePickerOpen(false)
        else if (settingsOpen) setSettingsOpen(false)
        else if (pluginsOpen) setPluginsOpen(false)
        else if (activeThread?.id && (snapshot.runningThreadIds || []).includes(activeThread.id)) {
          handleAbort(activeThread.id)
        }
      }
    }

    window.addEventListener('keydown', handleGlobalKeyDown)
    return () => window.removeEventListener('keydown', handleGlobalKeyDown)
  }, [
    commandPaletteOpen,
    changesOpen,
    debugOpen,
    filePickerOpen,
    settingsOpen,
    pluginsOpen,
    activeThread,
    snapshot.activeWorkspace,
  ])

  return (
    <div
      className={`w-full h-full flex flex-col overflow-hidden transition-colors duration-150 ${
        isDark ? 'dark bg-[#18181b] text-zinc-100' : 'light bg-zinc-50 text-zinc-900'
      }`}
    >
      {/* 顶部标题栏（包含 Tauri 窗口控制与状态） */}
      <TitleBar
        title={activeThread?.title || '新对话'}
        workspace={activeThread?.workspace}
        productName={
          activeThread?.mode === 'pm' || activeThread?.agentId === 'ada-pm' || activeThread?.agentId === 'pm-assistant'
            ? 'ada-pm'
            : (agentClient.productInfo?.name || agentClient.productInfo?.id || 'ada-coding')
        }
        mode={snapshot.currentMode}
        connected={connected}
        isDark={isDark}
        onReconnect={() => agentClient.reconnectImmediately()}
        onToggleTheme={() => setIsDark(!isDark)}
        onOpenSettings={() => setSettingsOpen(true)}
        onOpenPlugins={() => setPluginsOpen(true)}
        onOpenProcesses={() => setProcessOpen(true)}
        onOpenCommandPalette={() => setCommandPaletteOpen(true)}
        onOpenChanges={() => setChangesOpen(true)}
        activeChangeCount={activeChangeCount}
      />

      {/* 服务离线/正在重连提示横幅 */}
      {!connected && (
        <div className="bg-amber-500/10 dark:bg-amber-500/20 border-b border-amber-500/30 px-3 py-1 flex items-center justify-between text-xs text-amber-700 dark:text-amber-300 z-10">
          <div className="flex items-center space-x-2">
            <span className="relative flex h-2 w-2">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-amber-400 opacity-75" />
              <span className="relative inline-flex rounded-full h-2 w-2 bg-amber-500" />
            </span>
            <span>与核心服务连接断开，正在尝试重连...</span>
          </div>
          <button
            onClick={() => agentClient.reconnectImmediately()}
            className="px-2 py-0.5 rounded bg-amber-500/20 hover:bg-amber-500/30 text-amber-800 dark:text-amber-200 border border-amber-500/40 text-[11px] font-medium transition-colors cursor-pointer"
          >
            立即重连
          </button>
        </div>
      )}

      {/* 主体工作区（左右分栏） */}
      <div className="flex-1 flex overflow-hidden">
        {/* 左侧会话与工作区侧边栏 */}
        <Sidebar
          threads={snapshot.threads}
          activeThreadId={activeThread?.id || ''}
          activeWorkspace={activeThread?.workspace || snapshot.activeWorkspace}
          currentMode={snapshot.currentMode}
          runningThreadIds={snapshot.runningThreadIds || []}
          onSelectThread={handleSelectThread}
          onCreateThread={handleCreateThread}
          onSelectMode={handleSetMode}
          onDeleteThread={handleDeleteThread}
          onRemoveWorkspace={handleRemoveWorkspace}
          onOpenWorkspacePicker={() => {
            setFilePickerMode('directory')
            setFilePickerOpen(true)
          }}
        />

        {/* 右侧主聊天与输入区域 */}
        <main className="flex-1 flex flex-col h-full bg-white dark:bg-[#18181b] relative overflow-hidden transition-colors duration-150">
          {/* 顶部会话多标签页栏 */}
          <TabStrip
            threads={snapshot.threads}
            openTabIds={openTabIds}
            activeThreadId={activeThread?.id || ''}
            runningThreadIds={snapshot.runningThreadIds || []}
            isRunning={Boolean(activeThread?.id && (snapshot.runningThreadIds || []).includes(activeThread.id))}
            onSelectTab={handleSelectThread}
            onCloseTab={handleCloseTab}
            onNewTab={() => handleCreateThread(activeThread?.workspace || snapshot.activeWorkspace)}
          />

          {/* 新会话界面上下垂直居中；有消息时无缝恢复常规消息流与底部输入框 */}
          {!activeThread || activeThread.items.length === 0 ? (
            <EmptyConversationView
              thread={activeThread}
              mode={snapshot.currentMode}
              running={Boolean(activeThread?.id && (snapshot.runningThreadIds || []).includes(activeThread.id))}
              providerConfig={snapshot.providerConfig}
              approvalMode={snapshot.approvalMode}
              effort={snapshot.effort}
              currentWorkspace={activeThread?.workspace || snapshot.activeWorkspace}
              allWorkspaces={allWorkspaces}
              onSelectWorkspace={handleSelectWorkspace}
              onRemoveWorkspace={handleRemoveWorkspace}
              onOpenWorkspacePicker={() => {
                setFilePickerMode('directory')
                setFilePickerOpen(true)
              }}
              onSend={handleSend}
              onAbort={handleAbort}
              onSetMode={handleSetMode}
              onSetApprovalMode={(mode) => agentClient.setApprovalMode(mode)}
              onSetEffort={(effort) => agentClient.setEffort(effort)}
              onOpenSettings={() => setSettingsOpen(true)}
              onOpenPlugins={() => setPluginsOpen(true)}
              onOpenChanges={() => setChangesOpen(true)}
              onOpenDebug={() => setDebugOpen(true)}
              onNewThread={() => handleCreateThread(activeThread?.workspace || snapshot.activeWorkspace)}
              onCompact={handleCompact}
            />
          ) : (
            <>
              {/* 对话消息流（含深度思考折叠、Markdown、工具卡片等，key/threadId 确保独立容器与滚动隔离） */}
              <Transcript
                key={activeThread.id}
                threadId={activeThread.id}
                items={activeThread.items}
                running={Boolean(activeThread?.id && (snapshot.runningThreadIds || []).includes(activeThread.id))}
                activeWorkspace={activeThread.workspace || snapshot.activeWorkspace}
                currentMode={snapshot.currentMode}
                onAnswerQuestion={handleAnswerQuestion}
                onDecideApproval={handleDecideApproval}
                onRetry={() => handleRetry(activeThread.id)}
                onSelectThread={handleSelectThread}
              />

              {/* 底部输入框与加号菜单、模式选择、排队队列与遥测底栏 */}
              <Composer
                thread={activeThread}
                mode={snapshot.currentMode}
                running={Boolean(activeThread?.id && (snapshot.runningThreadIds || []).includes(activeThread.id))}
                providerConfig={snapshot.providerConfig}
                approvalMode={snapshot.approvalMode}
                effort={snapshot.effort}
                queue={(snapshot.queue || []).filter((q) => !q.threadId || q.threadId === activeThread?.id)}
                onSend={handleSend}
                onAbort={handleAbort}
                onSetMode={handleSetMode}
                onSetApprovalMode={(mode) => agentClient.setApprovalMode(mode)}
                onSetEffort={(effort) => agentClient.setEffort(effort)}
                onOpenSettings={() => setSettingsOpen(true)}
                onOpenPlugins={() => setPluginsOpen(true)}
                onOpenChanges={() => setChangesOpen(true)}
                onOpenDebug={() => setDebugOpen(true)}
                onNewThread={() => handleCreateThread(activeThread.workspace || snapshot.activeWorkspace)}
                activeChangeCount={activeChangeCount}
                onAnswerQuestion={handleAnswerQuestion}
                onPromoteQueueItem={(idx) => agentClient.promoteQueueItem(idx, activeThread?.id)}
                onRemoveQueueItem={(idx) => agentClient.removeFromQueue(idx, activeThread?.id)}
                onClearQueue={() => agentClient.clearQueue(activeThread?.id)}
                onSwitchThread={(threadId) => agentClient.setActiveThread(threadId)}
                onCompact={handleCompact}
              />
            </>
          )}
        </main>
      </div>

      {/* 改动审查面板 */}
      <ChangesPanel
        thread={activeThread}
        isOpen={changesOpen}
        onClose={() => setChangesOpen(false)}
      />

      {/* 命令面板 (Ctrl+K) */}
      <CommandPalette
        isOpen={commandPaletteOpen}
        onClose={() => setCommandPaletteOpen(false)}
        currentMode={snapshot.currentMode}
        onSelectMode={handleSetMode}
        onNewThread={() => handleCreateThread(activeThread?.workspace || snapshot.activeWorkspace)}
        onOpenSettings={() => setSettingsOpen(true)}
        onOpenPlugins={() => setPluginsOpen(true)}
        onOpenChanges={() => setChangesOpen(true)}
        onOpenDebug={() => setDebugOpen(true)}
        onOpenFilePicker={() => {
          setFilePickerMode('directory')
          setFilePickerOpen(true)
        }}
        onCompact={handleCompact}
        onToggleTheme={() => setIsDark(!isDark)}
        isDark={isDark}
      />

      {/* 通信与底层调试面板 */}
      <DebugPanel
        isOpen={debugOpen}
        onClose={() => setDebugOpen(false)}
      />

      {/* 全平台文件/目录选择器 */}
      <FilePicker
        isOpen={filePickerOpen}
        mode={filePickerMode}
        startPath={activeThread?.workspace || snapshot.activeWorkspace}
        onPicked={handlePickedWorkspace}
        onClose={() => setFilePickerOpen(false)}
      />

      {/* 设置弹窗 */}
      <SettingsModal
        isOpen={settingsOpen}
        config={snapshot.providerConfig}
        approvalMode={snapshot.approvalMode}
        effort={snapshot.effort}
        onClose={() => setSettingsOpen(false)}
        onSave={handleSaveSettings}
        onOpenProcesses={() => setProcessOpen(true)}
      />

      {/* 插件中心弹窗 */}
      <PluginsModal
        isOpen={pluginsOpen}
        onClose={() => setPluginsOpen(false)}
      />

      {/* 进程管理与服务监控弹窗 */}
      <ProcessModal
        isOpen={processOpen}
        onClose={() => setProcessOpen(false)}
      />

      {/* 统一操作确认弹窗 (无原生 window.confirm) */}
      <ConfirmModal
        isOpen={confirmModal.isOpen}
        title={confirmModal.title}
        message={confirmModal.message}
        subMessage={confirmModal.subMessage}
        confirmText={confirmModal.confirmText}
        cancelText={confirmModal.cancelText}
        variant={confirmModal.variant}
        onConfirm={confirmModal.onConfirm}
        onCancel={() => setConfirmModal((prev) => ({ ...prev, isOpen: false }))}
      />

      {/* 全局轻提示 Toast 宿主 */}
      <ToastHost />
    </div>
  )
}
