/**
 * Projects and threads.
 *
 * One project row shows the workspace the agent is pinned to, and the thread
 * list is the session history. Both are read from the store, which re-renders
 * this column whenever a turn changes something.
 */

import React, { useState } from 'react'
import { Icon, IconButton } from './controls'
import { C, FONT_SANS, M, shortPath } from '../theme'
import type { AgentStore } from '../agent/store'

function SectionHeader({
  label,
  open,
  onToggle,
  count,
}: {
  label: string
  open?: boolean
  onToggle?: () => void
  count?: string
}) {
  return (
    <div
      onClick={onToggle}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
        height: 24,
        paddingLeft: 6,
        paddingRight: 6,
        borderRadius: 6,
        cursor: onToggle ? 'pointer' : undefined,
        flexShrink: 0,
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
  const [adding, setAdding] = useState(false)
  const [path, setPath] = useState('')
  const [error, setError] = useState<string | null>(null)

  const project = store.project
  const threads = store.projectThreads.filter((thread) =>
    query.trim() ? thread.title.toLowerCase().includes(query.trim().toLowerCase()) : true,
  )

  const add = async () => {
    const message = await store.addProject(path)
    setError(message)
    if (message) return
    setAdding(false)
    setPath('')
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
              theme={{ caret: C.link, fontSans: FONT_SANS }}
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

        <SectionHeader
          label="Projects"
          open={projectsOpen}
          onToggle={() => setProjectsOpen((open) => !open)}
        />
        {projectsOpen ? (
          <>
            {store.projects.map((path) => (
              <Row
                key={path}
                testId={`project-${shortPath(path, 2)}`}
                icon="folder"
                label={shortPath(path, 2)}
                sub={
                  path === project && !store.workspaceInfo.scanning
                    ? `${store.workspaceInfo.files}`
                    : undefined
                }
                selected={path === project}
                onClick={() => store.selectProject(path)}
              />
            ))}
            {adding ? (
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 7,
                  height: 30,
                  flexShrink: 0,
                  marginTop: 2,
                  paddingLeft: 8,
                  paddingRight: 5,
                  borderRadius: 7,
                  backgroundColor: C.raised,
                  borderWidth: 1,
                  borderColor: error ? C.accent : C.borderStrong,
                }}
              >
                <Icon name="folder" size={12} color={error ? C.accent : C.tertiary} />
                <input
                  testId="project-path"
                  value={path}
                  autoFocus
                  placeholder="项目目录，回车添加"
                  theme={{ caret: C.link, fontSans: FONT_SANS }}
                  style={{
                    flexGrow: 1,
                    minWidth: 0,
                    fontSize: 12.5,
                    color: C.text,
                    backgroundColor: '#00000000',
                    borderWidth: 0,
                  }}
                  onChange={(event) => setPath(event.value ?? '')}
                  onSubmit={() => void add()}
                />
                <IconButton
                  icon="close"
                  size={11}
                  label="取消"
                  onClick={() => {
                    setAdding(false)
                    setError(null)
                  }}
                />
              </div>
            ) : (
              <Row
                testId="add-project"
                icon="plus"
                label="添加项目"
                tone="muted"
                onClick={() => setAdding(true)}
              />
            )}
            {error ? (
              <div style={{ paddingLeft: 7, paddingTop: 4, paddingBottom: 2 }}>
                <text style={{ fontSize: 11, lineHeight: 15, color: C.accent }}>{error}</text>
              </div>
            ) : null}
          </>
        ) : null}

        <div style={{ height: 18, flexShrink: 0 }} />

        <SectionHeader label="Threads" count={`${threads.length}`} />
        <Row
          testId="new-thread"
          icon="plus"
          label="新建会话"
          tone="muted"
          onClick={() => store.newThread()}
        />
        {threads.map((thread) => (
          <Row
            key={thread.id}
            testId={`thread-${thread.id}`}
            icon="thread"
            label={thread.title}
            sub={thread.id === store.activeId && store.running ? '运行中' : undefined}
            selected={thread.id === store.activeId}
            onClick={() => store.selectThread(thread.id)}
          />
        ))}
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
          icon="settings"
          testId="open-settings"
          label="设置"
          onClick={() => store.setSettings(true)}
        />
        <IconButton
          icon="plug"
          testId="status"
          label="工作区状态"
          onClick={() => void store.refresh()}
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
