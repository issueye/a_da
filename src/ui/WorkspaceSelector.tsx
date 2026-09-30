/**
 * 工作区快速选择器（用于居中新建对话界面输入框上方）
 *
 * 展示当前会话绑定的工作区，点击弹出下拉浮层，可切换至其它工作区、打开 a-da 的
 * 公共区，或快捷添加新工作区。
 */

import React from 'react'
import { Select, SelectTrigger, SelectContent, SelectItem, useGpuix } from '@gpuix/react'
import { Icon, MenuSurface, menuItemStyle, MenuRow, menuLayer } from './controls'
import type { AgentClient } from './client'
import { C } from '../theme'
import { PUBLIC_WORKSPACE_LABEL } from '../agent/home'
import { pickDirectory } from '../platform/dialog'

/** 「公共区」在下拉里的哨兵值：它是 a-da 提供的工作区，不在 projects 里。 */
const PUBLIC_OPTION = '__public_workspace__'

export function WorkspaceSelector({ client }: { client: AgentClient }) {
  const { renderer } = useGpuix()
  const current = client.state.active.workspace
  const label = client.state.labelFor(current)
  // 公共区已作为专门的一项固定在顶部，就不再从 projects 里重复列一遍。
  const otherProjects = client.state.projects.filter((p) => !client.state.isPublic(p))
  const items = [
    { value: PUBLIC_OPTION, label: PUBLIC_WORKSPACE_LABEL },
    ...otherProjects.map((p) => ({ value: p, label: client.state.labelFor(p) })),
    { value: '__add_new__', label: '+ 添加工作区...' },
  ]

  const handleChange = (selected: string) => {
    if (selected === PUBLIC_OPTION) {
      // 目录由 a-da 提供（不存在则建），当前会话直接绑过去。
      void client.request('workspace.openPublic', { threadId: client.state.active.id })
      return
    }
    if (selected === '__add_new__') {
      void pickDirectory(current, renderer).then((result) => {
        if (result.status === 'picked') {
          void client.request('workspace.add', { path: result.path }).then(({ error }) => {
            if (!error) {
              void client.request('thread.setWorkspace', {
                threadId: client.state.active.id,
                workspace: result.path,
              })
            }
          })
        }
      })
      return
    }
    void client.request('thread.setWorkspace', {
      threadId: client.state.active.id,
      workspace: selected,
    })
  }

  return (
    <Select
      items={items}
      value={client.state.isPublic(current) ? PUBLIC_OPTION : current}
      onValueChange={handleChange}
    >
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
            <SelectItem
              testId="select-workspace-public"
              value={PUBLIC_OPTION}
              style={menuItemStyle}
            >
              <MenuRow
                label={PUBLIC_WORKSPACE_LABEL}
                description="a-da 自带的工作区，不绑定任何项目"
                selected={client.state.isPublic(current)}
              />
            </SelectItem>
            {otherProjects.map((p) => (
              <SelectItem
                key={p}
                testId={`select-workspace-${client.state.labelFor(p)}`}
                value={p}
                style={menuItemStyle}
              >
                <MenuRow
                  label={client.state.labelFor(p)}
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
