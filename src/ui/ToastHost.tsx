/**
 * 轻提示浮层 (ToastHost)。
 *
 * ## 它解决什么
 *
 * 在此之前，"停用插件""压缩上下文""恢复文件"这类动作做完是**没有反馈**的：
 * 它们只写 `debug.trace`，而调试日志默认不在屏幕上（要开调试面板才看得到）。
 * 用户点一下开关，除了开关自己的颜色变了一下，没有任何"这件事真的成了"的确认。
 *
 * ## 为什么挂在窗口根上
 *
 * 和确认框、文件选择器同一个理由：它是**窗口级**浮层。挂在插件弹窗内部的话，
 * 一关弹窗提示就跟着没了——而"停用插件"恰恰就是关弹窗之前那一下。
 *
 * ## 几条刻意的取舍
 *
 * - **不做进出动画**：仓库里没有任何动画原语（没有 transition/keyframes），
 *   为提示引一套动画得不偿失；而且 GPU 渲染层加动画要自己管帧，风险不划算。
 * - **错误不自动消失**：`durationMs` 由 store 决定，error 默认 0（永不自动关）。
 *   自动消失的错误等于没提示——用户低头看一眼键盘就错过了。
 * - **容器 `pointerEvents: 'none'`**：右下角浮层是铺开的，不穿透就会挡住底下
 *   输入框与"停止"按钮的点击（浮层的经典事故）。只有卡片自己收点击。
 */

import React, { useEffect, useState } from 'react'
import type { AgentClient, ToastItem, ToastLevel } from './client'
import { C } from '../theme'
import { Icon } from './controls'
import type { IconName } from '../icons'

/** 每个级别一套配色与图标。颜色取自 theme，不另造一套硬编码。 */
const LEVEL_STYLE: Record<ToastLevel, { icon: IconName; color: string; border: string }> = {
  success: { icon: 'circleCheck', color: C.success, border: '#10b98140' },
  warn: { icon: 'alertTriangle', color: '#b45309', border: '#f59e0b40' },
  error: { icon: 'alertTriangle', color: C.danger, border: '#ef444440' },
}

function ToastCard({ toast, onDismiss }: { toast: ToastItem; onDismiss: () => void }) {
  const tone = LEVEL_STYLE[toast.level]
  return (
    <div
      testId={`toast-${toast.level}`}
      style={{
        display: 'flex',
        flexDirection: 'row',
        alignItems: 'flex-start',
        gap: 8,
        width: 340,
        paddingTop: 10,
        paddingBottom: 10,
        paddingLeft: 12,
        paddingRight: 8,
        borderRadius: 9,
        backgroundColor: C.raised,
        borderWidth: 1,
        borderColor: tone.border,
        boxShadow: {
          offsetX: 0,
          offsetY: 6,
          blurRadius: 20,
          spreadRadius: 0,
          color: C.shadowStrong,
        },
        // 卡片自己收点击；外层容器是穿透的（见文件头）
        pointerEvents: 'auto',
      }}
    >
      <div style={{ paddingTop: 1, flexShrink: 0 }}>
        <Icon name={tone.icon} size={13} color={tone.color} />
      </div>

      <div style={{ display: 'flex', flexDirection: 'column', gap: 2, flexGrow: 1, minWidth: 0 }}>
        <text
          testId="toast-message"
          style={{ fontSize: 12, lineHeight: 17, color: C.text, whiteSpace: 'normal' }}
        >
          {toast.message}
        </text>
        {toast.detail ? (
          <text style={{ fontSize: 11, lineHeight: 15, color: C.secondary, whiteSpace: 'normal' }}>
            {toast.detail}
          </text>
        ) : null}
      </div>

      <div
        testId="toast-dismiss"
        role="button"
        aria-label="关闭提示"
        onClick={onDismiss}
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          width: 18,
          height: 18,
          borderRadius: 4,
          cursor: 'pointer',
          flexShrink: 0,
          hover: { backgroundColor: C.chipHover },
        }}
      >
        <Icon name="close" size={10} color={C.faint} />
      </div>
    </div>
  )
}

export function ToastHost({ client }: { client: AgentClient }) {
  // 自己订阅，而不是指望父组件（AgentWindow）带着一起重渲染。
  // 之前是那样写的：它只读 `client.state.toasts`，挂在 AgentWindow 里确实会刷新——
  // 但那是**蹭**了 AgentWindow 的订阅。一旦有人把提示层单独渲染出去用（比如塞进
  // 别的壳、或单测里直接挂它），它就完全不响应了，而且没有任何报错：提示只是不出现。
  // 这种"看起来能跑、换个位置就静默失效"的依赖，正是本仓库 §15 说的那类坑。
  const [, setTick] = useState(0)
  useEffect(() => client.subscribe(() => setTick((tick) => tick + 1)), [client])

  // `?? []` 不是多余的：live 调试模式（`A_DA_CLIENT_VIEW=live`）下 client.state 是
  // `Object.create(store)`，读的是 AgentStore 的属性，而 store 上**没有** toasts 字段
  // ——那里读到 undefined，直接 `.length` 会把整窗带崩。
  const toasts = client.state.toasts ?? []
  if (toasts.length === 0) return null

  return (
    <div
      testId="toast-host"
      style={{
        position: 'absolute',
        right: 20,
        bottom: 20,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'flex-end',
        gap: 8,
        // 容器穿透，只有卡片收点击
        pointerEvents: 'none',
      }}
    >
      {toasts.map((toast) => (
        <ToastCard key={toast.id} toast={toast} onDismiss={() => client.ui.dismissToast(toast.id)} />
      ))}
    </div>
  )
}
