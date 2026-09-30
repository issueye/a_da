/**
 * 命令面板 (CommandPalette, Ctrl+K)
 *
 * 全窗模态：输入过滤 + 上下键/回车/Esc 键盘导航（焦点给 <input>，按键在
 * 元素 onKeyDown 里分发——Select/Combobox 的既有模式）。条目来自
 * buildPaletteItems：全局快捷键 + 模式切换，面板本身就是一份可执行的
 * 快捷键帮助。
 */

import React, { useEffect, useState } from 'react'
import type { AgentClient } from './client'
import { C, FONT_MONO } from '../theme'
import { GLOBAL_SHORTCUTS } from './shortcuts'

export interface PaletteItem {
  id: string
  label: string
  hint?: string
  description?: string
  run: () => void
}

/** 面板条目：快捷键动作在前，协作模式切换在后。导出以便测试。 */
export function buildPaletteItems(client: AgentClient): PaletteItem[] {
  const items: PaletteItem[] = GLOBAL_SHORTCUTS.filter((shortcut) => shortcut.id !== 'escape').map(
    (shortcut) => ({
      id: `shortcut-${shortcut.id}`,
      label: shortcut.label,
      hint: shortcut.keys,
      description: shortcut.description,
      run: () => shortcut.run(client),
    })
  )

  const modeItems: PaletteItem[] = (
    [
      { value: 'code', label: '切换到 Code 模式', description: '敏捷编码，全量工具' },
      { value: 'plan', label: '切换到 Plan 模式', description: '只读分析与方案设计' },
      { value: 'create', label: '切换到 Create 模式', description: '元开发：管理工具与技能' },
    ] as const
  ).map((mode) => ({
    id: `mode-${mode.value}`,
    label: mode.label,
    description: client.state.mode === mode.value ? `${mode.description}（当前）` : mode.description,
    run: () => void client.request('thread.setMode', { mode: mode.value }),
  }))

  return [...items, ...modeItems]
}

function filterItems(items: PaletteItem[], query: string): PaletteItem[] {
  const q = query.trim().toLowerCase()
  if (!q) return items
  return items.filter(
    (item) => item.label.toLowerCase().includes(q) || (item.description ?? '').toLowerCase().includes(q)
  )
}

export function CommandPalette({ client }: { client: AgentClient }) {
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const items = filterItems(buildPaletteItems(client), query)
  const safeActive = Math.min(active, Math.max(0, items.length - 1))

  useEffect(() => {
    setActive(0)
  }, [query])

  const runItem = (item: PaletteItem | undefined): void => {
    if (!item) return
    client.ui.setPaletteOpen(false)
    item.run()
  }

  return (
    <div
      testId="command-palette-overlay"
      style={{
        position: 'absolute',
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        paddingTop: 80,
        backgroundColor: C.scrim,
        // 与 SettingsDialog 同理：模态要吞掉后面的点击与滚轮
        pointerEvents: 'auto',
      }}
    >
      <div
        testId="command-palette"
        style={{
          display: 'flex',
          flexDirection: 'column',
          width: 520,
          maxWidth: 620,
          maxHeight: 480,
          backgroundColor: C.raised,
          borderWidth: 1,
          borderColor: C.borderStrong,
          borderRadius: 12,
          overflow: 'hidden',
          boxShadow: { offsetX: 0, offsetY: 8, blurRadius: 28, spreadRadius: 0, color: C.shadow },
        }}
      >
        <input
          testId="command-palette-input"
          autoFocus
          value={query}
          placeholder="输入命令或搜索…（Esc 关闭）"
          style={{
            fontSize: 13.5,
            lineHeight: 20,
            color: C.text,
            backgroundColor: C.raised,
            borderWidth: 0,
            paddingLeft: 14,
            paddingRight: 14,
            paddingTop: 12,
            paddingBottom: 10,
          }}
          onChange={(event: any) => setQuery(String(event?.value ?? ''))}
          onSubmit={() => runItem(items[safeActive])}
          onKeyDown={(event: any) => {
            const key = String(event?.key ?? '').toLowerCase()
            if (key === 'escape') {
              client.ui.setPaletteOpen(false)
            } else if (key === 'arrowdown' || key === 'down' || (key === 'n' && event?.modifiers?.ctrl)) {
              setActive((idx) => Math.min(idx + 1, items.length - 1))
            } else if (key === 'arrowup' || key === 'up' || (key === 'p' && event?.modifiers?.ctrl)) {
              setActive((idx) => Math.max(idx - 1, 0))
            }
          }}
        />

        <div
          testId="command-palette-list"
          style={{
            display: 'flex',
            flexDirection: 'column',
            borderTopWidth: 1,
            borderColor: C.cardBorder,
            maxHeight: 380,
            overflowY: 'scroll',
            paddingTop: 4,
            paddingBottom: 6,
          }}
        >
          {items.length === 0 ? (
            <text style={{ fontSize: 12.5, color: C.faint, padding: 12 }}>没有匹配的命令</text>
          ) : null}
          {items.map((item, idx) => {
            const isActive = idx === safeActive
            return (
              <div
                key={item.id}
                testId={`palette-item-${item.id}`}
                role="button"
                onClick={() => runItem(item)}
                onMouseEnter={() => setActive(idx)}
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 8,
                  paddingTop: 7,
                  paddingBottom: 7,
                  paddingLeft: 14,
                  paddingRight: 14,
                  backgroundColor: isActive ? C.overlay : undefined,
                  cursor: 'pointer',
                }}
              >
                <div style={{ display: 'flex', flexDirection: 'column', flexShrink: 1, minWidth: 0 }}>
                  <text
                    style={{
                      fontSize: 13,
                      lineHeight: 18,
                      color: isActive ? C.text : C.secondary,
                      whiteSpace: 'nowrap',
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                    }}
                  >
                    {item.label}
                  </text>
                  {item.description ? (
                    <text
                      style={{
                        fontSize: 11,
                        lineHeight: 15,
                        color: C.faint,
                        whiteSpace: 'nowrap',
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                      }}
                    >
                      {item.description}
                    </text>
                  ) : null}
                </div>
                <div style={{ flexGrow: 1 }} />
                {item.hint ? (
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      height: 18,
                      paddingLeft: 6,
                      paddingRight: 6,
                      borderRadius: 4,
                      backgroundColor: C.overlay,
                      flexShrink: 0,
                    }}
                  >
                    <text style={{ fontSize: 10.5, fontFamily: FONT_MONO, color: C.tertiary }}>{item.hint}</text>
                  </div>
                ) : null}
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
}
