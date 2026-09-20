/**
 * a_da — 具备原生 GPU 加速窗口的本地 AI 编程助手。
 *
 * 开发运行：`bun --hot app.tsx`
 * 二进制构建：`bun run build`
 */

// 1. 最优先执行底层平台引导，确保 stdio 句柄安全指向虚拟设备，
// 彻底解决 Rust 原生层向 stderr 写日志因空句柄导致 panic (os error 6) 闪退的问题。
import './src/platform/init'

import { appendFileSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'
import React from 'react'
import { render } from '@gpuix/react'
import { AgentWindow } from './src/AgentWindow'
import { getAppHome } from './src/agent/home'
import { activateAndShowWindow, isUserInitiatedExit } from './src/platform/win32'

// 日志记录：输出到应用数据目录，方便无控制台模式下追踪问题
const logDir = getAppHome()
try { mkdirSync(logDir, { recursive: true }) } catch {}
const logFile = join(logDir, 'app_debug.log')
function log(msg: string) {
  try { appendFileSync(logFile, `[${new Date().toISOString()}] ${msg}\n`) } catch {}
}

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
}) as any

process.on('uncaughtException', (err) => {
  log(`[uncaughtException] ${err?.stack || err}`)
})
process.on('unhandledRejection', (err: any) => {
  log(`[unhandledRejection] ${err?.stack || err}`)
})

try {
  log('开始调用 render() 挂载界面...')
  render(<AgentWindow />, {
    title: 'a_da',
    width: 1120,
    height: 760,
    titlebarTransparent: true,
    windowBackground: 'opaque',
    trafficLightX: 16,
    trafficLightY: 17,
    focus: process.env.GPUIX_BACKGROUND !== '1',
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
    } catch (e: any) {
      log(`第 ${attempt} 次前台置顶激活异常: ${e?.message || e}`)
    }
  }
  setTimeout(() => tryActivate(1), 100)
} catch (e: any) {
  log(`render() 异常: ${e?.stack || e}`)
}

// 保持事件循环活跃：防止打包为独立二进制后 JS 主线程因无待办异步任务而过早退出
setInterval(() => {
  // 维持心跳
}, 30_000)
