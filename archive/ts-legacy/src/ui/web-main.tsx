/**
 * Web / H5 端专属入口模块。
 *
 * 针对浏览器与移动端页面环境：
 * 1. 从当前 URL 或配置中提取远程 agent_core 的 WebSocket 服务地址；
 * 2. 建立标准的 JSON-RPC 2.0 客户端；
 * 3. 通过 react-dom 将整套 AgentWindow 挂载到网页 DOM 节点上。
 */

import React from 'react'
import { AgentWindow } from '../AgentWindow'
import { createWebSocketClient } from './client/ws'

export function mountWebApp(containerId = 'root') {
  if (typeof window === 'undefined' || typeof document === 'undefined') {
    return
  }

  const container = document.getElementById(containerId)
  if (!container) {
    console.error(`[web-main] 未找到 DOM 容器 #${containerId}`)
    return
  }

  const params = new URLSearchParams(window.location.search)
  const defaultWsHost = window.location.hostname || '127.0.0.1'
  const wsUrl = params.get('ws') || `ws://${defaultWsHost}:5678`
  const token = params.get('token') || ''

  // 创建双工 WebSocket RPC 客户端，无缝对接远程/宿主机上的 agent_core
  const client = createWebSocketClient({
    url: wsUrl,
    token,
  })

  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const { createRoot } = require('react-dom/client')
    const root = createRoot(container)
    root.render(<AgentWindow client={client} />)
  } catch (err) {
    console.error('[web-main] 挂载 React 网页应用失败:', err)
  }
}
