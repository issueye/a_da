/**
 * 工作区快速选择器（用于居中新建对话界面输入框上方）
 *
 * 展示当前会话绑定的工作区，点击弹出下拉浮层，可切换至其它工作区或快捷添加新工作区。
 */

import React from 'react'
import { Select, SelectTrigger, SelectContent, SelectItem, useGpuix } from '@gpuix/react'
import { Icon, MenuSurface, menuItemStyle, MenuRow, menuLayer } from './controls'
import type { AgentStore } from '../agent/store'
import { C, shortPath } from '../theme'
import { pickDirectory } from '../platform/dialog'

export function WorkspaceSelector({ store }: { store: AgentStore }) {
  const { renderer } = useGpuix()
  const current = store.active.workspace
  const label = shortPath(current, 2)
  const items = [
    ...store.projects.map((p) => ({ value: p, label: shortPath(p, 2) })),
    { value: '__add_new__', label: '+ 添加工作区...' },
  ]

  const handleChange = (selected: string) => {
    if (selected === '__add_new__') {
      void pickDirectory(current, renderer).then((result) => {
        if (result.status === 'picked') {
          void store.addProject(result.path).then((err) => {
            if (!err) {
              store.setThreadWorkspace(store.active.id, result.path)
            }
          })
        }
      })
      return
    }
    store.setThreadWorkspace(store.active.id, selected)
  }

  return (
    <Select items={items} value={current} onValueChange={handleChange}>
      <div style={{ position: 'relative', display: 'flex' }}>
        <SelectTrigger
          testId="workspace-selector-trigger"
          style={(state) => ({
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 6,
            height: 30,
            paddingLeft: 9,
            paddingRight: 8,
            borderRadius: 7,
            cursor: 'pointer',
            backgroundColor: state.open ? C.chip : C.card,
            borderWidth: 1,
            borderColor: state.open ? C.link : C.borderStrong,
            hover: { backgroundColor: C.overlay, borderColor: C.link },
          })}
        >
          <Icon name="folder" size={13} color={C.link} />
          <text
            style={{
              fontSize: 12.5,
              fontWeight: 600,
              color: C.text,
              whiteSpace: 'nowrap',
            }}
          >
            {label}
          </text>
          <Icon name="chevronDown" size={10} color={C.secondary} />
        </SelectTrigger>
        <SelectContent
          side="bottom"
          sideOffset={6}
          style={{ ...menuLayer(), minWidth: 260 }}
        >
          <MenuSurface maxHeight={280}>
            {store.projects.map((p) => (
              <SelectItem
                key={p}
                testId={`select-workspace-${shortPath(p, 2)}`}
                value={p}
                style={menuItemStyle}
              >
                <MenuRow
                  label={shortPath(p, 2)}
                  description={p}
                  selected={p === current}
                />
              </SelectItem>
            ))}
            <div
              style={{
                height: 1,
                backgroundColor: C.border,
                marginTop: 3,
                marginBottom: 3,
              }}
            />
            <SelectItem
              testId="select-workspace-add-new"
              value="__add_new__"
              style={menuItemStyle}
            >
              <MenuRow
                label="+ 添加工作区..."
                description="选择本地文件夹并绑定"
                selected={false}
              />
            </SelectItem>
          </MenuSurface>
        </SelectContent>
      </div>
    </Select>
  )
}
