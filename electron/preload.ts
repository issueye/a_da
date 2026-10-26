import { contextBridge, ipcRenderer } from 'electron'
import type { DesktopConfig } from './types.js'

contextBridge.exposeInMainWorld('electronAPI', {
  getCoreInfo: () => ipcRenderer.invoke('desktop:get-core-info'),
  getDesktopConfig: () => ipcRenderer.invoke('desktop:get-config'),
  setDesktopConfig: (cfg: DesktopConfig) => ipcRenderer.invoke('desktop:set-config', cfg),
  restartDesktopApp: () => ipcRenderer.invoke('desktop:restart'),
  restartGateway: () => ipcRenderer.invoke('desktop:restart-gateway'),
  getProcessReport: () => ipcRenderer.invoke('desktop:get-process-report'),
  killProcess: (pid: number) => ipcRenderer.invoke('desktop:kill-process', pid),
  windowMinimize: () => ipcRenderer.invoke('desktop:window-minimize'),
  windowMaximize: () => ipcRenderer.invoke('desktop:window-maximize'),
  windowClose: () => ipcRenderer.invoke('desktop:window-close'),
  isWindowMaximized: () => ipcRenderer.invoke('desktop:window-is-maximized'),
  onWindowMaximizeChange: (callback: (isMax: boolean) => void) => {
    const handler = (_: any, val: boolean) => callback(val)
    ipcRenderer.on('window:maximize-change', handler)
    return () => {
      ipcRenderer.removeListener('window:maximize-change', handler)
    }
  },
})

// ── 无边框窗口控制：给 SSR 页面顶栏注入最小化/最大化/关闭按钮 ─────────────
//
// SSR 页面是给浏览器用的，没有 Electron 的自绘标题栏。主窗口保持 `frame: false`，
// 这里把 `.ada-topbar` 变成拖拽区，并在右侧动作区追加三个按钮；按钮走宿主 IPC。
// 2026-10：React 桌面前端（`desktop-ui/`）已移除，界面只有 SSR 一种。

const ICON_MINIMIZE =
  '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M5 12h14"/></svg>'
const ICON_MAXIMIZE =
  '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" aria-hidden="true"><rect x="5" y="5" width="14" height="14" rx="1.5"/></svg>'
const ICON_RESTORE =
  '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" aria-hidden="true"><rect x="4" y="8" width="12" height="12" rx="1.5"/><path d="M8 8V6a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2h-2"/></svg>'
const ICON_CLOSE =
  '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"/></svg>'

const CONTROLS_CSS = `
  /* 标题栏是窗口 chrome：必须压在所有应用模态层（ada-modal z-index: 90）之上，
     否则弹窗打开时最小化/最大化/关闭点不动。 */
  .ada-topbar { position: relative; z-index: 100; -webkit-app-region: drag; app-region: drag; }
  .ada-topbar button, .ada-topbar a, .ada-topbar summary, .ada-topbar input,
  .ada-topbar select, .ada-topbar textarea, .ada-topbar [role="button"],
  .ada-electron-controls {
    -webkit-app-region: no-drag; app-region: no-drag;
  }
  .ada-electron-controls {
    display: flex; align-items: center; gap: 2px; margin-left: 6px;
    padding-left: 8px; border-left: 1px solid var(--ph-line, #3f3f46);
  }
  .ada-electron-controls button {
    width: 30px; height: 30px; display: grid; place-items: center;
    border: 0; border-radius: 6px; background: transparent;
    color: var(--ph-muted, #a1a1aa); cursor: pointer; padding: 0;
  }
  .ada-electron-controls button:hover {
    background: var(--ph-hover, rgba(161, 161, 170, 0.16));
    color: var(--ph-text, #e4e4e7);
  }
  .ada-electron-controls button[data-action="close"]:hover {
    background: #e11d48; color: #fff;
  }
`

function installSsrWindowControls(): void {
  if (document.getElementById('ada-electron-window-controls')) return
  const topbar = document.querySelector('.ada-topbar')
  if (!topbar) return
  const actions = topbar.querySelector('.ada-topbar__actions') || topbar

  if (!document.getElementById('ada-electron-window-controls-style')) {
    const style = document.createElement('style')
    style.id = 'ada-electron-window-controls-style'
    style.textContent = CONTROLS_CSS
    document.head?.appendChild(style)
  }

  const controls = document.createElement('div')
  controls.id = 'ada-electron-window-controls'
  controls.className = 'ada-electron-controls'

  const button = (
    action: string,
    label: string,
    title: string,
    icon: string,
    onClick: () => void
  ) => {
    const el = document.createElement('button')
    el.type = 'button'
    el.dataset.action = action
    el.setAttribute('aria-label', label)
    el.setAttribute('title', title)
    el.innerHTML = icon
    el.addEventListener('click', onClick)
    return el
  }

  const minimize = button('minimize', '最小化', '最小化', ICON_MINIMIZE, () => {
    void ipcRenderer.invoke('desktop:window-minimize')
  })
  const maximize = button('maximize', '最大化', '最大化', ICON_MAXIMIZE, () => {
    void ipcRenderer.invoke('desktop:window-maximize')
    window.setTimeout(refreshMaximized, 80)
  })
  const close = button('close', '关闭窗口', '关闭窗口', ICON_CLOSE, () => {
    void ipcRenderer.invoke('desktop:window-close')
  })
  controls.append(minimize, maximize, close)
  actions.appendChild(controls)

  function applyMaximized(isMax: boolean): void {
    maximize.innerHTML = isMax ? ICON_RESTORE : ICON_MAXIMIZE
    maximize.setAttribute('aria-label', isMax ? '向下还原' : '最大化')
    maximize.setAttribute('title', isMax ? '向下还原' : '最大化')
  }
  function refreshMaximized(): void {
    void ipcRenderer.invoke('desktop:window-is-maximized').then(applyMaximized)
  }
  ipcRenderer.on('window:maximize-change', (_event, isMax: boolean) => {
    applyMaximized(isMax === true)
  })
  refreshMaximized()
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', installSsrWindowControls, { once: true })
} else {
  installSsrWindowControls()
}
// 整页表单提交后会换文档；preload 会重新执行，再兜一次 pageshow。
window.addEventListener('pageshow', installSsrWindowControls)
