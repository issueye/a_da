/**
 * 新建会话居中视图（空会话时展示）
 *
 * 输入框上下垂直居中，正上方配备工作区选择器，对齐设计参考。
 */

import React from 'react'
import type { AgentStore } from '../agent/store'
import { Composer } from './Composer'
import { WorkspaceSelector } from './WorkspaceSelector'
import { C, M } from '../theme'

export function EmptyConversationView({ store }: { store: AgentStore }) {
  return (
    <div
      testId="welcome"
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        flexGrow: 1,
        minHeight: 0,
        width: '100%',
        paddingLeft: M.contentPadding,
        paddingRight: M.contentPadding,
        paddingBottom: 60,
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          width: '100%',
          maxWidth: M.composerMax,
        }}
      >
        {/* 输入框上方的选择工作区功能（对齐参考图 2） */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            marginBottom: 12,
            paddingLeft: 2,
          }}
        >
          <WorkspaceSelector store={store} />
          <text style={{ fontSize: 11, color: C.faint, marginLeft: 10 }}>
            在下方输入任务目标：Agent 会工作区内执行，改动与命令需你批准。Agent 只能访问当前项目内的文件
          </text>
        </div>

        {/* 居中的会话输入框 */}
        <Composer store={store} centered />
      </div>
    </div>
  )
}
