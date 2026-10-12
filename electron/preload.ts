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
