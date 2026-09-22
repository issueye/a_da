/**
 * 会话标签页。
 *
 * 一条横向的标签栏，摆在内容区顶上。**标签是视图，不是数据**：它记的是「现在
 * 开着哪几个会话」（`store.openTabs`），而会话本体在 `store.threads` 里、由
 * 侧边栏那份列表负责。所以：
 *
 * - 关标签只是关掉视图，会话和盘上的流水都不动，从侧边栏再点一下就回来了；
 * - 关标签因此不需要二次确认（对比侧边栏的垃圾桶：那个是删数据，要点两下）。
 *
 * 两者用同一个 `Thread`，切换、标题、运行状态自然一致。
 */

import React from 'react'
import { Icon } from './controls'
import { C, M } from '../theme'
import type { AgentStore } from '../agent/store'
import type { Thread } from '../agent/types'
import { getSubagentColor } from '../agent/subagents/types'

/**
 * 一个标签。
 *
 * 关闭按钮和选择区是**兄弟**而不是嵌套：click 会往上冒泡，如果 × 在带 onClick
 * 的元素里面，点 × 就会顺带把会话切过去再关掉。侧边栏的垃圾桶行同理，但那边的
 * × 是删数据，要点两下；这里是关视图，可逆，所以一下就关。
 */
function Tab({
  thread,
  selected,
  running,
  closable,
  onSelect,
  onClose,
}: {
  thread: Thread
  selected: boolean
  running: boolean
  /** 关不掉的不显示 ×：只有当前项目最后一个标签会这样，见 store.closeTab。 */
  closable: boolean
  onSelect: () => void
  onClose: () => void
}) {
  return (
    <div
      testId={`tab-${thread.id}`}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        height: M.tab,
        // 固定宽度，不按内容撑开：标题长短不一时整条标签看起来是齐的。装不下就
        // 横向滚动。
        width: 176,
        flexShrink: 0,

        borderRadius: 6,
        backgroundColor: selected ? C.tab : '#00000000',
        borderWidth: 1,
        // 选中的那个才描边：一列标签里只有它是「当前」。
        borderColor: selected ? C.border : '#00000000',
        hover: { backgroundColor: selected ? C.tab : C.overlay },
      }}
    >
      <div
        role="button"
        aria-label={thread.title}
        onClick={onSelect}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 6,
          height: '100%',
          flexGrow: 1,
          minWidth: 0,
          paddingLeft: 9,
          // 右边那 2px 是留给 × / 状态点的：紧贴着它，而不是和左边的图标等距。
          paddingRight: 2,
          cursor: 'pointer',
        }}
      >
        <Icon
          name={thread.isSubagent ? 'bot' : 'thread'}
          size={11}
          color={thread.isSubagent ? getSubagentColor(thread.subagentId) : selected ? C.secondary : C.faint}
        />
        <text
          style={{
            fontSize: 12,
            lineHeight: 15,
            color: selected ? C.text : C.secondary,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            // 标签是固定宽度，标题占满剩下的空间，长了才出省略号。
            flexGrow: 1,
            minWidth: 0,
          }}
        >
          {thread.title}
        </text>
      </div>

      {/*
        两种情况下不给关闭：跑着的那一轮（关掉会让正在跑的东西从视野里消失），
        以及当前项目最后一个标签（关掉就没内容可显示了）。都用一个位置的预留
        代替 ×，标签宽度不变。
      */}
      {running ? (
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            paddingLeft: 4,
            paddingRight: 8,
            flexShrink: 0,
          }}
        >
          <Icon name="dot" size={7} color={C.success} />
        </div>
      ) : closable ? (
        <div
          testId={`close-tab-${thread.id}`}
          role="button"
          aria-label="关闭标签页"
          onClick={onClose}
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            justifyContent: 'center',
            width: 18,
            height: 18,
            marginRight: 4,
            borderRadius: 4,
            flexShrink: 0,
            cursor: 'pointer',
            hover: { backgroundColor: C.overlayStrong },
          }}
        >
          <Icon name="close" size={11} color={C.faint} />
        </div>
      ) : (
        <div style={{ width: 4, flexShrink: 0 }} />
      )}
    </div>
  )
}

export function TabStrip({ store }: { store: AgentStore }) {
  const tabs = store.openTabs

  return (
    <div
      testId="tab-strip"
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        height: M.tabStrip,
        flexShrink: 0,
        gap: 2,
        paddingLeft: 6,
        paddingRight: 6,
        backgroundColor: C.canvas,
        borderBottomWidth: 1,
        borderColor: C.sidebarBorder,
      }}
    >
      {/*
        只有标签列表滚动，`+` 留在滚动区外：滚到后面时「新建」还得随手能点，
        它被推出视野就只能先滚回去。
      */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 2,
          flexGrow: 1,
          minWidth: 0,
          height: '100%',
          overflowX: 'scroll',
        }}
      >
        {tabs.map((thread) => (
          <Tab
            key={thread.id}
            thread={thread}
            selected={thread.id === store.activeId}
            running={store.isThreadRunning(thread.id)}
            closable={tabs.length > 1}
            onSelect={() => store.selectThread(thread.id)}
            onClose={() => store.closeTab(thread.id)}
          />
        ))}
      </div>
      <div
        testId="new-tab"
        role="button"
        aria-label="新建会话"
        onClick={() => store.newThread()}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'center',
          width: M.tab,
          height: M.tab,
          borderRadius: 6,
          flexShrink: 0,
          cursor: 'pointer',
          hover: { backgroundColor: C.overlay },
        }}
      >
        <Icon name="plus" size={12} color={C.faint} />
      </div>
    </div>
  )
}
