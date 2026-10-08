import React, { useState, useEffect, useCallback } from 'react'
import {
  X,
  Check,
  Globe,
  Key,
  Cpu,
  ShieldCheck,
  Server,
  FileText,
  Sliders,
  AlertCircle,
  Loader2,
  Image as ImageIcon,
  Layers,
  Plus,
  Trash2,
  RefreshCw,
  Search,
  CheckCircle2,
  Edit2,
} from 'lucide-react'
import type {
  ProviderConfig,
  ApprovalMode,
  Effort,
  ModelProtocol,
  ModelEntry,
  ProviderEntry,
} from '../types'
import { agentClient } from '../client/ws-client'
import { notify } from './ToastHost'
import { ConfirmModal } from './ConfirmModal'

interface SettingsModalProps {
  isOpen: boolean
  config: ProviderConfig
  approvalMode: ApprovalMode
  effort: Effort
  onClose: () => void
  onSave: (config: ProviderConfig) => void
}

const PROTOCOL_OPTIONS: { value: ModelProtocol; label: string; desc: string }[] = [
  { value: 'openai_chat', label: 'OpenAI Chat', desc: '标准 Chat Completions 协议 (/chat/completions)' },
  { value: 'anthropic', label: 'Anthropic', desc: '原生 Messages API 协议 (/messages，支持原生思考)' },
  { value: 'openai_responses', label: 'OpenAI Responses', desc: '新一代 Responses API 协议 (/responses)' },
]

export const SettingsModal: React.FC<SettingsModalProps> = ({
  isOpen,
  config,
  approvalMode: initialApprovalMode,
  effort: initialEffort,
  onClose,
  onSave,
}) => {
  // 选项卡
  const [activeTab, setActiveTab] = useState<'provider' | 'runtime'>('provider')

  // 多供应商体系
  const [providers, setProviders] = useState<ProviderEntry[]>([])
  const [activeProviderId, setActiveProviderId] = useState<string>('')
  const [editingId, setEditingId] = useState<string>('')
  const [deleteProviderTarget, setDeleteProviderTarget] = useState<ProviderEntry | null>(null)

  // 当前编辑中的供应商表单状态
  const [editName, setEditName] = useState<string>('')
  const [editProtocol, setEditProtocol] = useState<ModelProtocol>('openai_chat')
  const [editBaseUrl, setEditBaseUrl] = useState<string>('')
  const [editApiKey, setEditApiKey] = useState<string>('')
  const [editProxyUrl, setEditProxyUrl] = useState<string>('')
  const [editCustomHeadersText, setEditCustomHeadersText] = useState<string>('')
  const [editModels, setEditModels] = useState<ModelEntry[]>([])
  const [editDefaultModel, setEditDefaultModel] = useState<string>('')

  // 远程模型探测状态
  const [fetchingModels, setFetchingModels] = useState<boolean>(false)
  const [remoteModels, setRemoteModels] = useState<ModelEntry[]>([])
  const [remoteSearchQuery, setRemoteSearchQuery] = useState<string>('')
  const [showRemotePicker, setShowRemotePicker] = useState<boolean>(false)
  const [fetchModelMsg, setFetchModelMsg] = useState<{ type: 'success' | 'error'; text: string } | null>(null)

  // 手动新增模型临时输入
  const [showAddManualModel, setShowAddManualModel] = useState<boolean>(false)
  const [manualModelId, setManualModelId] = useState<string>('')
  const [manualModelName, setManualModelName] = useState<string>('')
  const [manualContextWindow, setManualContextWindow] = useState<number>(128000)
  const [manualMaxOutputTokens, setManualMaxOutputTokens] = useState<number>(8192)
  const [manualSupportsImages, setManualSupportsImages] = useState<boolean>(true)

  // 正在编辑的已有模型 ID 与草稿
  const [editingModelId, setEditingModelId] = useState<string | null>(null)
  const [editingModelDraft, setEditingModelDraft] = useState<ModelEntry | null>(null)

  // 运行环境配置
  const [configFilePath, setConfigFilePath] = useState<string>('')
  const [approvalMode, setApprovalMode] = useState<ApprovalMode>(initialApprovalMode)
  const [effort, setEffort] = useState<Effort>(initialEffort)

  // 操作反馈
  const [testing, setTesting] = useState(false)
  const [testResult, setTestResult] = useState<{ success: boolean; message: string } | null>(null)
  const [savedSuccess, setSavedSuccess] = useState(false)

  // 解析请求头字符串
  const parseHeaders = (text: string): Record<string, string> | undefined => {
    const trimmed = text.trim()
    if (!trimmed) return undefined
    const result: Record<string, string> = {}
    const items = trimmed.split(/;|\n/)
    for (const item of items) {
      const splitIdx = item.indexOf(':')
      if (splitIdx > 0) {
        const key = item.slice(0, splitIdx).trim()
        const val = item.slice(splitIdx + 1).trim()
        if (key && val) {
          result[key] = val
        }
      }
    }
    return Object.keys(result).length > 0 ? result : undefined
  }

  // 序列化请求头
  const formatHeaders = (headers?: Record<string, string>): string => {
    if (!headers || Object.keys(headers).length === 0) return ''
    return Object.entries(headers).map(([k, v]) => `${k}: ${v}`).join('; ')
  }

  // 核心：实时将右侧表单的修改同步写回当前 providers 列表中对应项，避免左侧列表与右侧输入不同步
  const updateCurrentProviderField = useCallback(
    <K extends keyof ProviderEntry>(field: K, val: ProviderEntry[K]) => {
      setProviders((prev) =>
        prev.map((item) => (item.id === editingId ? { ...item, [field]: val } : item))
      )
    },
    [editingId]
  )

  // 加载指定供应商进入右侧编辑表单
  const loadProviderIntoForm = useCallback((p: ProviderEntry) => {
    setEditingId(p.id)
    setEditName(p.name)
    setEditProtocol(p.protocol || 'openai_chat')
    setEditBaseUrl(p.baseUrl)
    setEditApiKey(p.apiKey)
    setEditProxyUrl(p.proxyUrl || '')
    setEditCustomHeadersText(formatHeaders(p.customHeaders))
    setEditModels(p.models || [])
    setEditDefaultModel(p.models?.[0]?.id || '')
    setEditingModelId(null)
    setEditingModelDraft(null)
    setRemoteModels([])
    setShowRemotePicker(false)
    setFetchModelMsg(null)
    setTestResult(null)
  }, [])

  // 弹窗打开时拉取持久化配置
  useEffect(() => {
    if (!isOpen) return

    let cancelled = false
    void (async () => {
      try {
        const [configRes, provListRes] = await Promise.all([
          agentClient.fetchConfig(),
          agentClient.listProviders(),
        ])

        if (cancelled) return

        if (configRes?.path) {
          setConfigFilePath(configRes.path)
        }

        let pList = provListRes?.providers || []
        let curActiveId = provListRes?.activeProviderId || ''

        // 若后端列表为空，用传入配置构建兜底项
        if (pList.length === 0) {
          const fallbackEntry: ProviderEntry = {
            id: 'default',
            name: '默认供应商',
            protocol: config.protocol || 'openai_chat',
            baseUrl: config.baseUrl || 'https://api.openai.com/v1',
            apiKey: config.apiKey || '',
            models: [
              {
                id: config.model || 'gpt-4o',
                name: config.model || 'gpt-4o',
                contextWindow: config.contextWindow || 128000,
                supportsImages: config.supportsImages ?? true,
              },
            ],
            customHeaders: config.customHeaders,
            proxyUrl: config.proxyUrl,
          }
          pList = [fallbackEntry]
          curActiveId = 'default'
        }

        setProviders(pList)
        setActiveProviderId(curActiveId || pList[0].id)

        // 优先载入激活中的供应商
        const activeEntry = pList.find((p) => p.id === curActiveId) || pList[0]
        loadProviderIntoForm(activeEntry)
      } catch (err) {
        console.warn('获取供应商数据失败:', err)
      }
    })()

    return () => {
      cancelled = true
    }
  }, [isOpen, loadProviderIntoForm])

  if (!isOpen) return null

  // 新建供应商
  const handleCreateNewProvider = () => {
    const newId = `prov_${Date.now().toString(36)}`
    const newEntry: ProviderEntry = {
      id: newId,
      name: '新建供应商',
      protocol: 'openai_chat',
      baseUrl: 'https://api.openai.com/v1',
      apiKey: '',
      models: [
        {
          id: 'gpt-4o',
          name: 'GPT-4o',
          contextWindow: 128000,
          supportsImages: true,
        },
      ],
    }
    setProviders((prev) => [...prev, newEntry])
    loadProviderIntoForm(newEntry)
  }

  // 设为激活
  const handleSetActive = async (id: string, modelId?: string) => {
    try {
      await agentClient.setActiveProvider(id, modelId)
      setActiveProviderId(id)

      // 寻找当前最新的供应商实体
      const currentLatest =
        id === editingId
          ? {
              id: editingId,
              name: editName.trim() || '未命名供应商',
              protocol: editProtocol,
              baseUrl: editBaseUrl.trim(),
              apiKey: editApiKey.trim(),
              proxyUrl: editProxyUrl.trim() || undefined,
              models: editModels,
              customHeaders: parseHeaders(editCustomHeadersText),
            }
          : providers.find((p) => p.id === id)

      if (currentLatest) {
        const targetModelId = modelId || currentLatest.models?.[0]?.id || 'gpt-4o'
        const targetM =
          currentLatest.models?.find((m) => m.id === targetModelId) || currentLatest.models?.[0]
        onSave({
          baseUrl: currentLatest.baseUrl,
          apiKey: currentLatest.apiKey,
          model: targetModelId,
          protocol: currentLatest.protocol,
          contextWindow: targetM?.contextWindow || 128000,
          supportsImages: targetM?.supportsImages ?? true,
          customHeaders: currentLatest.customHeaders,
          proxyUrl: currentLatest.proxyUrl,
        })
      }
    } catch (err: any) {
      notify({ message: `切换激活供应商失败: ${err?.message || err}`, level: 'error' })
    }
  }

  // 删除供应商
  const handleDeleteProvider = (id: string, e: React.MouseEvent) => {
    e.stopPropagation()
    if (providers.length <= 1) {
      notify({ message: '至少保留一个供应商，无法删除最后一个供应商', level: 'warn' })
      return
    }
    const target = providers.find((p) => p.id === id)
    if (target) {
      setDeleteProviderTarget(target)
    }
  }

  const handleDeleteProviderConfirm = async () => {
    if (!deleteProviderTarget) return
    const id = deleteProviderTarget.id
    try {
      await agentClient.deleteProvider(id)
      const remaining = providers.filter((p) => p.id !== id)
      setProviders(remaining)
      if (editingId === id && remaining.length > 0) {
        loadProviderIntoForm(remaining[0])
      }
      if (activeProviderId === id && remaining.length > 0) {
        setActiveProviderId(remaining[0].id)
      }
      notify({ message: `已删除供应商: ${deleteProviderTarget.name}`, level: 'success' })
    } catch (err: any) {
      notify({ message: `删除供应商失败: ${err?.message || err}`, level: 'error' })
    } finally {
      setDeleteProviderTarget(null)
    }
  }

  // 获取通用远程模型列表 (/models)
  const handleFetchRemoteModels = async () => {
    if (!editBaseUrl.trim()) {
      setFetchModelMsg({ type: 'error', text: '请先填写供应商 API 端点 (Base URL)' })
      return
    }

    setFetchingModels(true)
    setFetchModelMsg(null)
    setShowRemotePicker(false)

    try {
      const headers = parseHeaders(editCustomHeadersText)
      const res = await agentClient.fetchRemoteModels({
        protocol: editProtocol,
        baseUrl: editBaseUrl.trim(),
        apiKey: editApiKey.trim(),
        customHeaders: headers,
        proxyUrl: editProxyUrl.trim() || undefined,
      })

      const list = res?.models || []
      setRemoteModels(list)
      if (list.length === 0) {
        setFetchModelMsg({
          type: 'error',
          text: '远程端点已响应，但未发现可用模型列表。您可以点击右上角手动添加模型。',
        })
      } else {
        setFetchModelMsg({
          type: 'success',
          text: `成功获取到 ${list.length} 个可用模型，请在下方点击添加至当前供应商列表。`,
        })
        setShowRemotePicker(true)
      }
    } catch (err: any) {
      setFetchModelMsg({
        type: 'error',
        text: `获取模型失败: ${err?.message || err}`,
      })
    } finally {
      setFetchingModels(false)
    }
  }

  // 从远程选择中添加模型到当前供应商
  const handleAddRemoteModel = (model: ModelEntry) => {
    if (editModels.some((m) => m.id === model.id)) {
      return
    }
    const updated = [
      ...editModels,
      {
        ...model,
        contextWindow: model.contextWindow || 128000,
        maxOutputTokens: model.maxOutputTokens || 8192,
        supportsImages: model.supportsImages ?? true,
      },
    ]
    setEditModels(updated)
    updateCurrentProviderField('models', updated)
    if (!editDefaultModel) {
      setEditDefaultModel(model.id)
    }
  }

  // 手动添加模型
  const handleAddManualModel = () => {
    const id = manualModelId.trim()
    if (!id) return
    if (editModels.some((m) => m.id === id)) {
      notify({ message: '该模型已存在于列表中', level: 'warn' })
      return
    }

    const newM: ModelEntry = {
      id,
      name: manualModelName.trim() || id,
      contextWindow: manualContextWindow || 128000,
      maxOutputTokens: manualMaxOutputTokens || 8192,
      supportsImages: manualSupportsImages,
    }
    const updated = [...editModels, newM]
    setEditModels(updated)
    updateCurrentProviderField('models', updated)
    if (!editDefaultModel) {
      setEditDefaultModel(id)
    }
    setManualModelId('')
    setManualModelName('')
    setManualContextWindow(128000)
    setManualMaxOutputTokens(8192)
    setManualSupportsImages(true)
    setShowAddManualModel(false)
  }

  // 开启编辑模型
  const handleStartEditModel = (model: ModelEntry) => {
    setEditingModelId(model.id)
    setEditingModelDraft({
      id: model.id,
      name: model.name || model.id,
      contextWindow: model.contextWindow || 128000,
      maxOutputTokens: model.maxOutputTokens || 8192,
      supportsImages: model.supportsImages ?? true,
    })
  }

  // 取消编辑模型
  const handleCancelEditModel = () => {
    setEditingModelId(null)
    setEditingModelDraft(null)
  }

  // 保存模型修改
  const handleSaveEditedModel = () => {
    if (!editingModelId || !editingModelDraft) return
    const newId = editingModelDraft.id.trim()
    if (!newId) {
      notify({ message: '模型 ID 不能为空', level: 'warn' })
      return
    }
    // 检查 ID 是否与除了自身以外的其他模型重名
    if (newId !== editingModelId && editModels.some((m) => m.id === newId)) {
      notify({ message: '该模型 ID 已存在，请使用其他 ID', level: 'warn' })
      return
    }

    const updated = editModels.map((m) => {
      if (m.id === editingModelId) {
        return {
          id: newId,
          name: editingModelDraft.name?.trim() || newId,
          contextWindow: Number(editingModelDraft.contextWindow) || 128000,
          maxOutputTokens: Number(editingModelDraft.maxOutputTokens) || 8192,
          supportsImages: editingModelDraft.supportsImages ?? true,
        }
      }
      return m
    })

    setEditModels(updated)
    updateCurrentProviderField('models', updated)

    // 若原模型是默认模型且 ID 改变，更新默认模型 ID
    if (editDefaultModel === editingModelId && newId !== editingModelId) {
      setEditDefaultModel(newId)
    }

    setEditingModelId(null)
    setEditingModelDraft(null)
  }

  // 从当前列表移除模型
  const handleRemoveModel = (id: string) => {
    if (editModels.length <= 1) {
      notify({ message: '每个供应商至少保留一个模型标识', level: 'warn' })
      return
    }
    const next = editModels.filter((m) => m.id !== id)
    setEditModels(next)
    updateCurrentProviderField('models', next)
    if (editDefaultModel === id) {
      setEditDefaultModel(next[0]?.id || '')
    }
    if (editingModelId === id) {
      setEditingModelId(null)
      setEditingModelDraft(null)
    }
  }

  // 测试当前表单连通性
  const handleTestConnection = async () => {
    setTesting(true)
    setTestResult(null)
    try {
      const activeModel = editDefaultModel || editModels[0]?.id || 'gpt-4o'
      const testConfig: ProviderConfig = {
        baseUrl: editBaseUrl.trim(),
        apiKey: editApiKey.trim(),
        model: activeModel,
        protocol: editProtocol,
        customHeaders: parseHeaders(editCustomHeadersText),
        proxyUrl: editProxyUrl.trim() || undefined,
      }
      const res = await agentClient.checkProvider(testConfig)
      setTestResult({
        success: true,
        message: res?.message || '模型连接验证通过，参数有效！',
      })
    } catch (err: any) {
      setTestResult({
        success: false,
        message: err?.message || '连接失败，请检查 Base URL 与 API Key 是否有效。',
      })
    } finally {
      setTesting(false)
    }
  }

  // 保存当前编辑的供应商（彻底杜绝旧默认值覆盖）
  const handleSaveCurrentProvider = async (makeActive: boolean = false) => {
    if (!editName.trim()) {
      notify({ message: '请输入供应商名称', level: 'warn' })
      return
    }
    if (!editBaseUrl.trim()) {
      notify({ message: '请输入供应商 API 端点 (Base URL)', level: 'warn' })
      return
    }
    if (editModels.length === 0) {
      notify({ message: '请至少配置或获取一个模型', level: 'warn' })
      return
    }

    // 构造完全由当前最新表单输入组成的实体对象
    const finalEntry: ProviderEntry = {
      id: editingId,
      name: editName.trim(),
      protocol: editProtocol,
      baseUrl: editBaseUrl.trim(),
      apiKey: editApiKey.trim(),
      models: editModels,
      customHeaders: parseHeaders(editCustomHeadersText),
      proxyUrl: editProxyUrl.trim() || undefined,
    }

    try {
      // 1. 发送 provider.save 保存该实体
      await agentClient.saveProvider(finalEntry)

      // 2. 更新本地状态列表
      setProviders((prev) => prev.map((p) => (p.id === editingId ? finalEntry : p)))

      // 3. 若设为激活或当前编辑的就是激活项，执行激活逻辑
      const shouldActivate = makeActive || editingId === activeProviderId
      if (shouldActivate) {
        const targetModelId = editDefaultModel || finalEntry.models[0]?.id || 'gpt-4o'
        await agentClient.setActiveProvider(editingId, targetModelId)
        setActiveProviderId(editingId)

        const activeM =
          finalEntry.models.find((m) => m.id === targetModelId) || finalEntry.models[0]

        // 同步通知外层组件当前激活状态，严禁使用旧闭包变量
        onSave({
          baseUrl: finalEntry.baseUrl,
          apiKey: finalEntry.apiKey,
          model: targetModelId,
          protocol: finalEntry.protocol,
          contextWindow: activeM?.contextWindow || 128000,
          maxOutputTokens: activeM?.maxOutputTokens || 8192,
          supportsImages: activeM?.supportsImages ?? true,
          customHeaders: finalEntry.customHeaders,
          proxyUrl: finalEntry.proxyUrl,
        })
      }

      await agentClient.setApprovalMode(approvalMode)
      await agentClient.setEffort(effort)

      setSavedSuccess(true)
      setTimeout(() => {
        setSavedSuccess(false)
        onClose()
      }, 500)
    } catch (err: any) {
      notify({ message: `保存失败: ${err?.message || err}`, level: 'error' })
    }
  }

  // 过滤远程候选模型
  const filteredRemoteModels = remoteModels.filter((m) => {
    if (!remoteSearchQuery) return true
    const q = remoteSearchQuery.toLowerCase()
    return m.id.toLowerCase().includes(q) || (m.name && m.name.toLowerCase().includes(q))
  })

  return (
    <div className="fixed inset-0 bg-black/40 dark:bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4 select-none">
      <div className="w-full max-w-4xl xl:max-w-5xl h-[640px] max-h-[92vh] bg-white dark:bg-[#18181b] border border-zinc-200 dark:border-[#2e2e33] rounded-2xl shadow-2xl overflow-hidden flex flex-col text-xs transition-colors">
        {/* 顶部标题栏 */}
        <div className="flex items-center justify-between px-5 py-3 border-b border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#1e1e22]/50 flex-shrink-0">
          <div className="flex items-center space-x-2">
            <Sliders size={15} className="text-blue-500 dark:text-blue-400" />
            <h3 className="font-semibold text-zinc-900 dark:text-zinc-100 text-sm">模型供应商与运行环境配置</h3>
          </div>
          <button
            onClick={onClose}
            className="p-1 text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-white hover:bg-zinc-100 dark:hover:bg-zinc-800 rounded-lg transition-colors"
          >
            <X size={15} />
          </button>
        </div>

        {/* 标签切换栏 */}
        <div className="flex border-b border-zinc-200 dark:border-[#27272a] bg-zinc-100/70 dark:bg-[#141416] px-4 pt-1 flex-shrink-0">
          <button
            onClick={() => setActiveTab('provider')}
            className={`flex items-center space-x-1.5 px-3 py-2 font-medium border-b-2 transition-colors ${
              activeTab === 'provider'
                ? 'border-blue-600 dark:border-blue-500 text-blue-600 dark:text-blue-400'
                : 'border-transparent text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200'
            }`}
          >
            <Server size={13} />
            <span>大模型供应商管理 ({providers.length})</span>
          </button>
          <button
            onClick={() => setActiveTab('runtime')}
            className={`flex items-center space-x-1.5 px-3 py-2 font-medium border-b-2 transition-colors ${
              activeTab === 'runtime'
                ? 'border-blue-600 dark:border-blue-500 text-blue-600 dark:text-blue-400'
                : 'border-transparent text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200'
            }`}
          >
            <ShieldCheck size={13} />
            <span>执行策略与推演力度</span>
          </button>
        </div>

        {/* 表单内容区：大模型供应商采用左/右 Master-Detail 左右布局 */}
        <div className="flex-1 overflow-hidden flex flex-col min-h-0">
          {activeTab === 'provider' && (
            <div className="flex flex-1 overflow-hidden min-h-0">
              {/* 左侧栏：已配置的供应商列表 (Master) */}
              <div className="w-64 sm:w-72 border-r border-zinc-200 dark:border-[#27272a] bg-zinc-50/70 dark:bg-[#141417] p-3 flex flex-col overflow-hidden flex-shrink-0">
                <div className="flex items-center justify-between pb-2 border-b border-zinc-200/80 dark:border-zinc-800">
                  <span className="font-semibold text-zinc-800 dark:text-zinc-200 flex items-center space-x-1.5">
                    <Server size={13} className="text-blue-500" />
                    <span>已配置的供应商</span>
                  </span>
                  <span className="text-[10px] px-1.5 py-0.2 rounded-full bg-zinc-200 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 font-mono">
                    {providers.length}
                  </span>
                </div>

                <div className="flex-1 overflow-y-auto space-y-2 pt-2 pr-1">
                  {providers.map((p) => {
                    const isEditing = editingId === p.id
                    const isActive = activeProviderId === p.id
                    return (
                      <div
                        key={p.id}
                        onClick={() => loadProviderIntoForm(p)}
                        className={`group relative p-2.5 rounded-xl border text-left cursor-pointer transition-all ${
                          isEditing
                            ? 'border-blue-500 bg-blue-50/80 dark:bg-blue-950/30 shadow-sm ring-1 ring-blue-500/20'
                            : 'border-zinc-200 dark:border-zinc-800 bg-white dark:bg-[#18181b] hover:border-zinc-300 dark:hover:border-zinc-700'
                        }`}
                      >
                        <div className="flex items-center justify-between">
                          <span className="font-semibold text-zinc-900 dark:text-zinc-100 truncate pr-1 text-xs">
                            {p.name}
                          </span>
                          {isActive && (
                            <span className="flex items-center space-x-0.5 px-1.5 py-0.5 rounded-full text-[9px] bg-emerald-100 dark:bg-emerald-950/60 text-emerald-700 dark:text-emerald-300 font-medium border border-emerald-300 dark:border-emerald-700 flex-shrink-0">
                              <CheckCircle2 size={10} />
                              <span>激活中</span>
                            </span>
                          )}
                        </div>

                        <div className="flex items-center space-x-1.5 mt-1.5">
                          <span className="text-[10px] px-1.5 py-0.2 rounded bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 font-mono">
                            {p.protocol === 'anthropic' ? 'Anthropic' : p.protocol === 'openai_responses' ? 'Responses' : 'Chat'}
                          </span>
                          <span className="text-[10px] text-zinc-400 dark:text-zinc-500">
                            {p.models?.length || 0} 个模型
                          </span>
                        </div>

                        <div className="text-[10px] text-zinc-400 dark:text-zinc-500 truncate mt-1 font-mono">
                          {p.baseUrl}
                        </div>

                        {providers.length > 1 && (
                          <button
                            type="button"
                            onClick={(e) => handleDeleteProvider(p.id, e)}
                            title="删除供应商"
                            className="absolute top-2 right-2 p-1 text-zinc-400 hover:text-rose-500 opacity-0 group-hover:opacity-100 transition-opacity"
                          >
                            <Trash2 size={11} />
                          </button>
                        )}
                      </div>
                    )
                  })}
                </div>
              </div>

              {/* 右侧栏：选中供应商详情与模型管理 (Detail) */}
              <div className="flex-1 overflow-y-auto p-4 space-y-3.5 bg-white dark:bg-[#18181b]">
                {/* 供应商编辑标题与操作栏 */}
                <div className="flex items-center justify-between pb-2 border-b border-zinc-200 dark:border-zinc-800">
                  <div className="font-medium text-zinc-900 dark:text-zinc-100 flex items-center space-x-2">
                    <span className="text-zinc-500">编辑供应商：</span>
                    <input
                      type="text"
                      value={editName}
                      onChange={(e) => {
                        const val = e.target.value
                        setEditName(val)
                        updateCurrentProviderField('name', val)
                      }}
                      placeholder="供应商名称"
                      className="px-2.5 py-1 rounded-lg border border-zinc-300 dark:border-zinc-700 bg-zinc-50 dark:bg-zinc-800 text-xs font-semibold text-zinc-900 dark:text-zinc-100 outline-none focus:border-blue-500 w-52"
                    />
                  </div>

                  <div className="flex items-center space-x-2">
                    {activeProviderId === editingId ? (
                      <span className="text-[11px] text-emerald-600 dark:text-emerald-400 font-medium flex items-center space-x-1 px-2.5 py-1 rounded-md bg-emerald-50 dark:bg-emerald-950/30 border border-emerald-200 dark:border-emerald-800/40">
                        <CheckCircle2 size={12} />
                        <span>当前默认激活此供应商</span>
                      </span>
                    ) : (
                      <button
                        type="button"
                        onClick={() => handleSetActive(editingId)}
                        className="px-2.5 py-1 rounded-md bg-zinc-100 dark:bg-zinc-800 hover:bg-blue-600 hover:text-white text-zinc-700 dark:text-zinc-300 text-[11px] font-medium transition-colors"
                      >
                        设为当前激活
                      </button>
                    )}

                    <button
                      type="button"
                      onClick={handleCreateNewProvider}
                      className="flex items-center space-x-1 px-3 py-1 rounded-md bg-blue-50 dark:bg-blue-950/40 border border-blue-200 dark:border-blue-800 text-blue-600 dark:text-blue-400 hover:bg-blue-100 font-medium transition-colors"
                    >
                      <Plus size={12} />
                      <span>添加供应商</span>
                    </button>
                  </div>
                </div>

                {/* 接口协议选择 */}
                <div className="space-y-1.5">
                  <label className="text-zinc-700 dark:text-zinc-300 font-medium flex items-center space-x-1.5">
                    <Layers size={13} className="text-purple-500" />
                    <span>接口协议 (API Protocol)</span>
                  </label>
                  <div className="grid grid-cols-3 gap-2">
                    {PROTOCOL_OPTIONS.map((opt) => {
                      const isSelected = editProtocol === opt.value
                      return (
                        <button
                          key={opt.value}
                          type="button"
                          onClick={() => {
                            setEditProtocol(opt.value)
                            updateCurrentProviderField('protocol', opt.value)
                          }}
                          className={`p-2.5 rounded-xl border text-left transition-all ${
                            isSelected
                              ? 'border-purple-500 bg-purple-500/10 text-purple-600 dark:text-purple-300 font-medium shadow-sm'
                              : 'border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-900 text-zinc-700 dark:text-zinc-400 hover:border-zinc-300'
                          }`}
                        >
                          <div className="font-semibold text-xs">{opt.label}</div>
                          <div className="text-[10px] text-zinc-400 dark:text-zinc-500 mt-0.5 line-clamp-1">{opt.desc}</div>
                        </button>
                      )
                    })}
                  </div>
                </div>

                {/* API 端点与密钥 */}
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <div className="space-y-1.5">
                    <label className="flex items-center space-x-1.5 text-zinc-700 dark:text-zinc-300 font-medium">
                      <Globe size={13} className="text-blue-500" />
                      <span>API 端点 (Base URL)</span>
                    </label>
                    <input
                      type="text"
                      value={editBaseUrl}
                      onChange={(e) => {
                        const val = e.target.value
                        setEditBaseUrl(val)
                        updateCurrentProviderField('baseUrl', val)
                      }}
                      placeholder={editProtocol === 'anthropic' ? 'https://api.anthropic.com' : 'https://api.openai.com/v1'}
                      className="w-full bg-zinc-50 dark:bg-[#121214] border border-zinc-300 dark:border-[#2f2f35] rounded-lg px-3 py-1.5 text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 outline-none focus:border-blue-500 font-mono text-xs"
                    />
                  </div>

                  <div className="space-y-1.5">
                    <label className="flex items-center space-x-1.5 text-zinc-700 dark:text-zinc-300 font-medium">
                      <Key size={13} className="text-amber-500" />
                      <span>接口密钥 (API Key)</span>
                    </label>
                    <input
                      type="password"
                      value={editApiKey}
                      onChange={(e) => {
                        const val = e.target.value
                        setEditApiKey(val)
                        updateCurrentProviderField('apiKey', val)
                      }}
                      placeholder="sk-..."
                      className="w-full bg-zinc-50 dark:bg-[#121214] border border-zinc-300 dark:border-[#2f2f35] rounded-lg px-3 py-1.5 text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 outline-none focus:border-blue-500 font-mono text-xs"
                    />
                  </div>
                </div>

                {/* 网络代理与自定义请求头 */}
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <div className="space-y-1">
                    <div className="flex items-center justify-between text-zinc-600 dark:text-zinc-400 text-[11px]">
                      <span className="flex items-center space-x-1">
                        <Globe size={11} className="text-cyan-500" />
                        <span>网络代理 (Proxy URL 可选)</span>
                      </span>
                      <span className="text-[10px] text-zinc-400">HTTP / HTTPS / SOCKS5</span>
                    </div>
                    <input
                      type="text"
                      value={editProxyUrl}
                      onChange={(e) => {
                        const val = e.target.value
                        setEditProxyUrl(val)
                        updateCurrentProviderField('proxyUrl', val.trim() || undefined)
                      }}
                      placeholder="http://127.0.0.1:7890 或 socks5://127.0.0.1:1080"
                      className="w-full bg-zinc-50 dark:bg-[#121214] border border-zinc-300 dark:border-[#2f2f35] rounded-lg px-3 py-1.5 text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 outline-none focus:border-blue-500 font-mono text-xs"
                    />
                  </div>

                  <div className="space-y-1">
                    <div className="flex items-center justify-between text-zinc-600 dark:text-zinc-400 text-[11px]">
                      <span className="flex items-center space-x-1">
                        <Layers size={11} className="text-emerald-500" />
                        <span>自定义请求头 (可选)</span>
                      </span>
                      <span className="text-[10px] text-zinc-400">Key: Value; Key2: Value2</span>
                    </div>
                    <input
                      type="text"
                      value={editCustomHeadersText}
                      onChange={(e) => {
                        const val = e.target.value
                        setEditCustomHeadersText(val)
                        updateCurrentProviderField('customHeaders', parseHeaders(val))
                      }}
                      placeholder="X-Custom-Auth: token; Custom-Header: 123"
                      className="w-full bg-zinc-50 dark:bg-[#121214] border border-zinc-300 dark:border-[#2f2f35] rounded-lg px-3 py-1.5 text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 outline-none focus:border-blue-500 font-mono text-xs"
                    />
                  </div>
                </div>

                {/* 可用模型列表与远程获取 */}
                <div className="space-y-2 pt-2 border-t border-zinc-200 dark:border-zinc-800">
                  <div className="flex items-center justify-between">
                    <label className="flex items-center space-x-1.5 text-zinc-800 dark:text-zinc-200 font-medium">
                      <Cpu size={13} className="text-indigo-500" />
                      <span>可用模型列表 ({editModels.length})</span>
                    </label>

                    <div className="flex items-center space-x-2">
                      <button
                        type="button"
                        onClick={handleFetchRemoteModels}
                        disabled={fetchingModels || !editBaseUrl.trim()}
                        className="flex items-center space-x-1.5 px-2.5 py-1 rounded-md bg-indigo-50 dark:bg-indigo-950/40 border border-indigo-200 dark:border-indigo-800 text-indigo-600 dark:text-indigo-400 hover:bg-indigo-100 font-medium transition-colors disabled:opacity-50"
                      >
                        {fetchingModels ? (
                          <Loader2 size={12} className="animate-spin text-indigo-500" />
                        ) : (
                          <RefreshCw size={12} />
                        )}
                        <span>{fetchingModels ? '探测中...' : '获取远程可用模型 (/models)'}</span>
                      </button>

                      <button
                        type="button"
                        onClick={() => setShowAddManualModel(!showAddManualModel)}
                        className="flex items-center space-x-1 px-2 py-1 rounded-md border border-zinc-300 dark:border-zinc-700 bg-zinc-50 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 transition-colors"
                      >
                        <Plus size={11} />
                        <span>手动添加</span>
                      </button>
                    </div>
                  </div>

                  {/* 远程探测反馈 */}
                  {fetchModelMsg && (
                    <div
                      className={`p-2 rounded-lg border text-[11px] flex items-center space-x-2 ${
                        fetchModelMsg.type === 'success'
                          ? 'bg-emerald-50 dark:bg-emerald-950/20 border-emerald-200 dark:border-emerald-800/40 text-emerald-700 dark:text-emerald-300'
                          : 'bg-rose-50 dark:bg-rose-950/20 border-rose-200 dark:border-rose-800/40 text-rose-700 dark:text-rose-300'
                      }`}
                    >
                      {fetchModelMsg.type === 'success' ? <CheckCircle2 size={13} /> : <AlertCircle size={13} />}
                      <span>{fetchModelMsg.text}</span>
                    </div>
                  )}

                  {/* 远程选择面板 */}
                  {showRemotePicker && remoteModels.length > 0 && (
                    <div className="p-3 rounded-xl border border-indigo-200 dark:border-indigo-900/50 bg-indigo-50/40 dark:bg-indigo-950/20 space-y-2">
                      <div className="flex items-center justify-between">
                        <span className="font-medium text-indigo-900 dark:text-indigo-200">
                          发现 {remoteModels.length} 个远程模型，点击即可添加：
                        </span>
                        <div className="relative w-44">
                          <Search size={11} className="absolute left-2 top-2 text-zinc-400" />
                          <input
                            type="text"
                            value={remoteSearchQuery}
                            onChange={(e) => setRemoteSearchQuery(e.target.value)}
                            placeholder="筛选模型 ID..."
                            className="w-full bg-white dark:bg-zinc-900 border border-zinc-300 dark:border-zinc-700 rounded-md pl-6 pr-2 py-1 text-[11px] outline-none"
                          />
                        </div>
                      </div>

                      <div className="max-h-36 overflow-y-auto grid grid-cols-2 gap-1.5 pr-1">
                        {filteredRemoteModels.map((rm) => {
                          const alreadyAdded = editModels.some((m) => m.id === rm.id)
                          return (
                            <button
                              key={rm.id}
                              type="button"
                              onClick={() => handleAddRemoteModel(rm)}
                              disabled={alreadyAdded}
                              className={`flex items-center justify-between px-2.5 py-1.5 rounded-lg border text-left transition-colors ${
                                alreadyAdded
                                  ? 'bg-zinc-100 dark:bg-zinc-800/50 border-zinc-200 dark:border-zinc-800 text-zinc-400 cursor-not-allowed'
                                  : 'bg-white dark:bg-zinc-900 border-indigo-200 dark:border-indigo-800/60 hover:border-indigo-500 text-zinc-800 dark:text-zinc-200'
                              }`}
                            >
                              <div className="truncate mr-2 font-mono text-[11px]">{rm.id}</div>
                              {alreadyAdded ? (
                                <span className="text-[10px] text-zinc-400">已添加</span>
                              ) : (
                                <span className="text-[10px] text-indigo-600 dark:text-indigo-400 font-semibold">+ 添加</span>
                              )}
                            </button>
                          )
                        })}
                      </div>
                    </div>
                  )}

                  {/* 手动新增模型表单 */}
                  {showAddManualModel && (
                    <div className="p-3 rounded-xl border border-zinc-300 dark:border-zinc-700 bg-zinc-50 dark:bg-zinc-900 space-y-2.5">
                      <div className="font-semibold text-zinc-900 dark:text-zinc-100 flex items-center space-x-1.5">
                        <Plus size={13} className="text-blue-500" />
                        <span>手动新增模型项</span>
                      </div>
                      <div className="grid grid-cols-2 gap-2">
                        <div>
                          <label className="block text-[11px] text-zinc-500 mb-0.5">模型 ID (必填):</label>
                          <input
                            type="text"
                            value={manualModelId}
                            onChange={(e) => setManualModelId(e.target.value)}
                            placeholder="如 claude-3-7-sonnet"
                            className="w-full bg-white dark:bg-zinc-800 border border-zinc-300 dark:border-zinc-700 rounded px-2.5 py-1 text-xs outline-none font-mono"
                          />
                        </div>
                        <div>
                          <label className="block text-[11px] text-zinc-500 mb-0.5">显示名称 (可选):</label>
                          <input
                            type="text"
                            value={manualModelName}
                            onChange={(e) => setManualModelName(e.target.value)}
                            placeholder="如 Claude 3.7 Sonnet"
                            className="w-full bg-white dark:bg-zinc-800 border border-zinc-300 dark:border-zinc-700 rounded px-2.5 py-1 text-xs outline-none"
                          />
                        </div>
                      </div>

                      <div className="grid grid-cols-2 gap-2 pt-0.5">
                        <div>
                          <div className="flex items-center justify-between mb-0.5">
                            <label className="text-[11px] text-zinc-500">输入上下文 (Tokens):</label>
                            <div className="flex space-x-1">
                              <button
                                type="button"
                                onClick={() => setManualContextWindow(128000)}
                                className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                              >
                                128k
                              </button>
                              <button
                                type="button"
                                onClick={() => setManualContextWindow(200000)}
                                className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                              >
                                200k
                              </button>
                              <button
                                type="button"
                                onClick={() => setManualContextWindow(1000000)}
                                className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                              >
                                1M
                              </button>
                            </div>
                          </div>
                          <input
                            type="number"
                            value={manualContextWindow}
                            onChange={(e) => setManualContextWindow(Number(e.target.value) || 128000)}
                            className="w-full bg-white dark:bg-zinc-800 border border-zinc-300 dark:border-zinc-700 rounded px-2 py-1 text-xs font-mono outline-none"
                          />
                        </div>

                        <div>
                          <div className="flex items-center justify-between mb-0.5">
                            <label className="text-[11px] text-zinc-500">输出上下文 (Tokens):</label>
                            <div className="flex space-x-1">
                              <button
                                type="button"
                                onClick={() => setManualMaxOutputTokens(4096)}
                                className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                              >
                                4k
                              </button>
                              <button
                                type="button"
                                onClick={() => setManualMaxOutputTokens(8192)}
                                className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                              >
                                8k
                              </button>
                              <button
                                type="button"
                                onClick={() => setManualMaxOutputTokens(16384)}
                                className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                              >
                                16k
                              </button>
                              <button
                                type="button"
                                onClick={() => setManualMaxOutputTokens(65536)}
                                className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                              >
                                64k
                              </button>
                            </div>
                          </div>
                          <input
                            type="number"
                            value={manualMaxOutputTokens}
                            onChange={(e) => setManualMaxOutputTokens(Number(e.target.value) || 8192)}
                            className="w-full bg-white dark:bg-zinc-800 border border-zinc-300 dark:border-zinc-700 rounded px-2 py-1 text-xs font-mono outline-none"
                          />
                        </div>
                      </div>

                      <div className="flex items-center justify-between pt-1">
                        <label className="flex items-center space-x-1.5 cursor-pointer">
                          <input
                            type="checkbox"
                            checked={manualSupportsImages}
                            onChange={(e) => setManualSupportsImages(e.target.checked)}
                            className="accent-blue-600 rounded"
                          />
                          <span className="text-zinc-600 dark:text-zinc-400 text-xs flex items-center space-x-1">
                            <ImageIcon size={12} className="text-blue-500" />
                            <span>支持图像 (视觉多模态)</span>
                          </span>
                        </label>
                        <div className="flex items-center space-x-1.5">
                          <button
                            type="button"
                            onClick={() => setShowAddManualModel(false)}
                            className="px-2.5 py-1 rounded text-zinc-500 hover:bg-zinc-100 dark:hover:bg-zinc-800 text-xs"
                          >
                            取消
                          </button>
                          <button
                            type="button"
                            onClick={handleAddManualModel}
                            disabled={!manualModelId.trim()}
                            className="px-3 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white font-medium text-xs disabled:opacity-50 flex items-center space-x-1"
                          >
                            <Plus size={12} />
                            <span>确定添加</span>
                          </button>
                        </div>
                      </div>
                    </div>
                  )}

                  {/* 模型列表 */}
                  <div className="border border-zinc-200 dark:border-zinc-800 rounded-lg overflow-hidden bg-zinc-50/50 dark:bg-zinc-900/60 divide-y divide-zinc-100 dark:divide-zinc-800/80">
                    {editModels.map((m) => {
                      const isDefault = (editDefaultModel || editModels[0]?.id) === m.id
                      const isEditingThis = editingModelId === m.id && editingModelDraft

                      if (isEditingThis) {
                        return (
                          <div
                            key={m.id}
                            className="p-3 bg-blue-50/50 dark:bg-blue-950/20 border-l-4 border-l-blue-600 space-y-2 text-xs"
                          >
                            <div className="flex items-center justify-between">
                              <span className="font-semibold text-zinc-900 dark:text-zinc-100 flex items-center space-x-1.5">
                                <Edit2 size={12} className="text-blue-600" />
                                <span>编辑模型参数</span>
                              </span>
                              <span className="text-[11px] text-zinc-400 font-mono">原 ID: {m.id}</span>
                            </div>

                            <div className="grid grid-cols-2 gap-2">
                              <div>
                                <label className="block text-[11px] text-zinc-500 mb-0.5">模型 ID (必填):</label>
                                <input
                                  type="text"
                                  value={editingModelDraft.id}
                                  onChange={(e) =>
                                    setEditingModelDraft({ ...editingModelDraft, id: e.target.value })
                                  }
                                  className="w-full bg-white dark:bg-zinc-800 border border-zinc-300 dark:border-zinc-700 rounded px-2.5 py-1 text-xs outline-none font-mono"
                                />
                              </div>
                              <div>
                                <label className="block text-[11px] text-zinc-500 mb-0.5">显示名称 (可选):</label>
                                <input
                                  type="text"
                                  value={editingModelDraft.name || ''}
                                  onChange={(e) =>
                                    setEditingModelDraft({ ...editingModelDraft, name: e.target.value })
                                  }
                                  placeholder="显示名称 (可选)"
                                  className="w-full bg-white dark:bg-zinc-800 border border-zinc-300 dark:border-zinc-700 rounded px-2.5 py-1 text-xs outline-none"
                                />
                              </div>
                            </div>

                            <div className="grid grid-cols-2 gap-2">
                              <div>
                                <div className="flex items-center justify-between mb-0.5">
                                  <label className="text-[11px] text-zinc-500">输入上下文 (Tokens):</label>
                                  <div className="flex space-x-1">
                                    <button
                                      type="button"
                                      onClick={() =>
                                        setEditingModelDraft({ ...editingModelDraft, contextWindow: 128000 })
                                      }
                                      className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                                    >
                                      128k
                                    </button>
                                    <button
                                      type="button"
                                      onClick={() =>
                                        setEditingModelDraft({ ...editingModelDraft, contextWindow: 200000 })
                                      }
                                      className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                                    >
                                      200k
                                    </button>
                                    <button
                                      type="button"
                                      onClick={() =>
                                        setEditingModelDraft({ ...editingModelDraft, contextWindow: 1000000 })
                                      }
                                      className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                                    >
                                      1M
                                    </button>
                                  </div>
                                </div>
                                <input
                                  type="number"
                                  value={editingModelDraft.contextWindow || 128000}
                                  onChange={(e) =>
                                    setEditingModelDraft({
                                      ...editingModelDraft,
                                      contextWindow: Number(e.target.value) || 128000,
                                    })
                                  }
                                  className="w-full bg-white dark:bg-zinc-800 border border-zinc-300 dark:border-zinc-700 rounded px-2 py-1 text-xs font-mono outline-none"
                                />
                              </div>

                              <div>
                                <div className="flex items-center justify-between mb-0.5">
                                  <label className="text-[11px] text-zinc-500">输出上下文 (Tokens):</label>
                                  <div className="flex space-x-1">
                                    <button
                                      type="button"
                                      onClick={() =>
                                        setEditingModelDraft({ ...editingModelDraft, maxOutputTokens: 4096 })
                                      }
                                      className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                                    >
                                      4k
                                    </button>
                                    <button
                                      type="button"
                                      onClick={() =>
                                        setEditingModelDraft({ ...editingModelDraft, maxOutputTokens: 8192 })
                                      }
                                      className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                                    >
                                      8k
                                    </button>
                                    <button
                                      type="button"
                                      onClick={() =>
                                        setEditingModelDraft({ ...editingModelDraft, maxOutputTokens: 16384 })
                                      }
                                      className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                                    >
                                      16k
                                    </button>
                                    <button
                                      type="button"
                                      onClick={() =>
                                        setEditingModelDraft({ ...editingModelDraft, maxOutputTokens: 65536 })
                                      }
                                      className="text-[10px] px-1 py-0.2 bg-zinc-200 dark:bg-zinc-700 rounded hover:text-blue-500"
                                    >
                                      64k
                                    </button>
                                  </div>
                                </div>
                                <input
                                  type="number"
                                  value={editingModelDraft.maxOutputTokens || 8192}
                                  onChange={(e) =>
                                    setEditingModelDraft({
                                      ...editingModelDraft,
                                      maxOutputTokens: Number(e.target.value) || 8192,
                                    })
                                  }
                                  className="w-full bg-white dark:bg-zinc-800 border border-zinc-300 dark:border-zinc-700 rounded px-2 py-1 text-xs font-mono outline-none"
                                />
                              </div>
                            </div>

                            <div className="flex items-center justify-between pt-1">
                              <label className="flex items-center space-x-1.5 cursor-pointer">
                                <input
                                  type="checkbox"
                                  checked={editingModelDraft.supportsImages ?? true}
                                  onChange={(e) =>
                                    setEditingModelDraft({
                                      ...editingModelDraft,
                                      supportsImages: e.target.checked,
                                    })
                                  }
                                  className="accent-blue-600 rounded"
                                />
                                <span className="text-zinc-600 dark:text-zinc-400 text-xs flex items-center space-x-1">
                                  <ImageIcon size={12} className="text-blue-500" />
                                  <span>支持图像 (视觉多模态)</span>
                                </span>
                              </label>

                              <div className="flex items-center space-x-1.5">
                                <button
                                  type="button"
                                  onClick={handleCancelEditModel}
                                  className="px-2.5 py-1 rounded text-zinc-500 hover:bg-zinc-200 dark:hover:bg-zinc-800 text-xs"
                                >
                                  取消
                                </button>
                                <button
                                  type="button"
                                  onClick={handleSaveEditedModel}
                                  disabled={!editingModelDraft.id.trim()}
                                  className="px-3 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white font-medium text-xs disabled:opacity-50 flex items-center space-x-1"
                                >
                                  <Check size={12} />
                                  <span>保存修改</span>
                                </button>
                              </div>
                            </div>
                          </div>
                        )
                      }

                      return (
                        <div
                          key={m.id}
                          className="flex items-center justify-between px-3 py-2 text-xs hover:bg-white dark:hover:bg-zinc-800/40 transition-colors"
                        >
                          <div className="flex items-center space-x-2">
                            <button
                              type="button"
                              onClick={() => setEditDefaultModel(m.id)}
                              className={`w-3.5 h-3.5 rounded-full border flex items-center justify-center transition-all ${
                                isDefault
                                  ? 'border-blue-600 bg-blue-600 text-white'
                                  : 'border-zinc-300 dark:border-zinc-600 hover:border-blue-500'
                              }`}
                              title={isDefault ? '当前默认模型' : '点击设为默认'}
                            >
                              {isDefault && <Check size={10} />}
                            </button>
                            <span className="font-mono font-medium text-zinc-900 dark:text-zinc-100">{m.id}</span>
                            {m.name && m.name !== m.id && (
                              <span className="text-[11px] text-zinc-400">({m.name})</span>
                            )}
                            {isDefault && (
                              <span className="text-[9px] px-1.5 py-0.2 rounded bg-blue-100 dark:bg-blue-950 text-blue-700 dark:text-blue-300 font-medium">
                                默认
                              </span>
                            )}
                          </div>

                          <div className="flex items-center space-x-2.5 text-zinc-500 dark:text-zinc-400 text-[11px]">
                            <span className="px-1.5 py-0.5 rounded bg-zinc-100 dark:bg-zinc-800 font-mono text-[10px]">
                              入: {((m.contextWindow || 128000) / 1000).toFixed(0)}k
                            </span>
                            <span className="px-1.5 py-0.5 rounded bg-zinc-100 dark:bg-zinc-800 font-mono text-[10px]">
                              出: {((m.maxOutputTokens || 8192) / 1000).toFixed(0)}k
                            </span>
                            {m.supportsImages ? (
                              <span className="flex items-center space-x-0.5 text-blue-500" title="支持图像等多模态输入">
                                <ImageIcon size={11} />
                                <span>图像</span>
                              </span>
                            ) : (
                              <span className="text-zinc-400" title="仅支持纯文本">纯文本</span>
                            )}
                            <div className="flex items-center space-x-1 pl-1 border-l border-zinc-200 dark:border-zinc-800">
                              <button
                                type="button"
                                onClick={() => handleStartEditModel(m)}
                                className="p-1 text-zinc-400 hover:text-blue-500 transition-colors"
                                title="编辑模型参数 (输入/输出上下文、图像支持等)"
                              >
                                <Edit2 size={12} />
                              </button>
                              <button
                                type="button"
                                onClick={() => handleRemoveModel(m.id)}
                                className="p-1 text-zinc-400 hover:text-rose-500 transition-colors"
                                title="删除模型"
                              >
                                <Trash2 size={12} />
                              </button>
                            </div>
                          </div>
                        </div>
                      )
                    })}
                  </div>
                </div>

                {/* 连通性测试结果提示 */}
                {testResult && (
                  <div
                    className={`p-2.5 rounded-lg border flex items-center space-x-2 text-[11px] ${
                      testResult.success
                        ? 'bg-emerald-50 dark:bg-emerald-950/20 border-emerald-200 dark:border-emerald-500/30 text-emerald-700 dark:text-emerald-300'
                        : 'bg-rose-50 dark:bg-rose-950/20 border-rose-200 dark:border-rose-500/30 text-rose-700 dark:text-rose-300'
                    }`}
                  >
                    {testResult.success ? (
                      <Check size={13} className="text-emerald-500 flex-shrink-0" />
                    ) : (
                      <AlertCircle size={13} className="text-rose-500 flex-shrink-0" />
                    )}
                    <span>{testResult.message}</span>
                  </div>
                )}
              </div>
            </div>
          )}

          {activeTab === 'runtime' && (
            <div className="p-5 space-y-4 overflow-y-auto flex-1">
              {/* 安全审批策略选择 */}
              <div className="space-y-2">
                <label className="flex items-center space-x-1.5 text-zinc-800 dark:text-zinc-200 font-medium">
                  <ShieldCheck size={14} className="text-emerald-500 dark:text-emerald-400" />
                  <span>工具操作安全审批策略</span>
                </label>
                <div className="grid grid-cols-3 gap-2">
                  {[
                    { id: 'auto', label: '自动执行', desc: '全自动无阻塞运行' },
                    { id: 'ask', label: '询问确认', desc: '写操作前需授权确认' },
                    { id: 'readonly', label: '严格只读', desc: '完全禁止文件修改' },
                  ].map((item) => {
                    const isSelected = approvalMode === item.id
                    return (
                      <button
                        key={item.id}
                        type="button"
                        onClick={() => setApprovalMode(item.id as ApprovalMode)}
                        className={`p-2.5 rounded-xl border text-left transition-all ${
                          isSelected
                            ? 'border-emerald-500 bg-emerald-500/10 text-emerald-600 dark:text-emerald-300'
                            : 'border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-[#121214] text-zinc-700 dark:text-zinc-400 hover:border-zinc-300 dark:hover:border-zinc-700 hover:text-zinc-900 dark:hover:text-zinc-200'
                        }`}
                      >
                        <div className="font-medium text-xs text-zinc-900 dark:text-zinc-200">{item.label}</div>
                        <div className="text-[10px] text-zinc-500 mt-0.5">{item.desc}</div>
                      </button>
                    )
                  })}
                </div>
              </div>

              {/* 思考深度推演力度选择 */}
              <div className="space-y-2">
                <label className="flex items-center space-x-1.5 text-zinc-800 dark:text-zinc-200 font-medium">
                  <Cpu size={14} className="text-purple-500 dark:text-purple-400" />
                  <span>模型深度思考推演力度 (Reasoning Effort)</span>
                </label>
                <div className="grid grid-cols-4 gap-2">
                  {[
                    { id: 'low', label: '低 (Low)' },
                    { id: 'medium', label: '中 (Medium)' },
                    { id: 'high', label: '高 (High)' },
                    { id: 'max', label: '最大 (Max)' },
                  ].map((item) => {
                    const isSelected = effort === item.id
                    return (
                      <button
                        key={item.id}
                        type="button"
                        onClick={() => setEffort(item.id as Effort)}
                        className={`py-2 px-2 text-center rounded-lg border text-xs font-medium transition-all ${
                          isSelected
                            ? 'border-purple-500 bg-purple-500/10 text-purple-600 dark:text-purple-300'
                            : 'border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-[#121214] text-zinc-700 dark:text-zinc-400 hover:border-zinc-300 dark:hover:border-zinc-700 hover:text-zinc-900 dark:hover:text-zinc-200'
                        }`}
                      >
                        {item.label}
                      </button>
                    )
                  })}
                </div>
              </div>

              {/* 本地配置文件路径提示 */}
              <div className="pt-2 border-t border-zinc-200 dark:border-[#27272a] flex items-center space-x-1.5 text-zinc-500 text-[11px]">
                <FileText size={12} className="flex-shrink-0 text-zinc-400 dark:text-zinc-500" />
                <span className="truncate" title={configFilePath}>
                  配置文件：{configFilePath || '~/.a-da/config.json'}
                </span>
              </div>
            </div>
          )}
        </div>

        {/* 底部按钮区 */}
        <div className="flex items-center justify-between px-5 py-3 border-t border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#1e1e22]/50 flex-shrink-0">
          <div>
            {activeTab === 'provider' && (
              <button
                type="button"
                onClick={handleTestConnection}
                disabled={testing || !editBaseUrl}
                className="flex items-center space-x-1.5 px-3 py-1.5 rounded-lg border border-zinc-300 dark:border-zinc-700 bg-white dark:bg-zinc-800/80 hover:bg-zinc-100 dark:hover:bg-zinc-700 text-zinc-700 dark:text-zinc-300 transition-colors disabled:opacity-50"
              >
                {testing ? <Loader2 size={13} className="animate-spin text-blue-500" /> : <Server size={13} />}
                <span>{testing ? '正在测试连接...' : '测试当前供应商连通性'}</span>
              </button>
            )}
          </div>

          <div className="flex items-center space-x-2">
            <button
              onClick={onClose}
              className="px-3.5 py-1.5 rounded-lg border border-zinc-300 dark:border-zinc-700 hover:bg-zinc-100 dark:hover:bg-zinc-800 text-zinc-700 dark:text-zinc-300 transition-colors"
            >
              取消
            </button>
            <button
              onClick={() => handleSaveCurrentProvider(false)}
              className="px-4 py-1.5 rounded-lg border border-blue-600 text-blue-600 dark:text-blue-400 hover:bg-blue-50 dark:hover:bg-blue-950/40 font-medium transition-colors shadow-sm"
            >
              保存修改
            </button>
            <button
              onClick={() => handleSaveCurrentProvider(true)}
              className="flex items-center space-x-1.5 px-4 py-1.5 rounded-lg bg-blue-600 hover:bg-blue-500 text-white font-medium transition-colors shadow-sm"
            >
              {savedSuccess ? <Check size={13} /> : null}
              <span>{savedSuccess ? '已生效' : '保存并激活'}</span>
            </button>
          </div>
        </div>
      </div>

      {/* 删除供应商确认弹窗 */}
      <ConfirmModal
        isOpen={Boolean(deleteProviderTarget)}
        title="删除供应商配置"
        message={`确定要删除供应商「${deleteProviderTarget?.name || ''}」吗？`}
        subMessage="删除后该供应商下的所有模型配置将被清除，且不可恢复。"
        confirmText="确认删除"
        cancelText="取消"
        variant="danger"
        onConfirm={handleDeleteProviderConfirm}
        onCancel={() => setDeleteProviderTarget(null)}
      />
    </div>
  )
}
