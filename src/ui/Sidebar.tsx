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
import { C, FONT_SANS, M, shortPath } from '../theme'
import { pickDirectory } from '../platform/dialog'
import type { AgentStore } from '../agent/store'
import type { Thread } from '../agent/types'

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
          <text style={{ fontSize: 11, lineHeight: 15, color: C.faint, flexShrink: 0 }}>运行中</text>
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
  const [picking, setPicking] = useState(false)
  const [path, setPath] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const project = store.project
  const threads = store.projectThreads.filter((thread) =>
    query.trim() ? thread.title.toLowerCase().includes(query.trim().toLowerCase()) : true,
  )

  const add = async (chosen: string): Promise<void> => {
    const message = await store.addProject(chosen)
    setError(message)
    if (message) return
    setAdding(false)
    setPath('')
  }

  /**
   * 点「添加项目」：先把手输框摆出来，再去开原生目录选择器。
   *
   * 弹窗选中就直接加进来；取消或开不出来（非 Windows、自动化、A_DA_NO_DIALOG）
   * 时那个输入框就留在原地当退路——用户至少还能粘贴一个路径。
   */
  const startAdd = (): void => {
    setError(null)
    setAdding(true)
    setPicking(true)
    void pickDirectory(project)
      .then((result) => (result.status === 'picked' ? add(result.path) : undefined))
      .finally(() => setPicking(false))
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
          label="工作区"
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
                  placeholder="也可以直接粘贴路径，回车添加"
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
                  onSubmit={() => void add(path)}
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
                label={picking ? '正在打开目录选择…' : '添加项目'}
                tone="muted"
                onClick={startAdd}
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

        <SectionHeader label="会话" count={`${threads.length}`} />
        <Row
          testId="new-thread"
          icon="plus"
          label="新建会话"
          tone="muted"
          onClick={() => store.newThread()}
        />
        {threads.map((thread) => (
          <SessionRow
            key={thread.id}
            thread={thread}
            store={store}
            selected={thread.id === store.activeId}
            running={thread.id === store.activeId && store.running}
            onNotice={setNotice}
          />
        ))}
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
