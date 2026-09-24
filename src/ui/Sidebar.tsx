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
import { getSubagentColor } from '../agent/subagents/types'
import { openInExplorer } from '../platform/explorer'

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
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          flexGrow: 1,
          flexShrink: 1,
          minWidth: 0,
        }}
      >
        {label}
      </text>
      {sub ? <text style={{ fontSize: 11, lineHeight: 15, color: C.faint, flexShrink: 0 }}>{sub}</text> : null}
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
  hasChildren,
  childCount,
  isExpanded,
  onToggleExpand,
  hasRunningChildren,
}: {
  thread: Thread
  store: AgentStore
  selected: boolean
  running: boolean
  onNotice: (message: string | null) => void
  hasChildren?: boolean
  childCount?: number
  isExpanded?: boolean
  onToggleExpand?: () => void
  hasRunningChildren?: boolean
}) {
  const [hovered, setHovered] = useState(false)

  const remove = (e: any): void => {
    e?.stopPropagation?.()
    store.showConfirm({
      title: '删除会话',
      message: `确定要删除会话「${thread.title}」吗？删除后会话记录将无法恢复。`,
      confirmText: '确认删除',
      onConfirm: () => {
        onNotice(store.deleteThread(thread.id))
      },
    })
  }

  return (
    <div
      testId={`thread-${thread.id}`}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
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
      {/* 若有子智能体会话，最左侧显示折叠/展开切换箭头 */}
      {hasChildren ? (
        <div
          testId={`toggle-subagents-${thread.id}`}
          role="button"
          aria-label={isExpanded ? '收起子智能体会话' : '展开子智能体会话'}
          onClick={(e: any) => {
            e?.stopPropagation?.()
            onToggleExpand?.()
          }}
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            width: 16,
            height: 16,
            marginLeft: 4,
            borderRadius: 3,
            cursor: 'pointer',
            flexShrink: 0,
            hover: { backgroundColor: C.chipHover },
          }}
        >
          <Icon name={isExpanded ? 'chevronDown' : 'chevronRight'} size={10} color={C.faint} />
        </div>
      ) : null}

      <div
        role="button"
        aria-label={thread.title}
        onClick={() => store.selectThread(thread.id)}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 6,
          height: '100%',
          flexGrow: 1,
          minWidth: 0,
          paddingLeft: hasChildren ? 2 : 6,
          paddingRight: 4,
          overflow: 'hidden',
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
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            flexGrow: 1,
            flexShrink: 1,
            minWidth: 0,
          }}
        >
          {thread.title}
        </text>

        {/* 折叠时显示子智能体数量徽标 */}
        {hasChildren && !isExpanded && childCount ? (
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
              flexShrink: 0,
            }}
          >
            <text style={{ fontSize: 9.5, lineHeight: 13, color: C.faint }}>
              {childCount}
            </text>
          </div>
        ) : null}

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
        ) : hasChildren && !isExpanded && hasRunningChildren ? (
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 3,
              paddingLeft: 4,
              paddingRight: 4,
              height: 15,
              borderRadius: 3,
              backgroundColor: C.chip,
              marginRight: 4,
              flexShrink: 0,
            }}
          >
            <Icon name="dot" size={5} color={C.success} />
            <text style={{ fontSize: 9.5, lineHeight: 13, color: C.link }}>运行中</text>
          </div>
        ) : null}
      </div>

      <div
        testId={`delete-thread-${thread.id}`}
        role="button"
        aria-label="删除会话"
        onClick={remove}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'center',
          width: 24,
          height: '100%',
          flexShrink: 0,
          paddingRight: 6,
          cursor: 'pointer',
          opacity: hovered ? 1 : 0,
        }}
      >
        <Icon name="trash" size={12} color={C.faint} />
      </div>
    </div>
  )
}

/**
 * 子智能体专属会话行：以树形缩进（带左连接竖线与 bot 图标）展示在所属主会话之下。
 */
function SubagentSessionRow({
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
  const [hovered, setHovered] = useState(false)
  const agentColor = getSubagentColor(thread.subagentId)

  const remove = (e: any): void => {
    e?.stopPropagation?.()
    store.showConfirm({
      title: '删除子会话',
      message: `确定要删除子会话「${thread.title}」吗？删除后该子智能体会话记录将无法恢复。`,
      confirmText: '确认删除',
      onConfirm: () => {
        onNotice(store.deleteThread(thread.id))
      },
    })
  }

  return (
    <div
      testId={`thread-${thread.id}`}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        height: 26,
        marginLeft: 14,
        paddingLeft: 8,
        borderLeftWidth: 2,
        borderColor: selected ? agentColor : C.cardBorder,
        borderRadius: 5,
        flexShrink: 0,
        backgroundColor: selected ? C.tab : '#00000000',
        hover: { backgroundColor: selected ? C.tab : C.overlay },
      }}
    >
      <div
        role="button"
        aria-label={`子智能体: ${thread.title}`}
        onClick={() => {
          store.openTab(thread.id)
          store.selectThread(thread.id)
        }}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 6,
          height: '100%',
          flexGrow: 1,
          minWidth: 0,
          paddingRight: 4,
          overflow: 'hidden',
          cursor: 'pointer',
        }}
      >
        <Icon name="bot" size={12} color={agentColor} />
        <text
          style={{
            fontSize: 11.5,
            lineHeight: 15,
            color: selected ? C.text : C.secondary,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            flexGrow: 1,
            flexShrink: 1,
            minWidth: 0,
          }}
        >
          {thread.title}
        </text>
        {running ? (
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 3,
              paddingLeft: 4,
              paddingRight: 4,
              height: 15,
              borderRadius: 3,
              backgroundColor: C.chip,
              marginRight: 4,
              flexShrink: 0,
            }}
          >
            <Icon name="dot" size={5} color={C.success} />
            <text style={{ fontSize: 9.5, lineHeight: 13, color: C.link }}>运行中</text>
          </div>
        ) : null}
      </div>

      <div
        testId={`delete-thread-${thread.id}`}
        role="button"
        aria-label="删除子会话"
        onClick={remove}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'center',
          width: 22,
          height: '100%',
          flexShrink: 0,
          paddingRight: 5,
          borderRadius: 3,
          cursor: 'pointer',
          opacity: hovered ? 1 : 0,
          hover: { backgroundColor: C.chipHover },
        }}
      >
        <Icon name="trash" size={11} color={C.faint} />
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
  const [hovered, setHovered] = useState(false)
  const [collapsedParentIds, setCollapsedParentIds] = useState<Set<string>>(new Set())

  const toggleSubagentsExpand = (threadId: string) => {
    setCollapsedParentIds((prev) => {
      const next = new Set(prev)
      if (next.has(threadId)) {
        next.delete(threadId)
      } else {
        next.add(threadId)
      }
      return next
    })
  }
  const isCurrent = workspacePath === store.project
  const allWorkspaceThreads = store.threads.filter((t) => t.workspace === workspacePath)
  const rootThreads = allWorkspaceThreads.filter((t) => !t.parentId)
  const queryLower = query.trim().toLowerCase()

  const getRootParentId = (t: Thread): string | undefined => {
    let curr: Thread | undefined = t
    const visited = new Set<string>()
    while (curr && curr.parentId && !visited.has(curr.id)) {
      visited.add(curr.id)
      const parent = allWorkspaceThreads.find((p) => p.id === curr!.parentId)
      if (!parent) return curr.parentId
      if (!parent.parentId) return parent.id
      curr = parent
    }
    return undefined
  }

  const matchingRootThreads = rootThreads.filter((thread) => {
    if (!queryLower) return true
    if (thread.title.toLowerCase().includes(queryLower)) return true
    return allWorkspaceThreads.some(
      (child) =>
        (child.parentId === thread.id || getRootParentId(child) === thread.id) &&
        child.title.toLowerCase().includes(queryLower),
    )
  })
  const orphanedSubagents = allWorkspaceThreads.filter(
    (t) =>
      Boolean(t.parentId) &&
      !rootThreads.some((r) => r.id === t.parentId || r.id === getRootParentId(t)) &&
      (!queryLower || t.title.toLowerCase().includes(queryLower)),
  )

  const label = shortPath(workspacePath, 2)

  return (
    <div style={{ display: 'flex', flexDirection: 'column', width: '100%', flexShrink: 0 }}>
      {/* 根节点头部（工作区行） */}
      <div
        testId={`project-${label}`}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 6,
          height: 28,
          paddingLeft: 8,
          paddingRight: 8,
          borderRadius: 5,
          backgroundColor: isCurrent ? C.chip : '#00000000',
          hover: { backgroundColor: C.chipHover },
        }}
      >
        {/* 折叠/展开箭头 */}
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
            paddingRight: 4,
            overflow: 'hidden',
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
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              flexGrow: 1,
              flexShrink: 1,
              minWidth: 0,
            }}
          >
            {label}
          </text>
          {/* 会话数指示徽章：折叠态显示，展开态已有子列表不重复展示 */}
          {!expanded && allWorkspaceThreads.length > 0 ? (
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
                flexShrink: 0,
              }}
            >
              <text style={{ fontSize: 10, lineHeight: 14, color: C.faint }}>
                {`${allWorkspaceThreads.length}`}
              </text>
            </div>
          ) : null}
        </div>

        {/* 右侧操作按钮组：新建会话与移除工作区 */}
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 2, flexShrink: 0 }}>
          {/* 快捷新建会话按钮 (+)：仅在展开态展示 */}
          {expanded ? (
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
                opacity: hovered ? 1 : 0,
                hover: { backgroundColor: C.chipHover },
              }}
            >
              <Icon name="plus" size={11} color={C.tertiary} />
            </div>
          ) : null}

          {/* 从文件资源管理器打开 */}
          <div
            testId={`open-explorer-${label}`}
            role="button"
            aria-label={`在文件资源管理器中打开 ${label}`}
            onClick={(e: any) => {
              e?.stopPropagation?.()
              const ok = openInExplorer(workspacePath)
              if (!ok) {
                onNotice(`无法在文件资源管理器中打开工作区「${label}」：目标路径不存在或无法访问`)
              }
            }}
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              justifyContent: 'center',
              width: 18,
              height: 18,
              borderRadius: 4,
              cursor: 'pointer',
              opacity: hovered ? 1 : 0,
              hover: { backgroundColor: C.chipHover },
            }}
          >
            <Icon name="folderOpen" size={11} color={C.faint} />
          </div>

          {/* 移除工作区按钮 (垃圾桶) */}
          <div
            testId={`remove-project-${label}`}
            role="button"
            aria-label={`移除工作区 ${label}`}
            onClick={() => {
              store.showConfirm({
                title: '移除工作区',
                message: `确定要从列表中移除工作区「${label}」吗？工作区下的会话历史记录将被清除，但本地实际代码文件不会被删除。`,
                confirmText: '确认移除',
                onConfirm: () => {
                  const err = store.removeProject(workspacePath)
                  if (err) onNotice(err)
                },
              })
            }}
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              justifyContent: 'center',
              width: 18,
              height: 18,
              borderRadius: 4,
              cursor: 'pointer',
              opacity: hovered ? 1 : 0,
              hover: { backgroundColor: C.chipHover },
            }}
          >
            <Icon name="trash" size={11} color={C.faint} />
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
              {`${matchingRootThreads.length}`}
            </text>
          </div>

          {/* 会话列表项：主会话及嵌套子智能体项 */}
          {matchingRootThreads.map((thread) => {
            const childSubagents = allWorkspaceThreads.filter(
              (t) =>
                (t.parentId === thread.id || getRootParentId(t) === thread.id) &&
                (!queryLower ||
                  t.title.toLowerCase().includes(queryLower) ||
                  thread.title.toLowerCase().includes(queryLower)),
            )
            const hasChildren = childSubagents.length > 0
            const hasActiveChild = childSubagents.some((c) => c.id === store.activeId)
            const isSubagentsExpanded = hasChildren && (!collapsedParentIds.has(thread.id) || hasActiveChild)
            const hasRunningChildren = childSubagents.some((c) => store.isThreadRunning(c.id))

            return (
              <div
                key={thread.id}
                style={{ display: 'flex', flexDirection: 'column', width: '100%', gap: 1 }}
              >
                <SessionRow
                  thread={thread}
                  store={store}
                  selected={thread.id === store.activeId}
                  running={store.isThreadRunning(thread.id)}
                  onNotice={onNotice}
                  hasChildren={hasChildren}
                  childCount={childSubagents.length}
                  isExpanded={isSubagentsExpanded}
                  onToggleExpand={() => toggleSubagentsExpand(thread.id)}
                  hasRunningChildren={hasRunningChildren}
                />
                {isSubagentsExpanded
                  ? childSubagents.map((child) => (
                      <SubagentSessionRow
                        key={child.id}
                        thread={child}
                        store={store}
                        selected={child.id === store.activeId}
                        running={store.isThreadRunning(child.id)}
                        onNotice={onNotice}
                      />
                    ))
                  : null}
              </div>
            )
          })}
          {orphanedSubagents.map((child) => (
            <SubagentSessionRow
              key={child.id}
              thread={child}
              store={store}
              selected={child.id === store.activeId}
              running={store.isThreadRunning(child.id)}
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
