/**
 * UI 角色的启动代码（协议 §1.8 的另一半）。
 *
 * 从 `app.tsx` 原样搬来——**顺序不能动**：平台引导必须最先执行（它把 stdio 句柄指向虚拟设备，
 * 否则 Rust 原生层往 stderr 写日志会 panic）。`app.tsx` 只在**非 `--host`** 分支里动态 import
 * 这个模块，所以主机角色不会碰到渲染层（协议 §1.8 第 4 条）。
 */

// 1. 最优先执行底层平台引导，确保 stdio 句柄安全指向虚拟设备，
// 彻底解决 Rust 原生层向 stderr 写日志因空句柄导致 panic (os error 6) 闪退的问题。
import '../platform/init'

import React from 'react'
import { render } from '@gpuix/react'
import { AgentWindow } from '../AgentWindow'
import { resolveAgentClient } from './client'
import { log } from './client/logging'
import { activateAndShowWindow, isUserInitiatedExit } from '../platform/win32'
import { handleGlobalShortcut } from './shortcuts'

log(`=== a_da 启动 (pid=${process.pid}, platform=${process.platform}, cwd=${process.cwd()}) ===`)

// 进程退出防护：仅允许由用户显式点击关闭按钮触发的真实退出，
// 拦截 GPUI 帧循环中偶发的误判 onTerminated 退出，保证窗口长效驻留。
const origExit = process.exit.bind(process)
process.exit = ((code?: number) => {
  if (isUserInitiatedExit()) {
    log(`[process.exit] 用户触发正常退出 (code=${code})`)
    return origExit(code)
  }
  log(`[process.exit] 拦截意外退出调用 (code=${code})，保留应用运行:\n${new Error().stack}`)
}) as never

process.on('uncaughtException', (err) => {
  log(`[uncaughtException] ${err?.stack || err}`)
})
process.on('unhandledRejection', (err: unknown) => {
  log(`[unhandledRejection] ${(err as Error)?.stack || err}`)
})

try {
  // 选传输（M3-6）：开发/测试走进程内，打包形态走 WebSocket + 自 spawn 主机。
  // 这一步必须在 render 之前完成——界面起来时客户端就得是可用的。
  const { client, shutdown, info } = await resolveAgentClient()
  log(
    `传输：${info.transport}` +
      (info.port ? `（主机 pid=${info.pid} 端口=${info.port}）` : '（进程内）')
  )
  // 主机随 UI 退出：正常退出路径（process.exit / 信号）都收掉它，避免孤儿进程
  process.on('exit', shutdown)

  log('开始调用 render() 挂载界面...')
  render(<AgentWindow client={client} />, {
    title: 'a_da',
    width: 1370,
    height: 950,
    titlebarTransparent: true,
    windowBackground: 'opaque',
    trafficLightX: 16,
    trafficLightY: 17,
    focus: process.env.GPUIX_BACKGROUND !== '1',
    // 窗口级键盘：全局快捷键（Ctrl+K 命令面板等）。聚焦元素没消费的组合键
    // 会在冒泡相落到这里。
    onKeyDown: (event: unknown) => {
      handleGlobalShortcut(event as Parameters<typeof handleGlobalShortcut>[0], client)
    },
  })
  log('render() 初始化执行成功')

  // 挂载完成后，采用多阶梯度激活策略穿透桌面层级，确保窗口即刻在用户屏幕前台弹出
  const tryActivate = (attempt = 1) => {
    try {
      const ok = activateAndShowWindow()
      log(`第 ${attempt} 次前台置顶激活: ${ok}`)
      if (!ok && attempt < 5) {
        setTimeout(() => tryActivate(attempt + 1), 200)
      }
    } catch (e) {
      log(`第 ${attempt} 次前台置顶激活异常: ${(e as Error)?.message || e}`)
    }
  }
  setTimeout(() => tryActivate(1), 100)
} catch (e) {
  log(`render() 异常: ${(e as Error)?.stack || e}`)
}

// 保持事件循环活跃：防止打包为独立二进制后 JS 主线程因无待办异步任务而过早退出
setInterval(() => {
  // 维持心跳
}, 30_000)
