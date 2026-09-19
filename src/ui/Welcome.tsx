/**
 * The empty-thread card from the mock: what the agent is allowed to touch, and
 * where to type the task.
 */

import React from 'react'
import logoMark from '../../assets/logo-mark.svg' with { type: 'text' }
import { C } from '../theme'

export function Welcome() {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        flexGrow: 1,
        minHeight: 0,
        paddingTop: 26,
      }}
    >
      <div
        testId="welcome"
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          gap: 6,
          width: 432,
          paddingTop: 16,
          paddingBottom: 15,
          paddingLeft: 20,
          paddingRight: 20,
          backgroundColor: C.card,
          borderWidth: 1,
          borderColor: C.cardBorder,
          borderRadius: 10,
        }}
      >
        <svg
          source={logoMark}
          style={{ width: 30, height: 30, color: C.inverse, marginBottom: 2 }}
        />
        <text style={{ fontSize: 13, lineHeight: 18, fontWeight: 600, color: C.text }}>
          新会话
        </text>
        <text
          style={{
            fontSize: 12.5,
            lineHeight: 18,
            color: C.secondary,
            textAlign: 'center',
          }}
        >
          在下方输入任务目标：Agent 会工作区内执行，改动与命令需你批准
        </text>
        <text style={{ fontSize: 11.5, lineHeight: 16, color: C.accent }}>
          Agent 只能访问当前项目内的文件
        </text>
      </div>
    </div>
  )
}
