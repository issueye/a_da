import React from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import { registerDesktopShortcuts } from './client/desktop-bridge'
import './index.css'

// 桌面壳快捷键（F12 / Ctrl+Shift+I 打开开发者工具）；浏览器预览下是空操作
registerDesktopShortcuts()

const container = document.getElementById('root')
if (container) {
  const root = createRoot(container)
  root.render(
    <React.StrictMode>
      <App />
    </React.StrictMode>
  )
}
