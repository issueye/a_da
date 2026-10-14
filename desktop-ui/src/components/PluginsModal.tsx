import React, { useState, useEffect } from 'react'
import {
  X,
  Puzzle,
  Wrench,
  Sparkles,
  CheckCircle,
  FolderGit2,
  Globe,
  Shield,
  Sliders,
  Plus,
  Trash2,
  Check,
  AlertTriangle,
  Zap,
  Terminal,
  Settings,
  Clock,
  Eye,
  Edit3,
} from 'lucide-react'
import type {
  PluginItem,
  BuiltinToolInfo,
  ResolvedPluginCapabilitiesDto,
  SkillSummary,
  PluginCapabilities,
} from '../types'
import { agentClient } from '../client/ws-client'
import { ConfirmModal } from './ConfirmModal'

interface PluginsModalProps {
  isOpen: boolean
  onClose: () => void
}

type TabType = 'skills' | 'builtin-plugins' | 'workspace' | 'global' | 'builtins' | 'capabilities'

export const PluginsModal: React.FC<PluginsModalProps> = ({ isOpen, onClose }) => {
  const [activeTab, setActiveTab] = useState<TabType>('builtin-plugins')
  const [plugins, setPlugins] = useState<PluginItem[]>([])
  const [capabilities, setCapabilities] = useState<ResolvedPluginCapabilitiesDto | null>(null)
  const [pluginConfigs, setPluginConfigs] = useState<Record<string, Record<string, unknown>>>({})
  const [skills, setSkills] = useState<SkillSummary[]>([])
  const [builtinCatalog, setBuiltinCatalog] = useState<BuiltinToolInfo[]>([])
  const [loading, setLoading] = useState(false)
  const [actionNotice, setActionNotice] = useState<string | null>(null)
  const [deleteConfirmTarget, setDeleteConfirmTarget] = useState<PluginItem | null>(null)
  const [createTemplateScope, setCreateTemplateScope] = useState<'workspace' | 'global' | null>(null)

  // 插件配置编辑状态：pluginId -> { [key]: value }
  const [configDrafts, setConfigDrafts] = useState<Record<string, Record<string, string>>>({})
  // 能力开关编辑状态
  const [capsDraft, setCapsDraft] = useState<PluginCapabilities | null>(null)

  // 拉取插件中心所有数据
  const loadData = async () => {
    setLoading(true)
    try {
      const [pluginsRes, skillsRes, catalogRes] = await Promise.all([
        agentClient.fetchPlugins(),
        agentClient.fetchSkills(),
        agentClient.fetchBuiltinCatalog(),
      ])

      if (pluginsRes) {
        setPlugins(pluginsRes.plugins || [])
        setCapabilities(pluginsRes.capabilities || null)
        if (pluginsRes.capabilities?.capabilities) {
          setCapsDraft({ ...pluginsRes.capabilities.capabilities })
        }
        setPluginConfigs(pluginsRes.configs || {})
      }

      if (skillsRes) {
        setSkills(skillsRes)
      }

      if (catalogRes) {
        setBuiltinCatalog(catalogRes)
      }
    } catch (err) {
      console.warn('加载插件中心数据失败:', err)
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    if (isOpen) {
      loadData()
    }
  }, [isOpen])

  if (!isOpen) return null

  const showNotice = (msg: string) => {
    setActionNotice(msg)
    setTimeout(() => setActionNotice(null), 2500)
  }

  // 启停插件
  const handleTogglePlugin = async (item: PluginItem) => {
    const nextState = !item.enabled
    try {
      await agentClient.togglePlugin(item.id, nextState)
      setPlugins((prev) =>
        prev.map((p) => (p.id === item.id ? { ...p, enabled: nextState } : p))
      )
      showNotice(`${item.name} 已${nextState ? '启用' : '停用'}`)
    } catch (err: any) {
      showNotice(`切换失败: ${err.message}`)
    }
  }

  // 启停技能
  const handleToggleSkill = async (skill: SkillSummary) => {
    const nextState = !skill.enabled
    try {
      await agentClient.toggleSkill(skill.id, nextState)
      setSkills((prev) =>
        prev.map((s) => (s.id === skill.id ? { ...s, enabled: nextState } : s))
      )
      showNotice(`技能 ${skill.name} 已${nextState ? '启用' : '停用'}`)
    } catch (err: any) {
      showNotice(`技能切换失败: ${err.message}`)
    }
  }

  // 保存插件配置
  const handleSavePluginConfig = async (item: PluginItem) => {
    const drafts = configDrafts[item.id]
    if (!drafts) return
    try {
      await agentClient.savePluginConfig(item.id, drafts)
      showNotice(`${item.name} 配置已保存`)
      loadData()
    } catch (err: any) {
      showNotice(`保存配置失败: ${err.message}`)
    }
  }

  // 删除插件
  const handleDeletePlugin = (item: PluginItem) => {
    setDeleteConfirmTarget(item)
  }

  const handleDeletePluginConfirm = async () => {
    if (!deleteConfirmTarget) return
    try {
      await agentClient.deletePlugin(deleteConfirmTarget.filePath)
      showNotice(`插件 ${deleteConfirmTarget.name} 已成功删除`)
      loadData()
    } catch (err: any) {
      showNotice(`删除失败: ${err.message}`)
    } finally {
      setDeleteConfirmTarget(null)
    }
  }

  // 新建插件模板
  const handleCreateTemplate = (scope: 'workspace' | 'global') => {
    setCreateTemplateScope(scope)
  }

  const handleCreateTemplateConfirm = async (nameVal?: string) => {
    const scope = createTemplateScope
    if (!scope || !nameVal || !nameVal.trim()) return
    try {
      const res = await agentClient.createPluginTemplate({
        scope,
        name: nameVal.trim(),
      })
      showNotice(`创建插件模板成功: ${res?.filePath}`)
      loadData()
    } catch (err: any) {
      showNotice(`创建模板失败: ${err.message}`)
    } finally {
      setCreateTemplateScope(null)
    }
  }

  // 保存能力开关
  const handleSaveCapabilities = async () => {
    if (!capsDraft) return
    try {
      await agentClient.savePluginCapabilities(capsDraft)
      showNotice('能力安全开关已更新')
      loadData()
    } catch (err: any) {
      showNotice(`更新失败: ${err.message}`)
    }
  }

  const builtinPlugins = plugins.filter((p) => p.scope === 'builtin')
  const workspacePlugins = plugins.filter((p) => p.scope === 'workspace')
  const globalPlugins = plugins.filter((p) => p.scope === 'global')

  return (
    <div className="fixed inset-0 bg-black/40 dark:bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4 select-none">
      <div className="w-full max-w-4xl bg-white dark:bg-[#18181b] border border-zinc-200 dark:border-[#2e2e33] rounded-2xl shadow-2xl overflow-hidden flex flex-col text-xs h-[85vh] transition-colors">
        {/* 标题栏 */}
        <div className="flex items-center justify-between px-5 py-3.5 border-b border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#1e1e22]/50 flex-shrink-0">
          <div className="flex items-center space-x-2">
            <Puzzle size={16} className="text-blue-500 dark:text-blue-400" />
            <h3 className="font-semibold text-zinc-900 dark:text-zinc-100 text-sm">插件与能力中心</h3>
            <span className="text-[10px] text-zinc-600 dark:text-zinc-400 bg-zinc-200/80 dark:bg-zinc-800 px-2 py-0.5 rounded-full font-medium">
              共 {plugins.length} 扩展 · {skills.length} 技能
            </span>
          </div>
          <div className="flex items-center space-x-2">
            {actionNotice && (
              <span className="text-[11px] text-emerald-600 dark:text-emerald-400 font-medium px-2 py-0.5 rounded bg-emerald-50 dark:bg-emerald-950/40 border border-emerald-200 dark:border-emerald-500/20">
                {actionNotice}
              </span>
            )}
            <button
              onClick={onClose}
              className="p-1 text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-white hover:bg-zinc-100 dark:hover:bg-zinc-800 rounded-lg transition-colors"
            >
              <X size={15} />
            </button>
          </div>
        </div>

        {/* 顶部标签导航 */}
        <div className="flex border-b border-zinc-200 dark:border-[#27272a] bg-zinc-100/70 dark:bg-[#141416] px-4 pt-1 flex-shrink-0 overflow-x-auto">
          {[
            { id: 'builtin-plugins', label: '内置辅助插件', icon: Puzzle, count: builtinPlugins.length },
            { id: 'workspace', label: '工作区扩展', icon: FolderGit2, count: workspacePlugins.length },
            { id: 'global', label: '全局扩展', icon: Globe, count: globalPlugins.length },
            { id: 'skills', label: '技能库 (Skills)', icon: Zap, count: skills.length },
            { id: 'builtins', label: '内置核心工具', icon: Shield, count: builtinCatalog.length },
            { id: 'capabilities', label: '能力开关', icon: Sliders },
          ].map((tab) => {
            const IconComponent = tab.icon
            const isSelected = activeTab === tab.id
            return (
              <button
                key={tab.id}
                onClick={() => setActiveTab(tab.id as TabType)}
                className={`flex items-center space-x-1.5 px-3 py-2 font-medium border-b-2 whitespace-nowrap transition-colors text-xs ${
                  isSelected
                    ? 'border-blue-600 dark:border-blue-500 text-blue-600 dark:text-blue-400'
                    : 'border-transparent text-zinc-600 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200'
                }`}
              >
                <IconComponent size={13} />
                <span>{tab.label}</span>
                {tab.count !== undefined && (
                  <span className="text-[10px] bg-zinc-200/80 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 px-1.5 py-0.2 rounded-full font-medium">
                    {tab.count}
                  </span>
                )}
              </button>
            )
          })}
        </div>

        {/* 内容展示区 */}
        <div className="flex-1 p-5 overflow-y-auto space-y-4">
          {/* 内置辅助插件 */}
          {activeTab === 'builtin-plugins' && (
            <div className="space-y-3">
              <div className="text-[11px] text-zinc-600 dark:text-zinc-400">
                系统官方内置的标准辅助插件，提供版本控制、代码大纲分析、项目探查与准入门禁等开箱即用能力。
              </div>

              <div className="grid grid-cols-1 gap-2.5">
                {builtinPlugins.map((item) => (
                  <div
                    key={item.id}
                    className="p-3.5 rounded-xl border border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] hover:border-zinc-300 dark:hover:border-zinc-700 transition-colors flex flex-col space-y-2 shadow-sm dark:shadow-none"
                  >
                    <div className="flex items-center justify-between">
                      <div className="flex items-center space-x-2">
                        <Wrench size={14} className="text-blue-500 dark:text-blue-400" />
                        <span className="font-semibold text-zinc-900 dark:text-zinc-200">{item.name}</span>
                        <span className="font-mono text-[10px] text-zinc-500">{item.id}</span>
                      </div>

                      <button
                        onClick={() => handleTogglePlugin(item)}
                        className={`px-2.5 py-1 rounded-md text-[11px] font-medium transition-colors border ${
                          item.enabled
                            ? 'bg-emerald-50 dark:bg-emerald-950/30 border-emerald-300 dark:border-emerald-500/30 text-emerald-700 dark:text-emerald-300 hover:bg-emerald-100 dark:hover:bg-emerald-900/40'
                            : 'bg-zinc-200 dark:bg-zinc-800 border-zinc-300 dark:border-zinc-700 text-zinc-700 dark:text-zinc-400 hover:bg-zinc-300 dark:hover:bg-zinc-700'
                        }`}
                      >
                        {item.enabled ? '已激活' : '已停用'}
                      </button>
                    </div>

                    <div className="text-[11px] text-zinc-600 dark:text-zinc-400 leading-relaxed">
                      {item.plugin.manifest.description || '官方核心插件'}
                    </div>

                    {item.tools && item.tools.length > 0 && (
                      <div className="flex flex-wrap gap-1.5 pt-1">
                        {item.tools.map((t) => (
                          <span
                            key={t.name}
                            className="px-2 py-0.5 rounded bg-white dark:bg-zinc-800/80 border border-zinc-200 dark:border-zinc-700 text-[10px] font-mono text-zinc-700 dark:text-zinc-300 shadow-sm dark:shadow-none"
                            title={t.description}
                          >
                            {t.name}
                          </span>
                        ))}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* 工作区扩展 */}
          {activeTab === 'workspace' && (
            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <div className="text-[11px] text-zinc-600 dark:text-zinc-400">
                  当前工作区专属插件，存放于 <code>.ada/extensions/</code> 目录下，随项目版本库管理。
                </div>
                <button
                  onClick={() => handleCreateTemplate('workspace')}
                  className="flex items-center space-x-1.5 px-3 py-1.5 rounded-lg bg-blue-600 hover:bg-blue-500 text-white font-medium text-xs shadow-sm transition-colors"
                >
                  <Plus size={13} />
                  <span>新建工作区插件</span>
                </button>
              </div>

              {workspacePlugins.length === 0 ? (
                <div className="text-center py-12 border border-dashed border-zinc-300 dark:border-zinc-800 rounded-xl text-zinc-500">
                  当前工作区暂无自定义插件，点击右上角按钮即可一键生成扩展模板。
                </div>
              ) : (
                <div className="space-y-3">
                  {workspacePlugins.map((item) => (
                    <PluginDetailCard
                      key={item.id}
                      item={item}
                      configs={pluginConfigs[item.id] || {}}
                      onToggle={() => handleTogglePlugin(item)}
                      onDelete={() => handleDeletePlugin(item)}
                      onSaveConfig={() => handleSavePluginConfig(item)}
                      onDraftChange={(key, val) => {
                        setConfigDrafts((prev) => ({
                          ...prev,
                          [item.id]: {
                            ...(prev[item.id] || {}),
                            [key]: val,
                          },
                        }))
                      }}
                    />
                  ))}
                </div>
              )}
            </div>
          )}

          {/* 全局扩展 */}
          {activeTab === 'global' && (
            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <div className="text-[11px] text-zinc-600 dark:text-zinc-400">
                  用户级全局扩展，存放于 <code>~/.a-da/extensions/</code> 目录下，对所有工程通用。
                </div>
                <button
                  onClick={() => handleCreateTemplate('global')}
                  className="flex items-center space-x-1.5 px-3 py-1.5 rounded-lg bg-blue-600 hover:bg-blue-500 text-white font-medium text-xs shadow-sm transition-colors"
                >
                  <Plus size={13} />
                  <span>新建全局插件</span>
                </button>
              </div>

              {globalPlugins.length === 0 ? (
                <div className="text-center py-12 border border-dashed border-zinc-300 dark:border-zinc-800 rounded-xl text-zinc-500">
                  暂无全局插件，支持通过模板快速扩展自定义自动化工具。
                </div>
              ) : (
                <div className="space-y-3">
                  {globalPlugins.map((item) => (
                    <PluginDetailCard
                      key={item.id}
                      item={item}
                      configs={pluginConfigs[item.id] || {}}
                      onToggle={() => handleTogglePlugin(item)}
                      onDelete={() => handleDeletePlugin(item)}
                      onSaveConfig={() => handleSavePluginConfig(item)}
                      onDraftChange={(key, val) => {
                        setConfigDrafts((prev) => ({
                          ...prev,
                          [item.id]: {
                            ...(prev[item.id] || {}),
                            [key]: val,
                          },
                        }))
                      }}
                    />
                  ))}
                </div>
              )}
            </div>
          )}

          {/* 技能库 */}
          {activeTab === 'skills' && (
            <div className="space-y-3">
              <div className="text-[11px] text-zinc-600 dark:text-zinc-400">
                标准化专业技能与操作指引规范 (SKILL.md)，模型在执行复杂重构或特定指令时按需加载。
              </div>

              <div className="grid grid-cols-1 gap-2.5">
                {skills.map((skill) => (
                  <div
                    key={skill.id}
                    className="p-3.5 rounded-xl border border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] hover:border-zinc-300 dark:hover:border-zinc-700 transition-colors flex items-center justify-between shadow-sm dark:shadow-none"
                  >
                    <div className="space-y-1 max-w-[80%]">
                      <div className="flex items-center space-x-2">
                        <Zap size={14} className="text-amber-500 dark:text-amber-400" />
                        <span className="font-semibold text-zinc-900 dark:text-zinc-200">{skill.name}</span>
                        <span className="text-[10px] px-1.5 py-0.2 rounded bg-zinc-200 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 font-mono">
                          {skill.scope}
                        </span>
                      </div>
                      <div className="text-[11px] text-zinc-600 dark:text-zinc-400">{skill.description}</div>
                      <div className="text-[10px] text-zinc-400 dark:text-zinc-500 font-mono truncate">{skill.path}</div>
                    </div>

                    <button
                      onClick={() => handleToggleSkill(skill)}
                      className={`px-3 py-1 rounded-md text-[11px] font-medium transition-colors border ${
                        skill.enabled
                          ? 'bg-emerald-50 dark:bg-emerald-950/30 border-emerald-300 dark:border-emerald-500/30 text-emerald-700 dark:text-emerald-300 hover:bg-emerald-100 dark:hover:bg-emerald-900/40'
                          : 'bg-zinc-200 dark:bg-zinc-800 border-zinc-300 dark:border-zinc-700 text-zinc-700 dark:text-zinc-400 hover:bg-zinc-300 dark:hover:bg-zinc-700'
                      }`}
                    >
                      {skill.enabled ? '已启用' : '已停用'}
                    </button>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* 内置核心工具 */}
          {activeTab === 'builtins' && (
            <div className="space-y-3">
              <div className="text-[11px] text-zinc-600 dark:text-zinc-400">
                核心运行引擎提供的最底层系统级工具，包含代码文件读写、沙箱终端执行、任务清单管理与子智能体协作调度。
              </div>

              <div className="grid grid-cols-2 gap-2.5">
                {builtinCatalog.map((tool) => (
                  <div
                    key={tool.name}
                    className="p-3 rounded-xl border border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] hover:border-zinc-300 dark:hover:border-zinc-700 transition-colors flex flex-col justify-between shadow-sm dark:shadow-none"
                  >
                    <div className="space-y-1">
                      <div className="flex items-center justify-between">
                        <div className="flex items-center space-x-1.5 font-mono text-xs font-semibold text-zinc-800 dark:text-zinc-200">
                          <Terminal size={13} className="text-blue-500 dark:text-blue-400" />
                          <span>{tool.name}</span>
                        </div>
                        <span
                          className={`text-[10px] px-1.5 py-0.2 rounded font-medium border ${
                            tool.isReadOnly
                              ? 'bg-blue-50 dark:bg-blue-950/30 border-blue-200 dark:border-blue-500/30 text-blue-700 dark:text-blue-300'
                              : 'bg-amber-50 dark:bg-amber-950/30 border-amber-200 dark:border-amber-500/30 text-amber-700 dark:text-amber-300'
                          }`}
                        >
                          {tool.isReadOnly ? '只读' : '写入'}
                        </span>
                      </div>
                      <div className="text-[11px] font-medium text-zinc-700 dark:text-zinc-300">{tool.label}</div>
                      <div className="text-[10px] text-zinc-500 dark:text-zinc-400 leading-relaxed">{tool.description}</div>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* 能力安全开关 */}
          {activeTab === 'capabilities' && capsDraft && (
            <div className="space-y-4">
              <div className="text-[11px] text-zinc-600 dark:text-zinc-400">
                细粒度安全沙箱策略管控。对外部扩展拦截、系统提示词重写与钩子执行时限进行全方位准入限制。
              </div>

              <div className="space-y-2">
                {[
                  {
                    key: 'allowSystemPromptReplace',
                    label: '允许替换系统核心提示词',
                    desc: '第三方扩展可覆写默认 Prompt 指令',
                  },
                  {
                    key: 'allowTextRewrite',
                    label: '允许改写模型最终输出文本',
                    desc: '通过钩子对输出 Markdown 进行过滤或后处理',
                  },
                  {
                    key: 'allowThreadDeleteBlock',
                    label: '允许会话删除拦截保护',
                    desc: '阻止误删正在执行关键任务的会话',
                  },
                  {
                    key: 'allowCompactionReplace',
                    label: '允许自定义上下文压缩策略',
                    desc: '用扩展算法替换默认滑动窗口压缩算法',
                  },
                  {
                    key: 'allowPlanModeHooks',
                    label: '规划模式下执行插件决策钩子',
                    desc: '在只读 Plan 模式下允许运行安全的逻辑探测钩子',
                  },
                  {
                    key: 'allowThirdPartyHooks',
                    label: '放行第三方扩展代码决策点',
                    desc: '允许外部插件介入控制流重定向',
                  },
                  {
                    key: 'allowBuiltinShadow',
                    label: '允许插件同名覆盖内置工具',
                    desc: '当同名时优先使用工作区或全局自定义工具实现',
                  },
                ].map((item) => {
                  const val = Boolean((capsDraft as any)[item.key])
                  return (
                    <div
                      key={item.key}
                      onClick={() =>
                        setCapsDraft({
                          ...capsDraft,
                          [item.key]: !val,
                        })
                      }
                      className="p-3 rounded-xl border border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] hover:border-zinc-300 dark:hover:border-zinc-700 transition-colors flex items-center justify-between cursor-pointer shadow-sm dark:shadow-none"
                    >
                      <div className="space-y-0.5">
                        <div className="font-medium text-zinc-900 dark:text-zinc-200">{item.label}</div>
                        <div className="text-[11px] text-zinc-500 dark:text-zinc-400">{item.desc}</div>
                      </div>
                      <input
                        type="checkbox"
                        checked={val}
                        onChange={() => {}}
                        className="accent-blue-600 rounded cursor-pointer"
                      />
                    </div>
                  )
                })}

                {/* 钩子超时时间 */}
                <div className="p-3 rounded-xl border border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] flex items-center justify-between shadow-sm dark:shadow-none">
                  <div className="space-y-0.5">
                    <div className="font-medium text-zinc-900 dark:text-zinc-200">插件钩子执行超时时间 (毫秒)</div>
                    <div className="text-[11px] text-zinc-500 dark:text-zinc-400">超时未返回则自动熔断跳过</div>
                  </div>
                  <input
                    type="number"
                    value={capsDraft.hookTimeoutMs}
                    onChange={(e) =>
                      setCapsDraft({
                        ...capsDraft,
                        hookTimeoutMs: Number(e.target.value) || 500,
                      })
                    }
                    className="w-24 bg-white dark:bg-black/40 border border-zinc-300 dark:border-zinc-700 rounded-lg px-2.5 py-1 text-zinc-900 dark:text-zinc-200 font-mono text-xs text-right outline-none focus:border-blue-500"
                  />
                </div>
              </div>

              <div className="pt-2 flex justify-end">
                <button
                  onClick={handleSaveCapabilities}
                  className="flex items-center space-x-1.5 px-4 py-1.5 rounded-lg bg-blue-600 hover:bg-blue-500 text-white font-medium text-xs shadow-sm transition-colors"
                >
                  <Check size={13} />
                  <span>保存能力设置</span>
                </button>
              </div>
            </div>
          )}
        </div>

        {/* 底部信息栏 */}
        <div className="flex items-center justify-between px-5 py-3 border-t border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#1e1e22]/50 text-zinc-500 text-[11px] flex-shrink-0">
          <span>扩展目录兼容：.ada/extensions 与 ~/.a-da/extensions</span>
          <button
            onClick={onClose}
            className="px-4 py-1.5 rounded-lg bg-zinc-200 dark:bg-zinc-800 hover:bg-zinc-300 dark:hover:bg-zinc-700 text-zinc-800 dark:text-zinc-200 font-medium transition-colors"
          >
            完成
          </button>
        </div>
      </div>

      {/* 删除插件确认弹窗 */}
      <ConfirmModal
        isOpen={Boolean(deleteConfirmTarget)}
        title="删除插件"
        message={`确定要彻底删除插件「${deleteConfirmTarget?.name || ''}」吗？`}
        subMessage={`文件将被物理移除：${deleteConfirmTarget?.filePath || ''}`}
        confirmText="确认删除"
        cancelText="取消"
        variant="danger"
        onConfirm={handleDeletePluginConfirm}
        onCancel={() => setDeleteConfirmTarget(null)}
      />

      {/* 新建插件模板输入弹窗 */}
      <ConfirmModal
        isOpen={Boolean(createTemplateScope)}
        title="新建插件模板"
        message="请输入新插件名称（仅支持英文字母、数字与横线）："
        promptMode={true}
        promptDefaultValue="custom-tool"
        promptPlaceholder="custom-tool"
        confirmText="创建"
        cancelText="取消"
        variant="primary"
        onConfirm={handleCreateTemplateConfirm}
        onCancel={() => setCreateTemplateScope(null)}
      />
    </div>
  )
}

/** 插件卡片与参数配置子组件 */
const PluginDetailCard: React.FC<{
  item: PluginItem
  configs: Record<string, unknown>
  onToggle: () => void
  onDelete: () => void
  onSaveConfig: () => void
  onDraftChange: (key: string, val: string) => void
}> = ({ item, configs, onToggle, onDelete, onSaveConfig, onDraftChange }) => {
  const [editingConfig, setEditingConfig] = useState(false)
  const schemaProps = item.plugin.contributions.configSchema?.properties

  return (
    <div className="p-3.5 rounded-xl border border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] hover:border-zinc-300 dark:hover:border-zinc-700 transition-colors space-y-3 shadow-sm dark:shadow-none">
      <div className="flex items-center justify-between">
        <div className="flex items-center space-x-2">
          <Wrench size={14} className="text-blue-500 dark:text-blue-400" />
          <span className="font-semibold text-zinc-900 dark:text-zinc-200">{item.name}</span>
          <span className="text-[10px] text-zinc-500 font-mono">{item.fileName}</span>
          {item.status !== 'ready' && (
            <span className="px-1.5 py-0.2 rounded text-[10px] bg-amber-500/20 text-amber-700 dark:text-amber-300 border border-amber-500/30">
              {item.status}
            </span>
          )}
        </div>

        <div className="flex items-center space-x-2">
          {schemaProps && Object.keys(schemaProps).length > 0 && (
            <button
              onClick={() => setEditingConfig(!editingConfig)}
              className="p-1 text-zinc-500 dark:text-zinc-400 hover:text-zinc-900 dark:hover:text-zinc-200 hover:bg-zinc-200 dark:hover:bg-zinc-800 rounded transition-colors"
              title="配置项"
            >
              <Settings size={13} />
            </button>
          )}

          <button
            onClick={onDelete}
            className="p-1 text-zinc-400 dark:text-zinc-500 hover:text-rose-500 hover:bg-zinc-200 dark:hover:bg-zinc-800 rounded transition-colors"
            title="删除插件"
          >
            <Trash2 size={13} />
          </button>

          <button
            onClick={onToggle}
            className={`px-2.5 py-1 rounded-md text-[11px] font-medium transition-colors border ${
              item.enabled
                ? 'bg-emerald-50 dark:bg-emerald-950/30 border-emerald-300 dark:border-emerald-500/30 text-emerald-700 dark:text-emerald-300 hover:bg-emerald-100 dark:hover:bg-emerald-900/40'
                : 'bg-zinc-200 dark:bg-zinc-800 border-zinc-300 dark:border-zinc-700 text-zinc-700 dark:text-zinc-400 hover:bg-zinc-300 dark:hover:bg-zinc-700'
            }`}
          >
            {item.enabled ? '已启用' : '已停用'}
          </button>
        </div>
      </div>

      <div className="text-[11px] text-zinc-600 dark:text-zinc-400">{item.plugin.manifest.description || '自定义扩展插件'}</div>

      {item.tools && item.tools.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {item.tools.map((t) => (
            <span
              key={t.name}
              className="px-2 py-0.5 rounded bg-white dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700 text-[10px] font-mono text-zinc-700 dark:text-zinc-300 shadow-sm dark:shadow-none"
            >
              {t.name}
            </span>
          ))}
        </div>
      )}

      {/* 插件表单配置项 */}
      {editingConfig && schemaProps && (
        <div className="pt-2 border-t border-zinc-200 dark:border-zinc-800 space-y-2.5">
          <div className="text-[10px] font-semibold text-zinc-500 dark:text-zinc-400 uppercase tracking-wider">
            插件配置参数 (Config Schema)
          </div>

          {Object.entries(schemaProps).map(([key, prop]) => {
            const currentVal = String(configs[key] ?? prop.default ?? '')
            return (
              <div key={key} className="space-y-1">
                <label className="flex items-center justify-between text-[11px] text-zinc-700 dark:text-zinc-300">
                  <span>{prop.title || key}</span>
                  {prop.required && <span className="text-rose-500 text-[10px]">必填</span>}
                </label>
                <input
                  type={prop.type === 'secret' ? 'password' : 'text'}
                  defaultValue={currentVal}
                  onChange={(e) => onDraftChange(key, e.target.value)}
                  placeholder={prop.description || ''}
                  className="w-full bg-white dark:bg-[#1c1c20] border border-zinc-300 dark:border-zinc-700 rounded px-2.5 py-1 text-zinc-900 dark:text-zinc-100 text-xs outline-none focus:border-blue-500 font-mono shadow-sm dark:shadow-none"
                />
              </div>
            )
          })}

          <div className="flex justify-end pt-1">
            <button
              onClick={() => {
                onSaveConfig()
                setEditingConfig(false)
              }}
              className="px-3 py-1 bg-blue-600 hover:bg-blue-500 text-white rounded text-xs font-medium transition-colors"
            >
              保存参数
            </button>
          </div>
        </div>
      )}
    </div>
  )
}
