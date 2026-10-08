/**
 * The window title row.
 *
 * GPUI opens a borderless window on Windows, so this row is the caption: it
 * drags the window, and its three buttons minimize, maximize and close. On
 * macOS the same row is drawn and the native traffic lights sit on top of it,
 * so the buttons are skipped there.
 */

import React, { useRef } from 'react'
import { ICONS } from '../icons'
import { Icon, IconButton } from './controls'
import { C, M, type Appearance } from '../theme'
import { windowControls } from '../platform/win32'

function WindowButton({
  icon,
  onClick,
  label,
  testId,
  danger,
}: {
  icon: 'minus' | 'square' | 'close'
  onClick: () => void
  label: string
  testId: string
  danger?: boolean
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
        justifyContent: 'center',
        width: 44,
        height: M.windowButton,
        cursor: 'pointer',
        backgroundColor: '#00000000',
        // GPUI paints a child with no colour of its own in the parent's text
        // colour, so the glyph follows this hover too: white on the red close.
        color: C.secondary,
        hover: danger
          ? { backgroundColor: '#E81123', color: '#FFFFFF' }
          : { backgroundColor: C.overlayStrong },
      }}
    >
      <svg
        source={ICONS[icon]}
        style={{
          width: icon === 'close' ? 13 : 12,
          height: icon === 'close' ? 13 : 12,
          flexShrink: 0,
          pointerEvents: 'none',
        }}
      />
    </div>
  )
}

export function TitleBar({
  title,
  onToggleSidebar,
  onSearch,
  onToggleAppearance,
  appearance,
  onDragNotice,
}: {
  title: string
  onToggleSidebar: () => void
  onSearch: () => void
  onToggleAppearance: () => void
  appearance: Appearance
  onDragNotice?: (text: string) => void
}) {
  /**
   * The window is moved here, from the pointer events: gpui reports the cursor
   * in this window's client space, and `moveDrag` turns the distance from the
   * press into a new window position.
   *
   * These handlers live on the drag surface and the tab, never on the strip
   * itself. Listening for down *and* move is what arms GPUIX's pointer capture,
   * and a capturing ancestor swallows the clicks of its children — the minimize,
   * maximize and close buttons stopped working when this was on the container.
   * The drag surface holds nothing clickable, so capture there costs nothing.
   *
   * Moves that do not report the left button are ignored rather than treated as
   * a release: moving the window generates those, and ending the gesture on one
   * cut the drag in half.
   */
  const dragging = useRef(false)

  const dragHandlers = windowControls.custom
    ? {
        onMouseDown: (event: { x?: number; y?: number }) => {
          const result = windowControls.beginDrag({ x: event.x ?? 0, y: event.y ?? 0 })
          if (result.ok) {
            dragging.current = true
            return
          }
          onDragNotice?.(`拖动失败：${result.detail}`)
        },
        onMouseMove: (event: { x?: number; y?: number; pressedButton?: number | null }) => {
          if (!dragging.current) return
          // Only a move that reports the left button is part of the gesture. A
          // move without one arrives whenever the window itself moves under the
          // pointer, and acting on those would fight the drag.
          if (event.pressedButton !== 0) return
          const applied = windowControls.moveDrag({ x: event.x ?? 0, y: event.y ?? 0 })
          // A move that was not applied means the gesture broke, which is worth
          // a line in the event log. Applied positions stay out of it: there is
          // one per frame, and the panel has to stay readable.
          if (!/^\d/.test(applied)) onDragNotice?.(`拖动中 ${event.x},${event.y}：${applied}`)
        },
        onMouseUp: () => {
          dragging.current = false
          windowControls.endDrag()
        },
      }
    : {}

  return (
    <div
      testId="titlebar"
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        height: M.titleBar,
        flexShrink: 0,
        backgroundColor: C.canvas,
        borderBottomWidth: 1,
        borderColor: C.sidebarBorder,
        paddingLeft: 6,
        paddingRight: windowControls.custom ? 0 : 6,
        userSelect: 'none',
      }}
    >
      <IconButton
        icon="sidebar"
        size={13}
        testId="toggle-sidebar"
        label="收起侧边栏"
        onClick={onToggleSidebar}
        style={{ width: M.barButton, height: M.barButton }}
      />
      <IconButton
        icon="search"
        size={13}
        testId="search"
        label="搜索会话"
        onClick={onSearch}
        style={{ width: M.barButton, height: M.barButton }}
      />

      <div
        style={{
          width: 1,
          height: 14,
          backgroundColor: C.border,
          marginLeft: 5,
          marginRight: 7,
          flexShrink: 0,
        }}
      />
      <div
        {...dragHandlers}
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 6,
          height: M.tab,
          maxWidth: 420,
          paddingLeft: 8,
          paddingRight: 10,
          borderRadius: 6,
          backgroundColor: C.tab,
          borderWidth: 1,
          borderColor: C.border,
          flexShrink: 1,
          minWidth: 0,
        }}
      >
        <Icon name="thread" size={11} color={C.tertiary} />
        <text
          style={{
            fontSize: 12,
            lineHeight: 15,
            color: C.text,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            flexShrink: 1,
            minWidth: 0,
          }}
        >
          {title}
        </text>
      </div>
      <div
        testId="titlebar-drag"
        {...dragHandlers}
        style={{
          flexGrow: 1,
          height: '100%',
          // A transparent fill is what gives the surface a hitbox to grab.
          backgroundColor: '#00000000',
        }}
      />
      {windowControls.custom ? (
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', flexShrink: 0 }}>
          <WindowButton
            icon="minus"
            testId="win-minimize"
            label="最小化"
            onClick={() => windowControls.minimize()}
          />
          <WindowButton
            icon="square"
            testId="win-maximize"
            label="最大化"
            onClick={() => windowControls.toggleMaximize()}
          />
          <WindowButton
            icon="close"
            testId="win-close"
            label="关闭"
            danger
            onClick={() => windowControls.close()}
          />
        </div>
      ) : null}
    </div>
  )
}
