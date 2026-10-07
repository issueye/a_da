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
  const [changesOpen, setChangesOpen] = useState(false)
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false)
  const [debugOpen, setDebugOpen] = useState(false)
  const [filePickerOpen, setFilePickerOpen] = useState(false)
  const [filePickerMode, setFilePickerMode] = useState<'directory' | 'files'>('directory')
  const [openTabIds, setOpenTabIds] = useState<string[]>([])

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
  }

  const handleCreateThread = (workspace?: string) => {
    agentClient.createThread(workspace)
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

  const handleAbort = (targetThreadId?: string) => {
    agentClient.abortCurrent(targetThreadId || activeThread?.id)
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

      // Esc 关闭模态层
      if (e.key === 'Escape') {
        if (commandPaletteOpen) setCommandPaletteOpen(false)
        else if (changesOpen) setChangesOpen(false)
        else if (debugOpen) setDebugOpen(false)
        else if (filePickerOpen) setFilePickerOpen(false)
        else if (settingsOpen) setSettingsOpen(false)
        else if (pluginsOpen) setPluginsOpen(false)
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
        mode={snapshot.currentMode}
        connected={connected}
        isDark={isDark}
        onToggleTheme={() => setIsDark(!isDark)}
        onOpenSettings={() => setSettingsOpen(true)}
        onOpenPlugins={() => setPluginsOpen(true)}
        onOpenCommandPalette={() => setCommandPaletteOpen(true)}
        onOpenChanges={() => setChangesOpen(true)}
        activeChangeCount={activeChangeCount}
      />

      {/* 主体工作区（左右分栏） */}
      <div className="flex-1 flex overflow-hidden">
        {/* 左侧会话与工作区侧边栏 */}
        <Sidebar
          threads={snapshot.threads}
          activeThreadId={activeThread?.id || ''}
          activeWorkspace={activeThread?.workspace || snapshot.activeWorkspace}
          runningThreadIds={snapshot.runningThreadIds || []}
          onSelectThread={handleSelectThread}
          onCreateThread={handleCreateThread}
          onDeleteThread={handleDeleteThread}
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
              {/* 对话消息流（含深度思考折叠、Markdown、工具卡片等） */}
              <Transcript
                items={activeThread.items}
                running={Boolean(activeThread?.id && (snapshot.runningThreadIds || []).includes(activeThread.id))}
                activeWorkspace={activeThread.workspace || snapshot.activeWorkspace}
                currentMode={snapshot.currentMode}
                onAnswerQuestion={handleAnswerQuestion}
                onDecideApproval={handleDecideApproval}
              />

              {/* 底部输入框与加号菜单、模式选择、排队队列与遥测底栏 */}
              <Composer
                thread={activeThread}
                mode={snapshot.currentMode}
                running={Boolean(activeThread?.id && (snapshot.runningThreadIds || []).includes(activeThread.id))}
                providerConfig={snapshot.providerConfig}
                approvalMode={snapshot.approvalMode}
                effort={snapshot.effort}
                queue={snapshot.queue}
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
                onPromoteQueueItem={(idx) => agentClient.promoteQueueItem(idx)}
                onRemoveQueueItem={(idx) => agentClient.removeFromQueue(idx)}
                onClearQueue={() => agentClient.clearQueue()}
                onResumeSubagent={(subagentThreadId) => agentClient.resumeSubagent(subagentThreadId)}
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
      />

      {/* 插件中心弹窗 */}
      <PluginsModal
        isOpen={pluginsOpen}
        onClose={() => setPluginsOpen(false)}
      />

      {/* 全局轻提示 Toast 宿主 */}
      <ToastHost />
    </div>
  )
}
