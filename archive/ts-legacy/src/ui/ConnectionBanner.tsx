/**
 * 与主机的连接状态提示（协议 §1.5：断开必须"可理解 + 可重连"）。
 *
 * 为什么需要它：M3 起打包形态是两个进程（UI + 自己 spawn 的主机）。主机被杀、端口被占、
 * 或握手失败时，界面原先**只会静默停在最后一帧**——用户不知道发生了什么，也不知道该等还是该重启。
 * 这条横幅就是把那件事说出来，并且说清"正在自动重连、第几次"。
 *
 * 只在**非 connected** 时出现；进程内传输永远是 connected，所以开发与测试里它不占地方。
 */

import React from 'react'
import type { AgentClient } from './client'
import { C, M } from '../theme'

export function ConnectionBanner({ client }: { client: AgentClient }) {
  const connection = client.state.connection
  if (connection.status === 'connected') return null

  const reconnecting = connection.status === 'connecting' || connection.attempts > 0
  const headline = connection.status === 'connecting' ? '正在连接主机…' : '与主机的连接已断开'
  const detail = reconnecting
    ? `${connection.reason ? `${connection.reason}；` : ''}正在自动重连（第 ${Math.max(1, connection.attempts)} 次）`
    : connection.reason

  return (
    <div
      testId="connection-banner"
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
        height: 26,
        flexShrink: 0,
        paddingLeft: M.contentPadding,
        paddingRight: M.contentPadding,
        backgroundColor: C.raised,
        borderBottomWidth: 1,
        borderColor: C.borderStrong,
      }}
    >
      <div
        style={{
          width: 7,
          height: 7,
          borderRadius: 4,
          backgroundColor: connection.status === 'connecting' ? C.tertiary : C.accent,
        }}
      />
      <text style={{ fontSize: 11.5, fontWeight: 600, color: C.text }}>{headline}</text>
      {detail ? (
        <text style={{ fontSize: 11, color: C.secondary, flexShrink: 1 }}>{detail}</text>
      ) : null}
    </div>
  )
}
