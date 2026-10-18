import React, { useState, useEffect, useCallback } from 'react'
import {
  X,
  Activity,
  Cpu,
  Server,
  RefreshCw,
  Power,
  Terminal,
  Radio,
  Info,
  Copy,
  Check,
} from 'lucide-react'
import { agentClient } from '../client/ws-client'
import {
  getProcessReport,
  killProcess,
  restartDesktopApp,
  restartGateway,
  isDesktop,
  type ProcessReport,
  type ManagedProcessInfo,
} from '../client/desktop-bridge'

export type { ManagedProcessInfo, ProcessReport }

interface ProcessModalProps {
  isOpen: boolean
  onClose: () => void
  /**
   * 统一的操作确认弹窗（App 里的 ConfirmModal）。
   *
   * 不用 `window.confirm`：那是 WebView 的原生弹窗，
   * 与应用风格不一致、也无法本地化按钮文案。
   */
  onRequestConfirm?: (opts: {
    title?: string
    message: string
    subMessage?: string
    confirmText?: string
    variant?: 'danger' | 'warning' | 'primary'
  }) => Promise<boolean>
}

/**
 * 把网关实例注册表的条目映射成进程行。
 *
 * 网关模式下 agent 实例是**网关**拉起的，桌面端只记录了自己直接 spawn 的
 * `ada-gateway`——所以任务管理器里明明跑着 `ada-coding.exe`，面板里却只有网关。
 * 事实的所有者是网关（`registry.rs`：进程/端口/健康状态归网关），所以这里**问网关要**，
 * 而不是在桌面端另猜一份。
 */
function mapGatewayAgents(res: any): ManagedProcessInfo[] {
  const agents = Array.isArray(res?.agents) ? res.agents : []
  return agents.map((a: any) => {
    const endpoint = typeof a.endpoint === 'string' ? a.endpoint : ''
    const portMatch = endpoint.match(/:(\d+)\//)
    const status = String(a.status || '')
    const statusSuffix =
      status === 'ready'
        ? ''
        : status === 'starting'
        ? ' · 启动中'
        : status === 'unhealthy'
        ? ' · 健康检查失败'
        : status === 'stopped'
        ? ' · 已退出'
        : status
        ? ` · ${status}`
        : ''
    return {
      name: String(a.product || a.id || 'agent'),
      role: `网关托管实例 · ${a.workspace || '未标工作区'}${statusSuffix}`,
      pid: typeof a.pid === 'number' ? a.pid : null,
      port: portMatch ? Number(portMatch[1]) : null,
      // 网关的判据：只有 stopped 是"进程已退出"（unhealthy = 进程还在但健康检查失败）
      alive: status !== 'stopped',
      status,
      url: endpoint || null,
      products: Array.isArray(a.products) ? a.products.map((p: any) => String(p)) : [],
    }
  })
}

/** 进程状态徽章：按 `status` 分档，而不是只有"活的/死的"两种颜色。 */
const StatusPill: React.FC<{ alive: boolean; status?: string }> = ({ alive, status }) => {
  const tone =
    !alive || status === 'stopped'
      ? {
          label: '已停止',
          cls: 'bg-zinc-100 dark:bg-zinc-800 text-zinc-500 dark:text-zinc-400 border-zinc-200 dark:border-zinc-700',
          dot: 'bg-zinc-400',
        }
      : status === 'unhealthy'
      ? {
          label: '健康检查失败',
          cls: 'bg-amber-50 dark:bg-amber-950/40 text-amber-700 dark:text-amber-300 border-amber-200 dark:border-amber-900/50',
          dot: 'bg-amber-500',
        }
      : status === 'starting'
      ? {
          label: '启动中',
          cls: 'bg-blue-50 dark:bg-blue-950/40 text-blue-700 dark:text-blue-300 border-blue-200 dark:border-blue-900/50',
          dot: 'bg-blue-500 animate-pulse',
        }
      : {
          label: '运行中',
          cls: 'bg-emerald-50 dark:bg-emerald-950/40 text-emerald-700 dark:text-emerald-300 border-emerald-200 dark:border-emerald-900/50',
          dot: 'bg-emerald-500',
        }

  return (
    <span
      className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium border whitespace-nowrap ${tone.cls}`}
    >
      <span className={`w-1.5 h-1.5 rounded-full ${tone.dot}`} />
      {tone.label}
    </span>
  )
}

/** PID / 端口这类"扫一眼就够"的元信息。 */
const MetaPill: React.FC<{ label: string; value: React.ReactNode }> = ({ label, value }) => (
  <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-300">
    <span className="text-zinc-400 dark:text-zinc-500">{label}</span>
    <span className="tabular-nums">{value}</span>
  </span>
)

/**
 * 端地址：等宽 + **可复制**。
 *
 * 排查连接问题的第一件事就是把这个 `ws://` 地址贴到别处（命令、日志、issue），
 * 原先它只是一段 200px 的截断文本，选都选不全。
 */
const EndpointPill: React.FC<{ url: string }> = ({ url }) => {
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(url)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1200)
    } catch {
      // 剪贴板被宿主禁用时什么都不做：地址本身仍然能看、能手动选中
    }
  }
  return (
    <span className="inline-flex items-center gap-1 pl-1.5 pr-0.5 py-0.5 rounded bg-zinc-100 dark:bg-zinc-800 text-zinc-500 dark:text-zinc-400 max-w-full">
      <span className="truncate" title={url}>
        {url}
      </span>
      <button
        type="button"
        onClick={copy}
        aria-label={copied ? '已复制地址' : '复制地址'}
        title={copied ? '已复制' : `复制 ${url}`}
        className="p-0.5 rounded text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors cursor-pointer flex-shrink-0"
      >
        {copied ? <Check size={10} /> : <Copy size={10} />}
      </button>
    </span>
  )
}

export const ProcessModal: React.FC<ProcessModalProps> = ({ isOpen, onClose, onRequestConfirm }) => {
  const [report, setReport] = useState<ProcessReport | null>(null)
  const [gatewayAgents, setGatewayAgents] = useState<ManagedProcessInfo[]>([])
  const [loading, setLoading] = useState(false)
  const [terminatingPid, setTerminatingPid] = useState<number | null>(null)
  const [message, setMessage] = useState<{ text: string; type: 'success' | 'error' | 'info' } | null>(null)
  /// 最近一次拉取成功的时间（`HH:MM:SS`）。轮询每 4 秒一次，没有任何"刚刚更新过"的反馈时，
  /// 用户分不清"面板是活的"还是"界面卡住了"。
  const [lastUpdated, setLastUpdated] = useState('')

  const hasDesktopHost = isDesktop()

  /// `silent` = 后台轮询：**不点亮刷新按钮**。
  ///
  /// 之前轮询也走 `setLoading(true)`，于是每 4 秒刷新图标自己转一圈——
  /// 用户会以为自己在点它，或者以为界面在反复重连。
  const fetchProcesses = useCallback(async (silent = false) => {
    if (!silent) setLoading(true)
    try {
      if (hasDesktopHost) {
        const data = await getProcessReport()
        if (data) {
          setReport(data)

          // 网关模式：实例由网关拉起，桌面端不知道它们 —— 直接问网关的实例注册表
          if (data.mode === 'gateway') {
            try {
              const res = await agentClient.request('gateway.listAgents', {})
              setGatewayAgents(mapGatewayAgents(res))
            } catch (err) {
              // 网关不可达就如实不显示，绝不编造实例
              console.warn('[ProcessModal] 获取网关实例列表失败:', err)
              setGatewayAgents([])
            }
          } else {
            setGatewayAgents([])
          }
        }
      } else {
        // 浏览器开发调试预览兜底
        setReport({
          mode: 'gateway',
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
        setGatewayAgents([])
      }
      setLastUpdated(new Date().toLocaleTimeString('zh-CN', { hour12: false }))
    } catch (err: any) {
      console.error('[ProcessModal] 获取进程状态失败:', err)
      // 轮询失败不弹横幅：网关正在重启时每 4 秒闪一条错误，比不提示更吵
      if (!silent) {
        setMessage({ text: `获取进程状态失败: ${err?.message || String(err)}`, type: 'error' })
      }
    } finally {
      if (!silent) setLoading(false)
    }
  }, [hasDesktopHost])

  useEffect(() => {
    if (isOpen) {
      setMessage(null)
      void fetchProcesses()
      const interval = setInterval(() => {
        void fetchProcesses(true)
      }, 4000)
      return () => clearInterval(interval)
    }
  }, [isOpen, fetchProcesses])

  // 统一确认：优先用应用自己的弹窗；没有注入时才退回原生 confirm（浏览器预览等）
  const askConfirm = async (opts: {
    title?: string
    message: string
    subMessage?: string
    confirmText?: string
    variant?: 'danger' | 'warning' | 'primary'
  }): Promise<boolean> => {
    if (onRequestConfirm) return onRequestConfirm(opts)
    return window.confirm(opts.message)
  }

  const handleKillProcess = async (pid: number, name: string) => {
    const ok = await askConfirm({
      title: '终止进程',
      message: `确定要终止进程 ${name} (PID: ${pid}) 吗？`,
      subMessage: '网关托管的实例被终止后，下次需要时网关会按工作区重新拉起它。',
      confirmText: '终止进程',
      variant: 'danger',
    })
    if (!ok) {
      return
    }
    setTerminatingPid(pid)
    try {
      if (hasDesktopHost) {
        await killProcess(pid)
        setMessage({ text: `已成功终止进程 ${name} (PID: ${pid})`, type: 'success' })
      } else {
        setMessage({ text: `浏览器模拟模式：已请求终止 PID ${pid}`, type: 'info' })
      }
      setTimeout(() => void fetchProcesses(true), 600)
    } catch (err: any) {
      setMessage({ text: `终止进程失败: ${err?.message || String(err)}`, type: 'error' })
    } finally {
      setTerminatingPid(null)
    }
  }

  const handleRestartAll = async () => {
    const ok = await askConfirm({
      title: '重启所有服务',
      message: '确定要重启所有桌面端核心服务并重新加载应用吗？',
      subMessage: '当前正在执行的会话会被中断。',
      confirmText: '重启',
      variant: 'warning',
    })
    if (!ok) {
      return
    }
    try {
      setMessage({ text: '正在重启应用与服务...', type: 'info' })
      await restartDesktopApp()
    } catch (err: any) {
      setMessage({ text: `重启失败: ${err?.message || String(err)}`, type: 'error' })
    }
  }

  /**
   * 只重新唤醒网关：网关进程意外死掉、或改了网关配置之后，
   * 不必重启整个桌面端（会话/界面状态都还在）。
   */
  const handleRestartGateway = async () => {
    const ok = await askConfirm({
      title: '重新唤醒网关',
      message: '确定要重新拉起 ada-gateway 吗？',
      subMessage: '当前桌面端托管的网关进程会被终止，并按原参数重新拉起；网关托管的 agent 实例会随之重启。',
      confirmText: '重新唤醒',
      variant: 'warning',
    })
    if (!ok) {
      return
    }
    try {
      setMessage({ text: '正在重新唤醒网关...', type: 'info' })
      await restartGateway()
      // 网关重启后端口会变，重新握手一次
      setTimeout(() => {
        agentClient.reconnectImmediately()
        void fetchProcesses(true)
      }, 1200)
      setMessage({ text: '已请求重新唤醒网关', type: 'success' })
    } catch (err: any) {
      setMessage({ text: `重新唤醒网关失败: ${err?.message || String(err)}`, type: 'error' })
    }
  }

  // 桌面端自己托管的进程 + **网关托管的实例**（后者是问网关要来的，见 mapGatewayAgents）
  const allProcesses: ManagedProcessInfo[] = [...(report?.processes || []), ...gatewayAgents]

  if (!isOpen) return null

  const modeLabel = report ? (report.mode === 'gateway' ? '网关模式' : '直连模式') : '检测中'

  return (
    <div
      className="fixed inset-0 z-50 bg-black/60 backdrop-blur-xs flex items-center justify-center p-4 animate-in fade-in duration-150"
      // 点遮罩关闭（只认落在遮罩本身上的 mousedown，避免拖选文字时误关）
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="process-modal-title"
        className="w-full max-w-2xl bg-white dark:bg-[#18181b] rounded-2xl border border-zinc-200 dark:border-zinc-800 shadow-2xl flex flex-col max-h-[85vh] overflow-hidden"
      >
        {/* 标题栏 */}
        <div className="px-5 py-3.5 border-b border-zinc-200 dark:border-zinc-800 flex items-center justify-between gap-3 bg-zinc-50/70 dark:bg-[#151518]/70">
          <div className="flex items-center gap-2.5 min-w-0">
            <div className="w-8 h-8 rounded-lg bg-blue-500/10 dark:bg-blue-500/20 text-blue-600 dark:text-blue-400 flex items-center justify-center flex-shrink-0">
              <Activity size={16} />
            </div>
            <div className="min-w-0">
              <div className="flex items-center gap-2 min-w-0">
                <h3
                  id="process-modal-title"
                  className="text-sm font-semibold text-zinc-900 dark:text-zinc-100 truncate"
                >
                  进程管理与服务监控
                </h3>
                <span className="px-1.5 py-0.5 rounded text-[10px] font-medium border border-zinc-200 dark:border-zinc-700 bg-zinc-100/70 dark:bg-zinc-800/70 text-zinc-600 dark:text-zinc-300 whitespace-nowrap">
                  {modeLabel}
                </span>
              </div>
              <p className="text-[11px] text-zinc-500 dark:text-zinc-400 truncate">
                a_da 后端 Agent 与 Gateway 守护进程
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            aria-label="关闭"
            className="p-1.5 text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200/60 dark:hover:bg-zinc-800 rounded-lg transition-colors cursor-pointer flex-shrink-0"
            title="关闭弹窗"
          >
            <X size={16} />
          </button>
        </div>

        {/* 状态总览：左侧"活着吗 + 在哪"，右侧"什么时候更新的 + 手动刷新" */}
        <div className="px-5 py-2.5 bg-zinc-100/50 dark:bg-zinc-900/40 border-b border-zinc-200/80 dark:border-zinc-800/80 flex items-center justify-between gap-3 text-xs">
          <div className="flex items-center gap-2 min-w-0">
            <span
              className={`w-2 h-2 rounded-full flex-shrink-0 ${
                report?.core_alive ? 'bg-emerald-500' : 'bg-rose-500'
              }`}
            />
            <span className="font-medium text-zinc-700 dark:text-zinc-300 whitespace-nowrap">
              {report?.core_alive ? '核心链路在线' : '核心服务离线'}
            </span>
            <span className="text-zinc-300 dark:text-zinc-700 flex-shrink-0">|</span>
            <span
              className="font-mono text-[11px] text-zinc-500 dark:text-zinc-400 truncate"
              title={report?.core_url || ''}
            >
              {report?.core_url || '未连接'}
            </span>
          </div>

          <div className="flex items-center gap-2 flex-shrink-0">
            <span
              className="hidden sm:inline font-mono text-[10px] text-zinc-400 dark:text-zinc-500 tabular-nums"
              title="自动每 4 秒刷新一次；显示的是最近一次成功拉取的时间"
            >
              {lastUpdated ? `更新于 ${lastUpdated}` : '—'}
            </span>
            <button
              onClick={() => void fetchProcesses()}
              disabled={loading}
              aria-label="刷新进程状态"
              className="flex items-center gap-1 px-2.5 py-1 text-xs rounded-md bg-white dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700 hover:bg-zinc-50 dark:hover:bg-zinc-700/80 text-zinc-700 dark:text-zinc-300 transition-colors cursor-pointer shadow-2xs disabled:opacity-60"
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
            className={`px-5 py-2 text-xs flex items-center justify-between gap-3 border-b ${
              message.type === 'success'
                ? 'bg-emerald-50 dark:bg-emerald-950/20 text-emerald-700 dark:text-emerald-300 border-emerald-200 dark:border-emerald-900/30'
                : message.type === 'error'
                ? 'bg-rose-50 dark:bg-rose-950/20 text-rose-700 dark:text-rose-300 border-rose-200 dark:border-rose-900/30'
                : 'bg-blue-50 dark:bg-blue-950/20 text-blue-700 dark:text-blue-300 border-blue-200 dark:border-blue-900/30'
            }`}
          >
            <span className="min-w-0 break-words">{message.text}</span>
            <button
              onClick={() => setMessage(null)}
              aria-label="关闭提示"
              className="text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200 cursor-pointer flex-shrink-0"
            >
              <X size={12} />
            </button>
          </div>
        )}

        {/* 进程列表主体 */}
        <div className="px-5 py-4 overflow-y-auto flex-1">
          <div className="flex items-center justify-between mb-2">
            <span className="text-[11px] font-medium text-zinc-500 dark:text-zinc-400">
              受管进程实例
            </span>
            <span className="text-[11px] font-mono text-zinc-400 dark:text-zinc-500 tabular-nums">
              {allProcesses.length} 个
            </span>
          </div>

          {allProcesses.length === 0 && (
            <div className="mt-1 p-8 text-center border border-dashed border-zinc-200 dark:border-zinc-800 rounded-xl">
              <Terminal size={28} className="mx-auto text-zinc-300 dark:text-zinc-600 mb-2" />
              <p className="text-xs text-zinc-500 dark:text-zinc-400">
                当前暂无托管运行中的子进程
              </p>
            </div>
          )}

          <div className="space-y-2">
            {allProcesses.map((proc, index) => {
              const isGateway = proc.name.includes('gateway')
              const isPm = proc.name.includes('pm')

              return (
                <div
                  key={`${proc.name}-${proc.pid || index}`}
                  className="p-3.5 rounded-xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-[#18181b] hover:border-zinc-300 dark:hover:border-zinc-700 transition-colors flex items-start gap-3"
                >
                  <div
                    className={`w-9 h-9 rounded-lg flex items-center justify-center flex-shrink-0 ${
                      isGateway
                        ? 'bg-amber-500/10 text-amber-600 dark:text-amber-400'
                        : isPm
                        ? 'bg-purple-500/10 text-purple-600 dark:text-purple-400'
                        : 'bg-blue-500/10 text-blue-600 dark:text-blue-400'
                    }`}
                  >
                    {isGateway ? (
                      <Server size={17} />
                    ) : isPm ? (
                      <Cpu size={17} />
                    ) : (
                      <Terminal size={17} />
                    )}
                  </div>

                  <div className="min-w-0 flex-1">
                    {/* 第一行：名字 + 状态（同一个东西的两面，放一起看） */}
                    <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                      <span className="font-mono text-xs font-semibold text-zinc-900 dark:text-zinc-100 truncate">
                        {proc.name}
                      </span>
                      <StatusPill alive={proc.alive} status={proc.status} />
                      {(proc.products || []).map((p) => (
                        <span
                          key={p}
                          title={`本进程服务的产品：${p}`}
                          className={`px-1.5 py-0.5 rounded text-[10px] font-medium border whitespace-nowrap ${
                            p.includes('pm')
                              ? 'bg-purple-500/10 text-purple-600 dark:text-purple-400 border-purple-500/25'
                              : 'bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/25'
                          }`}
                        >
                          服务 {p}
                        </span>
                      ))}
                    </div>

                    {/* 第二行：角色 / 工作区（过长就省略，hover 看全） */}
                    <div
                      className="mt-0.5 text-[11px] text-zinc-500 dark:text-zinc-400 truncate"
                      title={proc.role}
                    >
                      {proc.role}
                    </div>

                    {/* 第三行：排障要用的三件事——PID、端口、地址 */}
                    <div className="mt-2 flex flex-wrap items-center gap-1.5 font-mono text-[10px]">
                      <MetaPill label="PID" value={proc.pid ?? '--'} />
                      {proc.port ? <MetaPill label="端口" value={proc.port} /> : null}
                      {proc.url ? <EndpointPill url={proc.url} /> : null}
                    </div>
                  </div>

                  {/* 右侧：终止（破坏性动作，平时克制，hover 才显红） */}
                  <div className="flex-shrink-0">
                    {proc.pid && proc.alive ? (
                      <button
                        onClick={() => handleKillProcess(proc.pid!, proc.name)}
                        disabled={terminatingPid === proc.pid}
                        className="flex items-center gap-1 px-2 py-1 text-[11px] font-medium text-zinc-500 dark:text-zinc-400 hover:text-rose-600 dark:hover:text-rose-400 border border-transparent hover:border-rose-200 dark:hover:border-rose-900/50 hover:bg-rose-50 dark:hover:bg-rose-950/30 rounded-md transition-colors whitespace-nowrap cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                        title="停止此进程"
                      >
                        <Power size={11} />
                        <span>{terminatingPid === proc.pid ? '停止中…' : '终止'}</span>
                      </button>
                    ) : (
                      <span className="text-[11px] text-zinc-400 dark:text-zinc-500 italic px-2">离线</span>
                    )}
                  </div>
                </div>
              )
            })}
          </div>
        </div>

        {/* 底部功能栏：窄窗口纵向堆叠，按钮组独占一行、不折字 */}
        <div className="px-5 py-3 border-t border-zinc-200 dark:border-zinc-800 bg-zinc-50/70 dark:bg-[#151518]/70 flex flex-col gap-2.5 sm:flex-row sm:items-center sm:justify-between">
          <p className="flex items-start gap-1.5 text-[11px] leading-relaxed text-zinc-500 dark:text-zinc-400 min-w-0">
            <Info size={12} className="mt-0.5 flex-shrink-0" />
            <span>
              网关或实例异常时，可单独重新唤醒网关；重启全部服务会中断正在执行的会话。
            </span>
          </p>

          <div className="flex items-center justify-end gap-2 flex-shrink-0">
            {isDesktop() && (
              <button
                onClick={handleRestartGateway}
                className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium whitespace-nowrap bg-white dark:bg-zinc-900 border border-zinc-300 dark:border-zinc-700 hover:bg-zinc-50 dark:hover:bg-zinc-800 text-zinc-700 dark:text-zinc-200 rounded-lg transition-colors cursor-pointer"
                title="终止当前托管的网关进程并按原参数重新拉起"
              >
                <Radio size={13} />
                <span>重新唤醒网关</span>
              </button>
            )}
            <button
              onClick={handleRestartAll}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium whitespace-nowrap bg-amber-600 hover:bg-amber-500 text-white rounded-lg transition-colors cursor-pointer shadow-xs"
              title="重启所有托管服务"
            >
              <Power size={13} />
              <span>重启所有服务</span>
            </button>
            <button
              onClick={onClose}
              className="px-3 py-1.5 text-xs font-medium whitespace-nowrap text-zinc-600 dark:text-zinc-300 hover:bg-zinc-200/70 dark:hover:bg-zinc-800 rounded-lg transition-colors cursor-pointer"
            >
              关闭
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
