/**
 * The few pieces every panel reuses: an icon, an icon button, a chip select and
 * the menu row its options render in.
 */

import React from 'react'
import { Select, SelectContent, SelectItem, SelectTrigger, type StyleDesc } from '@gpuix/react'
import { ICONS, type IconName } from '../icons'
import { C, M } from '../theme'

export function Icon({
  name,
  size = 14,
  color = C.secondary,
}: {
  name: IconName
  size?: number
  color?: string
}) {
  return (
    <svg
      source={ICONS[name]}
      style={{ width: size, height: size, flexShrink: 0, color, pointerEvents: 'none' }}
    />
  )
}

export function IconButton({
  icon,
  size = 14,
  color,
  onClick,
  testId,
  label,
  disabled,
  style,
}: {
  icon: IconName
  size?: number
  color?: string
  onClick?: () => void
  testId?: string
  label?: string
  disabled?: boolean
  style?: StyleDesc
}) {
  return (
    <div
      testId={testId}
      role="button"
      aria-label={label ?? icon}
      onClick={disabled ? undefined : onClick}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'center',
        width: M.iconButton,
        height: M.iconButton,
        borderRadius: 6,
        flexShrink: 0,
        cursor: disabled ? 'default' : 'pointer',
        backgroundColor: '#00000000',
        hover: disabled ? undefined : { backgroundColor: C.overlay },
        active: disabled ? undefined : { backgroundColor: C.overlayStrong },
        ...style,
      }}
    >
      <Icon name={icon} size={size} color={disabled ? C.ghost : (color ?? C.secondary)} />
    </div>
  )
}

/** A chip that acts instead of opening a menu, like the composer's 刷新. */
export function ChipButton({
  icon,
  label,
  onClick,
  testId,
  active,
}: {
  icon: IconName
  label: string
  onClick?: () => void
  testId?: string
  active?: boolean
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
        gap: 5,
        height: M.row,
        paddingLeft: 7,
        paddingRight: 7,
        borderRadius: 7,
        flexShrink: 0,
        cursor: 'pointer',
        backgroundColor: active ? C.chip : '#00000000',
        hover: { backgroundColor: C.chip },
      }}
    >
      <Icon name={icon} size={12} color={active ? C.text : C.tertiary} />
      <text
        style={{
          fontSize: 12.5,
          lineHeight: 16,
          color: active ? C.text : C.secondary,
          whiteSpace: 'nowrap',
        }}
      >
        {label}
      </text>
    </div>
  )
}

/**
 * A menu is two boxes, not one.
 *
 * `<anchored>` paints an opaque `#1A1A1A` behind its children so a deferred
 * overlay is never see-through, and the floating layer's own div covers that. If
 * the rounded, bordered card *is* that div, its four corners cut into the dark
 * fill and the menu looks like it has black corners — invisible in the dark mode,
 * glaring on the light one. So the layer gets a square fill of the same colour,
 * and the radius, border and shadow live on a card inside it.
 *
 * Both are functions rather than constants because the palette is installed at
 * runtime: a module-level object would freeze whatever mode was active at import
 * and never follow a switch.
 */
export function menuLayer(): StyleDesc {
  return { backgroundColor: C.raised }
}

export function menuCard(): StyleDesc {
  return {
    display: 'flex',
    flexDirection: 'column',
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 4,
    paddingRight: 4,
    backgroundColor: C.raised,
    borderWidth: 1,
    borderColor: C.borderStrong,
    borderRadius: 10,
    overflow: 'hidden',
    boxShadow: { offsetX: 0, offsetY: 6, blurRadius: 18, spreadRadius: 0, color: C.shadow },
  }
}

/** The rounded surface a menu's rows sit on, inside the square layer. */
export function MenuSurface({
  children,
  maxHeight,
}: {
  children: React.ReactNode
  maxHeight?: number
}) {
  return (
    <div
      style={{
        ...menuCard(),
        ...(maxHeight ? { maxHeight, overflowY: 'scroll' as const } : {}),
      }}
    >
      {children}
    </div>
  )
}

export function menuItemStyle(state: { selected: boolean; highlighted: boolean }): StyleDesc {
  return {
    display: 'flex',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    width: '100%',
    borderRadius: 6,
    backgroundColor: state.highlighted ? C.chip : state.selected ? C.overlayStrong : '#00000000',
    hover: { backgroundColor: C.chip },
    cursor: 'pointer',
  }
}

export function MenuRow({
  label,
  description,
  selected,
  hint,
}: {
  label: string
  description?: string
  selected: boolean
  hint?: string
}) {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
        width: '100%',
        paddingTop: description ? 5 : 4,
        paddingBottom: description ? 5 : 4,
        paddingLeft: 8,
        paddingRight: 8,
      }}
    >
      <div style={{ display: 'flex', flexDirection: 'column', flexGrow: 1, minWidth: 0 }}>
        <text
          style={{
            fontSize: 12.5,
            lineHeight: 16,
            fontWeight: selected ? 600 : 500,
            color: C.text,
            whiteSpace: 'nowrap',
            textOverflow: 'ellipsis',
          }}
        >
          {label}
        </text>
        {description ? (
          <text style={{ fontSize: 11.5, lineHeight: 15, color: C.tertiary }}>{description}</text>
        ) : null}
      </div>
      {hint ? <text style={{ fontSize: 11, color: C.faint }}>{hint}</text> : null}
      {selected ? <Icon name="check" size={11} color={C.tertiary} /> : null}
    </div>
  )
}

/** The composer and sidebar chips: a small select that opens a styled menu. */
export function ChipSelect({
  value,
  onChange,
  items,
  icon,
  label,
  caret = true,
  testId,
  menuWidth,
  strong,
  children,
}: {
  value: string
  onChange: (next: string) => void
  items: { value: string; label: string }[]
  icon?: IconName
  label: string
  caret?: boolean
  testId?: string
  menuWidth?: number
  strong?: boolean
  children: React.ReactNode
}) {
  return (
    <Select items={items} value={value} onValueChange={onChange}>
      <div style={{ position: 'relative', display: 'flex' }}>
        <SelectTrigger
          testId={testId}
          style={(state) => ({
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 5,
            height: M.row,
            paddingLeft: 7,
            paddingRight: caret ? 5 : 7,
            borderRadius: 7,
            cursor: 'pointer',
            backgroundColor: state.open ? C.chip : '#00000000',
            hover: { backgroundColor: C.chip },
          })}
        >
          {icon ? <Icon name={icon} size={12} color={strong ? C.secondary : C.tertiary} /> : null}
          <text
            style={{
              fontSize: 12.5,
              lineHeight: 16,
              color: strong ? C.text : C.secondary,
              whiteSpace: 'nowrap',
            }}
          >
            {label}
          </text>
          {caret ? <Icon name="chevronDown" size={11} color={C.faint} /> : null}
        </SelectTrigger>
        <SelectContent
          side="top"
          sideOffset={6}
          style={{ ...menuLayer(), minWidth: menuWidth ?? 190 }}
        >
          <MenuSurface>{children}</MenuSurface>
        </SelectContent>
      </div>
    </Select>
  )
}
