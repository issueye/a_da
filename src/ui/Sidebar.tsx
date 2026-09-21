/**
 * 工作区与会话。
 *
 * One project row shows the workspace the agent is pinned to, and the session list
 * is the conversation history. Both are read from the store, which re-renders
 * this column whenever a turn changes something. 添加项目 opens the native
 * directory picker; a session is deleted from the trash icon at the end of its row.
 */

import React, { useState } from 'react'
import { Icon, IconButton } from './controls'
import { C, editorTheme, M, shortPath } from '../theme'
import type { AgentStore } from '../agent/store'
import type { Thread } from '../agent/types'

function SectionHeader({
  label,
  open,
  onToggle,
  count,
  action,
}: {
  label: string
  open?: boolean
  onToggle?: () => void
  count?: string
  action?: React.ReactNode
}) {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        gap: 4,
        height: 24,
        paddingLeft: 4,
        paddingRight: 4,
        borderRadius: 6,
        flexShrink: 0,
      }}
    >
      <div
        role="button"
        aria-label={open ? `折叠${label}` : `展开${label}`}
        onClick={onToggle}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 6,
          flexGrow: 1,
          height: '100%',
          paddingLeft: 4,
          paddingRight: 4,
          borderRadius: 4,
          cursor: onToggle ? 'pointer' : undefined,
          hover: onToggle ? { backgroundColor: C.overlay } : undefined,
        }}
      >
        <text style={{ fontSize: 11.5, lineHeight: 15, fontWeight: 600, color: C.secondary }}>
          {label}
        </text>
        {count ? <text style={{ fontSize: 11, lineHeight: 15, color: C.faint }}>{count}</text> : null}
        <div style={{ flexGrow: 1 }} />
        {onToggle ? <Icon name={open ? 'chevronDown' : 'chevronRight'} size={11} color={C.faint} /> : null}
      </div>
      {action}
    </div>
  )
}

function Row({
  icon,
  label,
  sub,
  selected,
  onClick,
  testId,
  tone = 'normal',
}: {
  icon: 'folder' | 'file' | 'thread' | 'plus'
  label: string
  sub?: string
  selected?: boolean
  onClick?: () => void
  testId?: string
  tone?: 'normal' | 'muted'
}) {
  return (
    <div
      testId={testId}
      role="button"
      aria-label={label}
      onClick={onClick}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        gap: 7,
        height: M.row,
        paddingLeft: 6,
        paddingRight: 6,
        borderRadius: 6,
        flexShrink: 0,
        cursor: 'pointer',
        backgroundColor: selected ? C.tab : '#00000000',
        hover: { backgroundColor: selected ? C.tab : C.overlay },
      }}
    >
      <Icon name={icon} size={13} color={tone === 'muted' ? C.faint : C.secondary} />
      <text
        style={{
          fontSize: 12.5,
          lineHeight: 16,
          color: tone === 'muted' ? C.tertiary : C.text,
          whiteSpace: 'nowrap',
          textOverflow: 'ellipsis',
          flexShrink: 1,
        }}
      >
        {label}
      </text>
      <div style={{ flexGrow: 1 }} />
      {sub ? <text style={{ fontSize: 11, lineHeight: 15, color: C.faint }}>{sub}</text> : null}
    </div>
  )
}

/**
 * 一个会话行：左边整片是「打开这个会话」，右边的垃圾桶图标删掉它。
 *
 * 选择区和图标是**兄弟**而不是嵌套：click 会往上冒泡，如果图标在带 onClick 的
 * 元素里面，点图标就会顺带把会话切过去再删掉。
 *
 * 删除要点两下。第一下只是把垃圾桶变成红色的「确认删除」，鼠标移出这一行就复位；
 * 会话记录是这一轮的唯一副本，删掉就没了（盘上那份 JSONL 也一起删）。
 */
function SessionRow({
  thread,
  store,
  selected,
  running,
  onNotice,
}: {
  thread: Thread
  store: AgentStore
  selected: boolean
  running: boolean
  onNotice: (message: string | null) => void
}) {
  const [armed, setArmed] = useState(false)

  const remove = (): void => {
    if (!armed) {
      setArmed(true)
      return
    }
    onNotice(store.deleteThread(thread.id))
    setArmed(false)
  }

  return (
    <div
      testId={`thread-${thread.id}`}
      onMouseLeave={() => setArmed(false)}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        height: M.row,
        borderRadius: 6,
        flexShrink: 0,
        backgroundColor: selected ? C.tab : '#00000000',
        hover: { backgroundColor: selected ? C.tab : C.overlay },
      }}
    >
      <div
        role="button"
        aria-label={thread.title}
        onClick={() => store.selectThread(thread.id)}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 7,
          height: '100%',
          flexGrow: 1,
          minWidth: 0,
          paddingLeft: 6,
          cursor: 'pointer',
        }}
      >
        <Icon name="thread" size={13} color={C.secondary} />
        <text
          style={{
            fontSize: 12.5,
            lineHeight: 16,
            color: C.text,
            whiteSpace: 'nowrap',
            textOverflow: 'ellipsis',
            flexShrink: 1,
          }}
        >
          {thread.title}
        </text>
        <div style={{ flexGrow: 1 }} />
        {running ? (
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 4,
              paddingLeft: 5,
              paddingRight: 5,
              height: 16,
              borderRadius: 4,
              backgroundColor: C.chip,
              marginRight: 4,
              flexShrink: 0,
            }}
          >
            <Icon name="dot" size={6} color={C.success} />
            <text style={{ fontSize: 10, lineHeight: 14, color: C.faint }}>运行中</text>
          </div>
        ) : null}
      </div>

      <div
        testId={`delete-thread-${thread.id}`}
        role="button"
        aria-label={armed ? '确认删除会话' : '删除会话'}
        onClick={remove}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'flex-end',
          gap: 4,
          height: '100%',
          flexShrink: 0,
          paddingLeft: 5,
          paddingRight: 7,
          cursor: 'pointer',
        }}
      >
        {armed ? (
          <text style={{ fontSize: 11, lineHeight: 15, color: C.accent }}>确认删除</text>
        ) : null}
        <Icon name="trash" size={12} color={armed ? C.accent : C.faint} />
      </div>
    </div>
  )
}

function WorkspaceTreeNode({
  workspacePath,
  store,
  query,
  expanded,
  onToggleExpand,
  onNotice,
}: {
  workspacePath: string
  store: AgentStore
  query: string
  expanded: boolean
  onToggleExpand: () => void
  onNotice: (message: string | null) => void
}) {
  const [armedRemove, setArmedRemove] = useState(false)
  const isCurrent = workspacePath === store.project
  const allWorkspaceThreads = store.threads.filter((t) => t.workspace === workspacePath)
  const matchingThreads = allWorkspaceThreads.filter((t) =>
    query.trim() ? t.title.toLowerCase().includes(query.trim().toLowerCase()) : true,
  )

  const label = shortPath(workspacePath, 2)

  return (
    <div style={{ display: 'flex', flexDirection: 'column', width: '100%', flexShrink: 0 }}>
      {/* 工作区行（树根节点） */}
      <div
        testId={`project-${label}`}
        onMouseLeave={() => setArmedRemove(false)}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 6,
          height: M.row,
          paddingLeft: 6,
          paddingRight: 6,
          borderRadius: 6,
          flexShrink: 0,
          backgroundColor: isCurrent ? C.tab : '#00000000',
          hover: { backgroundColor: isCurrent ? C.tab : C.overlay },
        }}
      >
        {/* 折叠/展开三角箭头 */}
        <div
          role="button"
          aria-label={expanded ? `折叠工作区 ${label}` : `展开工作区 ${label}`}
          onClick={onToggleExpand}
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            width: 14,
            height: 14,
            borderRadius: 3,
            cursor: 'pointer',
            hover: { backgroundColor: C.chipHover },
          }}
        >
          <Icon name={expanded ? 'chevronDown' : 'chevronRight'} size={11} color={C.faint} />
        </div>

        {/* 文件夹图标与名称（主体点击切换工作区） */}
        <div
          role="button"
          aria-label={label}
          onClick={() => {
            store.selectProject(workspacePath)
            if (!expanded) onToggleExpand()
          }}
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 6,
            height: '100%',
            flexGrow: 1,
            minWidth: 0,
            cursor: 'pointer',
          }}
        >
          <Icon name="folder" size={13} color={isCurrent ? C.link : C.secondary} />
          <text
            style={{
              fontSize: 12.5,
              lineHeight: 16,
              fontWeight: isCurrent ? 600 : 400,
              color: isCurrent ? C.text : C.secondary,
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis',
              flexShrink: 1,
            }}
          >
            {label}
          </text>
          <div style={{ flexGrow: 1 }} />
          {/* 会话数指示微章 */}
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              minWidth: 16,
              height: 16,
              paddingLeft: 4,
              paddingRight: 4,
              borderRadius: 8,
              backgroundColor: C.chip,
              marginRight: 4,
            }}
          >
            <text style={{ fontSize: 10, lineHeight: 14, color: C.faint }}>
              {`${allWorkspaceThreads.length}`}
            </text>
          </div>
        </div>

        {/* 右侧操作按钮组：新建会话与移除工作区 */}
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 2, flexShrink: 0 }}>
          {/* 快捷新建会话按钮 (+) */}
          <div
            role="button"
            aria-label={`在 ${label} 中新建会话`}
            onClick={() => {
              store.selectProject(workspacePath)
              store.newThread(workspacePath)
            }}
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              width: 18,
              height: 18,
              borderRadius: 4,
              cursor: 'pointer',
              hover: { backgroundColor: C.chipHover },
            }}
          >
            <Icon name="plus" size={11} color={C.tertiary} />
          </div>

          {/* 移除工作区按钮 (垃圾桶) */}
          <div
            testId={`remove-project-${label}`}
            role="button"
            aria-label={armedRemove ? `确认移除工作区 ${label}` : `移除工作区 ${label}`}
            onClick={() => {
              if (!armedRemove) {
                setArmedRemove(true)
                return
              }
              const err = store.removeProject(workspacePath)
              if (err) onNotice(err)
              setArmedRemove(false)
            }}
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              justifyContent: 'center',
              gap: 3,
              height: 18,
              paddingLeft: armedRemove ? 5 : 3,
              paddingRight: armedRemove ? 5 : 3,
              borderRadius: 4,
              cursor: 'pointer',
              hover: { backgroundColor: C.chipHover },
            }}
          >
            {armedRemove ? (
              <text style={{ fontSize: 10.5, lineHeight: 15, color: C.accent }}>确认移除</text>
            ) : null}
            <Icon name="trash" size={11} color={armedRemove ? C.accent : C.faint} />
          </div>
        </div>
      </div>

      {/* 展开的会话子树列表 */}
      {expanded ? (
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            paddingLeft: 12,
            borderLeftWidth: 1,
            borderColor: C.cardBorder,
            marginLeft: 11,
            marginTop: 2,
            marginBottom: 4,
            gap: 1,
          }}
        >
          {/* 会话数量指示行（满足既有测试中会话计数的断言） */}
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 6,
              height: 20,
              paddingLeft: 6,
            }}
          >
            <text style={{ fontSize: 11, lineHeight: 15, fontWeight: 600, color: C.secondary }}>
              会话
            </text>
            <text style={{ fontSize: 10.5, lineHeight: 15, color: C.faint }}>
              {`${matchingThreads.length}`}
            </text>
          </div>


          {/* 会话列表项 */}
          {matchingThreads.map((thread) => (
            <SessionRow
              key={thread.id}
              thread={thread}
              store={store}
              selected={thread.id === store.activeId}
              running={store.isThreadRunning(thread.id)}
              onNotice={onNotice}
            />
          ))}
        </div>
      ) : null}
    </div>
  )
}

export function Sidebar({
  store,
  searchOpen,
  onCloseSearch,
}: {
  store: AgentStore
  searchOpen: boolean
  onCloseSearch: () => void
}) {
  const [projectsOpen, setProjectsOpen] = useState(true)
  const [query, setQuery] = useState('')
  const [notice, setNotice] = useState<string | null>(null)
  const [expandedWorkspaces, setExpandedWorkspaces] = useState<Record<string, boolean>>({})

  const project = store.project

  const isExpanded = (p: string) => {
    if (query.trim()) {
      const hasMatch = store.threads.some(
        (t) => t.workspace === p && t.title.toLowerCase().includes(query.trim().toLowerCase()),
      )
      if (hasMatch) return true
    }
    return expandedWorkspaces[p] ?? (p === project)
  }

  const toggleExpand = (p: string) => {
    setExpandedWorkspaces((prev) => ({
      ...prev,
      [p]: !isExpanded(p),
    }))
  }

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        width: M.sidebar,
        height: '100%',
        flexShrink: 0,
        backgroundColor: C.sidebar,
        borderRightWidth: 1,
        borderColor: C.sidebarBorder,
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          flexGrow: 1,
          minHeight: 0,
          overflowY: 'scroll',
          paddingTop: 12,
          paddingBottom: 12,
          paddingLeft: 8,
          paddingRight: 8,
        }}
      >
        {searchOpen ? (
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 7,
              height: 30,
              flexShrink: 0,
              marginBottom: 8,
              paddingLeft: 8,
              paddingRight: 5,
              borderRadius: 7,
              backgroundColor: C.raised,
              borderWidth: 1,
              borderColor: C.borderStrong,
            }}
          >
            <Icon name="search" size={12} color={C.tertiary} />
            <input
              testId="thread-search"
              value={query}
              autoFocus
              theme={editorTheme()}
              style={{
                flexGrow: 1,
                minWidth: 0,
                fontSize: 12.5,
                color: C.text,
                backgroundColor: '#00000000',
                borderWidth: 0,
              }}
              onChange={(event) => setQuery(event.value ?? '')}
            />
            <IconButton icon="close" size={11} label="关闭搜索" onClick={onCloseSearch} />
          </div>
        ) : null}

        {/* 新建对话主操作按键（位于工作区上方） */}
        <div
          testId="sidebar-new-chat"
          role="button"
          aria-label="新建对话"
          onClick={() => {
            store.newThread()
          }}
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 8,
            height: 34,
            paddingLeft: 10,
            paddingRight: 10,
            marginBottom: 10,
            borderRadius: 8,
            backgroundColor: C.card,
            borderWidth: 1,
            borderColor: C.borderStrong,
            cursor: 'pointer',
            hover: { backgroundColor: C.overlay, borderColor: C.link },
          }}
        >
          <Icon name="plus" size={13} color={C.link} />
          <text style={{ fontSize: 13, fontWeight: 600, color: C.text, flexGrow: 1 }}>新建对话</text>
          <Icon name="chevronRight" size={11} color={C.faint} />
        </div>

        <SectionHeader
          label="工作区"
          open={projectsOpen}
          onToggle={() => setProjectsOpen((open) => !open)}
          count={`${store.projects.length}`}
        />
        {projectsOpen ? (
          <>
            {store.projects.map((p) => (
              <WorkspaceTreeNode
                key={p}
                workspacePath={p}
                store={store}
                query={query}
                expanded={isExpanded(p)}
                onToggleExpand={() => toggleExpand(p)}
                onNotice={setNotice}
              />
            ))}
          </>
        ) : null}

        {notice ? (
          <div style={{ paddingLeft: 7, paddingTop: 4, paddingBottom: 2 }}>
            <text style={{ fontSize: 11, lineHeight: 15, color: C.accent }}>{notice}</text>
          </div>
        ) : null}
      </div>

      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'flex-end',
          gap: 4,
          height: 42,
          flexShrink: 0,
          paddingLeft: 8,
          paddingRight: 8,
          borderTopWidth: 1,
          borderColor: C.sidebarBorder,
        }}
      >
        <text style={{ fontSize: 11, color: C.faint, flexGrow: 1 }}>
          {store.workspaceInfo.scanning ? '索引中…' : `${store.workspaceInfo.files} 个文件`}
        </text>
        <IconButton
          icon={store.appearance === 'dark' ? 'sun' : 'moon'}
          testId="toggle-appearance"
          label={store.appearance === 'dark' ? '切换到浅色模式' : '切换到深色模式'}
          onClick={() => store.toggleAppearance()}
        />
        <IconButton
          icon="settings"
          testId="open-settings"
          label="设置"
          onClick={() => store.setSettings(true)}
        />
        <IconButton
          icon="plug"
          testId="open-plugins"
          label="插件管理"
          onClick={() => store.setPlugins(true)}
        />
        <IconButton
          icon="refresh"
          testId="refresh-workspace"
          label="刷新工作区"
          onClick={() => void store.refresh()}
        />
      </div>
    </div>
  )
}
