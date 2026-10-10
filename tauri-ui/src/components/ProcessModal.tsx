import React, { useState, useEffect, useCallback } from 'react'
import {
  X,
  Activity,
  Cpu,
  Server,
  RefreshCw,
  Power,
  Terminal,
  CheckCircle2,
  AlertCircle,
  Radio,
  ExternalLink,
} from 'lucide-react'
import { agentClient, restartDesktopApp } from '../client/ws-client'

export interface ManagedProcessInfo {
  name: string
  role: string
  pid?: number | null
  port?: number | null
  alive: boolean
  status: string
  url?: string | null
}

export interface ProcessReport {
  mode: 'direct' | 'gateway'
  core_alive: boolean
  core_url: string
  core_port: number
  error?: string | null
  processes: ManagedProcessInfo[]
}

interface ProcessModalProps {
  isOpen: boolean
  onClose: () => void
}

export const ProcessModal: React.FC<ProcessModalProps> = ({ isOpen, onClose }) => {
  const [report, setReport] = useState<ProcessReport | null>(null)
  const [loading, setLoading] = useState(false)
  const [terminatingPid, setTerminatingPid] = useState<number | null>(null)
  const [message, setMessage] = useState<{ text: string; type: 'success' | 'error' | 'info' } | null>(null)

  const isTauri = typeof window !== 'undefined' && Boolean((window as any).__TAURI_INTERNALS__ || (window as any).__TAURI__)

  const fetchProcesses = useCallback(async () => {
    setLoading(true)
    try {
      if (isTauri) {
        const { invoke } = await import('@tauri-apps/api/core')
        const data = await invoke<ProcessReport>('get_process_report')
        setReport(data)
      } else {
        // 浏览器开发调试预览兜底
        setReport({
          mode: agentClient.desktopMode || 'direct',
          core_alive: agentClient.connected,
          core_url: 'ws://127.0.0.1:52353/rpc',
          core_port: 52353,
          error: null,
          processes: [
            {
              name: 'ada-coding',
              role: 'Coding / PM 核心引擎服务',
              pid: 18420,
              port: 52353,
              alive: true,
              status: 'running',
              url: 'ws://127.0.0.1:52353/rpc',
            },
          ],
        })
      }
    } catch (err: any) {
      console.error('[ProcessModal] 获取进程状态失败:', err)
      setMessage({ text: `获取进程状态失败: ${err?.message || String(err)}`, type: 'error' })
    } finally {
      setLoading(false)
    }
  }, [isTauri])

  useEffect(() => {
    if (isOpen) {
      setMessage(null)
      fetchProcesses()
      const interval = setInterval(fetchProcesses, 4000)
      return () => clearInterval(interval)
    }
  }, [isOpen, fetchProcesses])

  const handleKillProcess = async (pid: number, name: string) => {
    if (!window.confirm(`确定要终止进程 ${name} (PID: ${pid}) 吗？`)) {
      return
    }
    setTerminatingPid(pid)
    try {
      if (isTauri) {
        const { invoke } = await import('@tauri-apps/api/core')
        await invoke('kill_process', { pid })
        setMessage({ text: `已成功终止进程 ${name} (PID: ${pid})`, type: 'success' })
      } else {
        setMessage({ text: `浏览器模拟模式：已请求终止 PID ${pid}`, type: 'info' })
      }
      setTimeout(fetchProcesses, 600)
    } catch (err: any) {
      setMessage({ text: `终止进程失败: ${err?.message || String(err)}`, type: 'error' })
    } finally {
      setTerminatingPid(null)
    }
  }

  const handleRestartAll = async () => {
    if (!window.confirm('确定要重启所有桌面端核心服务并重新加载应用吗？')) {
      return
    }
    try {
      setMessage({ text: '正在重启应用与服务...', type: 'info' })
      await restartDesktopApp()
    } catch (err: any) {
      setMessage({ text: `重启失败: ${err?.message || String(err)}`, type: 'error' })
    }
  }

  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 bg-black/60 backdrop-blur-xs flex items-center justify-center p-4 animate-in fade-in duration-150">
      <div className="w-full max-w-2xl bg-white dark:bg-[#18181b] rounded-2xl border border-zinc-200 dark:border-zinc-800 shadow-2xl flex flex-col max-h-[85vh] overflow-hidden">
        {/* 标题栏 */}
        <div className="px-6 py-4 border-b border-zinc-200 dark:border-zinc-800 flex items-center justify-between bg-zinc-50/70 dark:bg-[#151518]/70">
          <div className="flex items-center space-x-2.5">
            <div className="w-8 h-8 rounded-lg bg-blue-500/10 dark:bg-blue-500/20 text-blue-600 dark:text-blue-400 flex items-center justify-center">
              <Activity size={18} />
            </div>
            <div>
              <div className="flex items-center space-x-2">
                <h3 className="text-sm font-bold text-zinc-900 dark:text-zinc-100">
                  进程管理与服务监控
                </h3>
                <span className="text-[10px] px-2 py-0.5 rounded-full font-medium bg-zinc-200/70 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300">
                  {report?.mode === 'gateway' ? '网关模式' : '直连模式'}
                </span>
              </div>
              <p className="text-[11px] text-zinc-500 dark:text-zinc-400">
                查看并管理 a_da 后端 Agent 与 Gateway 守护进程
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-1.5 text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200/60 dark:hover:bg-zinc-800 rounded-lg transition-colors cursor-pointer"
            title="关闭弹窗"
          >
            <X size={16} />
          </button>
        </div>

        {/* 状态总览横幅 */}
        <div className="px-6 py-3 bg-zinc-100/50 dark:bg-zinc-900/40 border-b border-zinc-200/80 dark:border-zinc-800/80 flex items-center justify-between text-xs">
          <div className="flex items-center space-x-3">
            <div className="flex items-center space-x-1.5">
              <span
                className={`w-2.5 h-2.5 rounded-full ${
                  report?.core_alive ? 'bg-emerald-500 animate-pulse' : 'bg-rose-500'
                }`}
              />
              <span className="font-medium text-zinc-700 dark:text-zinc-300">
                {report?.core_alive ? '核心链路在线' : '核心服务离线'}
              </span>
            </div>
            <span className="text-zinc-300 dark:text-zinc-700">|</span>
            <div className="text-zinc-500 dark:text-zinc-400 font-mono text-[11px] truncate max-w-[280px]">
              {report?.core_url || '未连接'}
            </div>
          </div>

          <div className="flex items-center space-x-2">
            <button
              onClick={fetchProcesses}
              disabled={loading}
              className="flex items-center space-x-1 px-2.5 py-1 text-xs rounded-md bg-white dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700 hover:bg-zinc-50 dark:hover:bg-zinc-700/80 text-zinc-700 dark:text-zinc-300 transition-colors cursor-pointer shadow-2xs"
              title="立即刷新进程状态"
            >
              <RefreshCw size={12} className={loading ? 'animate-spin text-blue-500' : ''} />
              <span>刷新</span>
            </button>
          </div>
        </div>

        {/* 操作消息提醒 */}
        {message && (
          <div
            className={`px-6 py-2 text-xs flex items-center justify-between border-b ${
              message.type === 'success'
                ? 'bg-emerald-50 dark:bg-emerald-950/20 text-emerald-700 dark:text-emerald-300 border-emerald-200 dark:border-emerald-900/30'
                : message.type === 'error'
                ? 'bg-rose-50 dark:bg-rose-950/20 text-rose-700 dark:text-rose-300 border-rose-200 dark:border-rose-900/30'
                : 'bg-blue-50 dark:bg-blue-950/20 text-blue-700 dark:text-blue-300 border-blue-200 dark:border-blue-900/30'
            }`}
          >
            <span>{message.text}</span>
            <button
              onClick={() => setMessage(null)}
              className="text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200 cursor-pointer"
            >
              <X size={12} />
            </button>
          </div>
        )}

        {/* 进程列表主体 */}
        <div className="p-6 overflow-y-auto flex-1 space-y-3">
          <div className="text-[11px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500 mb-2">
            受管进程实例 ({report?.processes.length || 0})
          </div>

          {(!report?.processes || report.processes.length === 0) && (
            <div className="p-8 text-center border-2 border-dashed border-zinc-200 dark:border-zinc-800 rounded-xl">
              <Terminal size={32} className="mx-auto text-zinc-300 dark:text-zinc-600 mb-2" />
              <p className="text-xs text-zinc-500 dark:text-zinc-400">
                当前暂无托管运行中的子进程
              </p>
            </div>
          )}

          {report?.processes.map((proc, index) => {
            const isGateway = proc.name.includes('gateway')
            const isPm = proc.name.includes('pm')
            const isCoding = proc.name.includes('coding')

            return (
              <div
                key={`${proc.name}-${proc.pid || index}`}
                className="p-4 rounded-xl border border-zinc-200 dark:border-zinc-800/90 bg-white dark:bg-[#18181b] hover:border-zinc-300 dark:hover:border-zinc-700 transition-all shadow-2xs flex items-center justify-between gap-4"
              >
                {/* 左侧：图标与基础信息 */}
                <div className="flex items-start space-x-3 min-w-0">
                  <div
                    className={`w-9 h-9 rounded-xl flex items-center justify-center flex-shrink-0 mt-0.5 ${
                      isGateway
                        ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
                        : isPm
                        ? 'bg-purple-500/10 text-purple-600 dark:text-purple-400'
                        : 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
                    }`}
                  >
                    {isGateway ? (
                      <Server size={18} />
                    ) : isPm ? (
                      <Cpu size={18} />
                    ) : (
                      <Terminal size={18} />
                    )}
                  </div>

                  <div className="min-w-0">
                    <div className="flex items-center space-x-2">
                      <span className="font-mono text-xs font-bold text-zinc-900 dark:text-zinc-100">
                        {proc.name}
                      </span>
                      <span
                        className={`text-[9px] px-1.5 py-0.2 rounded font-medium flex items-center space-x-1 ${
                          proc.alive
                            ? 'bg-emerald-100 dark:bg-emerald-950/60 text-emerald-700 dark:text-emerald-300 border border-emerald-200 dark:border-emerald-800/50'
                            : 'bg-rose-100 dark:bg-rose-950/60 text-rose-700 dark:text-rose-300 border border-rose-200 dark:border-rose-800/50'
                        }`}
                      >
                        <span
                          className={`w-1.5 h-1.5 rounded-full ${
                            proc.alive ? 'bg-emerald-500' : 'bg-rose-500'
                          }`}
                        />
                        <span>{proc.alive ? '运行中' : '已停止'}</span>
                      </span>
                    </div>

                    <div className="text-[11px] text-zinc-500 dark:text-zinc-400 mt-0.5">
                      {proc.role}
                    </div>

                    <div className="flex flex-wrap items-center gap-2 mt-2 font-mono text-[10px]">
                      {proc.pid ? (
                        <span className="px-1.5 py-0.5 rounded bg-zinc-100 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300">
                          PID: {proc.pid}
                        </span>
                      ) : (
                        <span className="px-1.5 py-0.5 rounded bg-zinc-100 dark:bg-zinc-800 text-zinc-400">
                          PID: --
                        </span>
                      )}

                      {proc.port ? (
                        <span className="px-1.5 py-0.5 rounded bg-zinc-100 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300">
                          端口: {proc.port}
                        </span>
                      ) : null}

                      {proc.url ? (
                        <span
                          className="px-1.5 py-0.5 rounded bg-zinc-100 dark:bg-zinc-800 text-zinc-500 truncate max-w-[200px]"
                          title={proc.url}
                        >
                          {proc.url}
                        </span>
                      ) : null}
                    </div>
                  </div>
                </div>

                {/* 右侧：管理操作 */}
                <div className="flex items-center space-x-2 flex-shrink-0">
                  {proc.pid && proc.alive ? (
                    <button
                      onClick={() => handleKillProcess(proc.pid!, proc.name)}
                      disabled={terminatingPid === proc.pid}
                      className="px-2.5 py-1 text-xs font-medium text-rose-600 dark:text-rose-400 hover:bg-rose-50 dark:hover:bg-rose-950/30 border border-rose-200 dark:border-rose-900/50 rounded-lg transition-colors cursor-pointer"
                      title="停止此进程"
                    >
                      {terminatingPid === proc.pid ? '正在停止...' : '终止进程'}
                    </button>
                  ) : (
                    <span className="text-[11px] text-zinc-400 italic">离线</span>
                  )}
                </div>
              </div>
            )
          })}
        </div>

        {/* 底部功能栏 */}
        <div className="px-6 py-4 border-t border-zinc-200 dark:border-zinc-800 bg-zinc-50/70 dark:bg-[#151518]/70 flex items-center justify-between">
          <div className="text-[11px] text-zinc-500 dark:text-zinc-400">
            遇到连接故障或状态异常时，可一键重启所有服务
          </div>

          <div className="flex items-center space-x-2.5">
            <button
              onClick={handleRestartAll}
              className="flex items-center space-x-1.5 px-3 py-1.5 text-xs font-medium bg-amber-600 hover:bg-amber-500 text-white rounded-lg transition-colors cursor-pointer shadow-xs"
              title="重启所有托管服务"
            >
              <Power size={13} />
              <span>重启所有服务</span>
            </button>
            <button
              onClick={onClose}
              className="px-3.5 py-1.5 text-xs font-medium bg-zinc-200 dark:bg-zinc-800 hover:bg-zinc-300 dark:hover:bg-zinc-700 text-zinc-700 dark:text-zinc-300 rounded-lg transition-colors cursor-pointer"
            >
              关闭
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
