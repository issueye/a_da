import React, { useState, useEffect } from 'react'
import {
  X,
  Puzzle,
  Wrench,
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
  RefreshCw,
  Store,
  Upload,
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
import { PluginDiagnostics } from './PluginDiagnostics'
import { PluginMarketplace } from './PluginMarketplace'
import { PublishPluginModal } from './PublishPluginModal'

interface PluginsModalProps {
  isOpen: boolean
  onClose: () => void
}

/**
 * 标签页（**五页，按"是什么"分**）。
 *
 * 早先是六页，其中「内置辅助插件 / 工作区扩展 / 全局扩展」是**同一类东西按来源**切开的三份，
 * 用户要在三个标签之间点来点去才知道自己到底有哪些扩展。现在它们合成一页
 * （来源做成**筛选条**），其余分别是插件市场、技能、核心工具、能力开关——
 * 一个标签对应一种**事物**，而不是同一件事物的一个切片。
 */
type TabType = 'extensions' | 'market' | 'skills' | 'builtins' | 'capabilities'

/** 扩展的来源筛选（三个插件页合并之后的那个筛选条）。 */
type ScopeFilter = 'all' | 'builtin' | 'workspace' | 'global'

/** 技能的来源筛选（与插件一栏同口径，另加 `plugin`：插件带来的技能）。 */
type SkillSourceFilter = 'all' | 'builtin' | 'workspace' | 'global' | 'plugin'

export const PluginsModal: React.FC<PluginsModalProps> = ({ isOpen, onClose }) => {
  const [activeTab, setActiveTab] = useState<TabType>('extensions')
  const [scopeFilter, setScopeFilter] = useState<ScopeFilter>('all')
  const [skillFilter, setSkillFilter] = useState<SkillSourceFilter>('all')
  const [plugins, setPlugins] = useState<PluginItem[]>([])
  const [capabilities, setCapabilities] = useState<ResolvedPluginCapabilitiesDto | null>(null)
  const [pluginConfigs, setPluginConfigs] = useState<Record<string, Record<string, unknown>>>({})
  const [skills, setSkills] = useState<SkillSummary[]>([])
  const [builtinCatalog, setBuiltinCatalog] = useState<BuiltinToolInfo[]>([])
  const [loading, setLoading] = useState(false)
  const [actionNotice, setActionNotice] = useState<string | null>(null)
  const [deleteConfirmTarget, setDeleteConfirmTarget] = useState<PluginItem | null>(null)
  /** 上传到 plugin-hub 的目标插件（内置插件没有磁盘包，不参与） */
  const [publishTarget, setPublishTarget] = useState<PluginItem | null>(null)
  /**
   * 新建插件草稿：**作用域在弹窗里选**（不再让用户在工具栏上先挑按钮再输名字）。
   *
   * 默认工作区：插件通常随项目走（`.ada/agent-coding/extensions`），要全局通用再切。
   */
  const [createDraft, setCreateDraft] = useState<{ scope: 'workspace' | 'global'; name: string } | null>(
    null,
  )

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
      const res = await agentClient.deletePlugin(deleteConfirmTarget.filePath)
      showNotice(
        res?.applied === true
          ? `插件 ${deleteConfirmTarget.name} 已删除并卸下`
          : res?.applied === false
            ? `插件 ${deleteConfirmTarget.name} 已删除，但重装配被拒：见「插件诊断」`
            : `插件 ${deleteConfirmTarget.name} 已删除（当前宿主未接重装配，重启后彻底卸下）`,
      )
      await loadData()
    } catch (err: any) {
      showNotice(`删除失败: ${err.message}`)
    } finally {
      setDeleteConfirmTarget(null)
    }
  }

  // 新建插件：打开草稿弹窗（作用域在弹窗里选）
  const handleCreateTemplate = (scope: 'workspace' | 'global' = 'workspace') => {
    setCreateDraft({ scope, name: 'custom-tool' })
  }

  const handleCreateTemplateConfirm = async () => {
    const draft = createDraft
    const name = draft?.name.trim()
    if (!draft || !name) return
    try {
      const res = await agentClient.createPluginTemplate({ scope: draft.scope, name })
      // applied 是"引擎侧真的换了"的唯一凭据：不给它下结论，用户就只能靠"重启试试"判断
      showNotice(
        res?.applied === true
          ? `已创建并生效：${res?.filePath}`
          : res?.applied === false
            ? `已创建，但装配被拒（见「插件诊断」）：${res?.filePath}`
            : `已创建：${res?.filePath}（当前宿主未接重装配，重启后生效）`,
      )
      await loadData()
    } catch (err: any) {
      showNotice(`创建模板失败: ${err.message}`)
    } finally {
      setCreateDraft(null)
    }
  }

  /**
   * 手动"重新加载"：重新扫盘 **+ 重装配**。
   *
   * 手改过 `index.ts` 的插件（或者从外部拖进目录的包）只能靠这个让引擎重新执行它；
   * 早先它只重扫目录，工具集合纹丝不动，于是"刷新了但没生效"。
   */
  const handleReloadPlugins = async () => {
    try {
      const res = await agentClient.reloadPlugins()
      showNotice(
        res?.applied === true
          ? `已重新加载 ${res?.plugins?.length || 0} 个扩展并生效`
          : `已重新扫描 ${res?.plugins?.length || 0} 个扩展（引擎侧未重装配，需重启）`,
      )
      await loadData()
    } catch (err: any) {
      showNotice(`重新加载失败: ${err.message}`)
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

  // ── 派生数据（一页一类事物，筛选只是同一份数据的视图）────────────────────
  const scopeCounts = {
    all: plugins.length,
    builtin: plugins.filter((p) => p.scope === 'builtin').length,
    workspace: plugins.filter((p) => p.scope === 'workspace').length,
    global: plugins.filter((p) => p.scope === 'global').length,
  }
  const visiblePlugins =
    scopeFilter === 'all' ? plugins : plugins.filter((p) => p.scope === scopeFilter)

  /** 技能来源：`plugin` 是"插件带来的"（口径与 `SkillSummary.pluginId` 一致）。 */
  const skillSourceOf = (s: SkillSummary): Exclude<SkillSourceFilter, 'all'> =>
    s.pluginId ? 'plugin' : (s.scope as 'builtin' | 'workspace' | 'global')
  const skillCounts = {
    all: skills.length,
    builtin: skills.filter((s) => skillSourceOf(s) === 'builtin').length,
    workspace: skills.filter((s) => skillSourceOf(s) === 'workspace').length,
    global: skills.filter((s) => skillSourceOf(s) === 'global').length,
    plugin: skills.filter((s) => skillSourceOf(s) === 'plugin').length,
  }
  const visibleSkills =
    skillFilter === 'all' ? skills : skills.filter((s) => skillSourceOf(s) === skillFilter)
  /** 启用技能的**正文**总字节：技能是**全量进提示词**的，这个数字直接对应每轮的固定开销。 */
  const enabledSkillBytes = skills
    .filter((s) => s.enabled)
    .reduce((sum, s) => sum + (s.body ? new Blob([s.body]).size : 0), 0)

  /**
   * **哪些核心工具被磁盘插件顶替了**（决策 1 的口径：插件只能重实现已注册的名字）。
   *
   * 只算**启用的磁盘插件**：内置辅助插件（`scope === 'builtin'`）的工具本来就是
   * 编译进来的实现，把它们标成"顶替"是错的——那会指向不存在的问题。
   */
  const replacedBuiltins = new Map<string, string>()
  for (const p of plugins) {
    if (!p.enabled || p.scope === 'builtin') continue
    for (const t of p.tools || []) {
      if (!replacedBuiltins.has(t.name)) replacedBuiltins.set(t.name, p.name)
    }
  }

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
            { id: 'extensions', label: '扩展', icon: Puzzle, count: plugins.length },
            { id: 'market', label: '插件市场', icon: Store },
            { id: 'skills', label: '技能库 (Skills)', icon: Zap, count: skills.length },
            { id: 'builtins', label: '核心工具', icon: Shield, count: builtinCatalog.length },
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
          {/* 扩展（合并了原先的三个标签：内置辅助插件 / 工作区扩展 / 全局扩展）
              —— 它们是同一类东西按来源切的三份，来源交给下面的筛选条。 */}
          {activeTab === 'extensions' && (
            <div className="space-y-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="text-[11px] text-zinc-600 dark:text-zinc-400">
                  {scopeFilter === 'builtin'
                    ? '系统官方内置的标准辅助插件，提供版本控制、代码大纲分析、项目探查与准入门禁等开箱即用能力。'
                    : scopeFilter === 'workspace'
                      ? '当前工作区专属插件，存放于 .ada/agent-coding/extensions/，随项目版本库管理。'
                      : scopeFilter === 'global'
                        ? '用户级全局扩展，存放于 ~/.a-da/agent-coding/extensions/，对所有工程通用。'
                        : '插件按来源分三类：内置（编译进来）、工作区（随项目走）、全局（对所有工程通用）。同一个工具名被插件重实现时，插件胜出。'}
                </div>
                <div className="flex items-center space-x-1.5">
                  <button
                    onClick={handleReloadPlugins}
                    title="重新扫描扩展目录，并让引擎重新装配贡献（手改过 index.ts 后点这个）"
                    className="flex items-center space-x-1 px-2.5 py-1.5 rounded-lg bg-zinc-200 dark:bg-zinc-800 hover:bg-zinc-300 dark:hover:bg-zinc-700 text-zinc-700 dark:text-zinc-200 font-medium text-[11px] border border-zinc-300 dark:border-zinc-700 transition-colors"
                  >
                    <RefreshCw size={12} />
                    <span>重新加载</span>
                  </button>
                  <button
                    onClick={() => handleCreateTemplate('workspace')}
                    className="flex items-center space-x-1 px-2.5 py-1.5 rounded-lg bg-blue-600 hover:bg-blue-500 text-white font-medium text-[11px] shadow-sm transition-colors"
                  >
                    <Plus size={12} />
                    <span>新建插件</span>
                  </button>
                </div>
              </div>

              {/* 来源筛选：三个标签合成的就是这个 */}
              <div className="flex items-center flex-wrap gap-1">
                <span className="text-[10px] text-zinc-400 mr-0.5">来源</span>
                {(
                  [
                    ['all', '全部', scopeCounts.all],
                    ['builtin', '内置', scopeCounts.builtin],
                    ['workspace', '工作区', scopeCounts.workspace],
                    ['global', '全局', scopeCounts.global],
                  ] as const
                ).map(([value, label, count]) => (
                  <button
                    key={value}
                    onClick={() => setScopeFilter(value)}
                    className={`px-2 py-0.5 rounded-md text-[11px] transition-colors ${
                      scopeFilter === value
                        ? 'bg-blue-600 text-white'
                        : 'bg-zinc-100 dark:bg-zinc-800/70 text-zinc-600 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700'
                    }`}
                  >
                    {label} <span className="font-semibold">{count}</span>
                  </button>
                ))}
              </div>

              {visiblePlugins.length === 0 ? (
                <div className="text-center py-12 border border-dashed border-zinc-300 dark:border-zinc-800 rounded-xl text-zinc-500">
                  {scopeFilter === 'all'
                    ? '还没有任何扩展。用右上角的按钮生成一个插件模板即可开始。'
                    : scopeFilter === 'workspace'
                      ? '当前工作区暂无自定义插件，点击右上角「新建工作区插件」生成模板。'
                      : scopeFilter === 'global'
                        ? '暂无全局插件，点击右上角「新建全局插件」生成模板。'
                        : '没有内置辅助插件。'}
                </div>
              ) : (
                <div className="space-y-3">
                  {visiblePlugins.map((item) => (
                    <PluginDetailCard
                      key={item.id}
                      item={item}
                      configs={pluginConfigs[item.id] || {}}
                      // 该插件带来的技能条数：与「技能库」里那条 `pluginName` 徽标是同一件事，
                      // 放在这里是为了"这个扩展到底给我加了什么"能一眼看全。
                      skillCount={skills.filter((s) => s.pluginId === item.id).length}
                      onToggle={() => handleTogglePlugin(item)}
                      // 内置插件是**编译进来的**，`filePath` 只是 `(builtin):<id>` 这种占位串，
                      // 走删除只会"报成功但什么也没删"。这类卡片不给删除按钮。
                      onDelete={item.scope === 'builtin' ? undefined : () => handleDeletePlugin(item)}
                      onPublish={item.scope === 'builtin' ? undefined : () => setPublishTarget(item)}
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

          {/* 插件市场：对接 plugin-hub（地址 / Token 在这里配置） */}
          {activeTab === 'market' && (
            <PluginMarketplace
              installed={plugins}
              onInstalled={loadData}
              onNotice={showNotice}
            />
          )}

          {/* 技能库 */}
          {activeTab === 'skills' && (
            <div className="space-y-3">
              <div className="text-[11px] text-zinc-600 dark:text-zinc-400">
                标准化专业技能与操作指引规范（SKILL.md）。技能只从<span className="font-semibold">两处</span>目录扫描：
                <code className="mx-1 font-mono text-[10px]">.ada/agent-coding/skills/</code>（工作区，随项目走）与
                <code className="mx-1 font-mono text-[10px]">~/.a-da/agent-coding/skills/</code>（全局）；
                插件自带的技能在插件包的 <code className="font-mono text-[10px]">skills/&lt;name&gt;/SKILL.md</code>。
                技能正文**全量进系统提示词**，所以下面那行字节数就是"每轮固定开销"的一部分——停用不用的技能能省回来。
              </div>

              {/* 来源筛选与「扩展」页同口径（目录 / 插件 / 内置）；插件技能另有插件名徽标 */}
              <div className="flex items-center flex-wrap gap-1">
                <span className="text-[10px] text-zinc-400 mr-0.5">来源</span>
                {(
                  [
                    ['all', '全部', skillCounts.all],
                    ['builtin', '内置', skillCounts.builtin],
                    ['workspace', '工作区', skillCounts.workspace],
                    ['global', '全局', skillCounts.global],
                    ['plugin', '插件', skillCounts.plugin],
                  ] as const
                ).map(([value, label, count]) => (
                  <button
                    key={value}
                    onClick={() => setSkillFilter(value)}
                    className={`px-2 py-0.5 rounded-md text-[11px] transition-colors ${
                      skillFilter === value
                        ? 'bg-blue-600 text-white'
                        : 'bg-zinc-100 dark:bg-zinc-800/70 text-zinc-600 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700'
                    }`}
                  >
                    {label} <span className="font-semibold">{count}</span>
                  </button>
                ))}
              </div>
              <div className="text-[10px] text-zinc-500 font-mono">
                启用 {skills.filter((s) => s.enabled).length} / 共 {skills.length} 条 · 启用正文合计{' '}
                {(enabledSkillBytes / 1024).toFixed(1)} KB
              </div>

              {visibleSkills.length === 0 ? (
                <div className="text-center py-12 border border-dashed border-zinc-300 dark:border-zinc-800 rounded-xl text-zinc-500">
                  {skillFilter === 'plugin'
                    ? '当前没有插件带来的技能（插件用 api.registerSkill 声明，见「扩展」页）。'
                    : '这一类暂时没有技能。'}
                </div>
              ) : (
                <div className="grid grid-cols-1 gap-2.5">
                {visibleSkills.map((skill) => (
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
                        {/* SP-45：插件技能要说清是哪个插件带来的——否则用户看到一条
                            "来源不明"的技能，停用与排查都无从下手 */}
                        {skill.pluginName && (
                          <span className="text-[10px] px-1.5 py-0.2 rounded bg-indigo-100 dark:bg-indigo-950/40 text-indigo-700 dark:text-indigo-300">
                            {skill.pluginName}
                          </span>
                        )}
                      </div>
                      <div className="text-[11px] text-zinc-600 dark:text-zinc-400">{skill.description}</div>
                      <div className="text-[10px] text-zinc-400 dark:text-zinc-500 font-mono truncate">{skill.path}</div>
                      {/* SP-45：frontmatter 写错的键以前被静默忽略，作者完全看不出问题出在键名上 */}
                      {skill.unknownFrontmatterKeys && skill.unknownFrontmatterKeys.length > 0 && (
                        <div className="text-[10px] text-amber-600 dark:text-amber-400">
                          认不出的 frontmatter 键：{skill.unknownFrontmatterKeys.join('、')}（这些设置不会生效）
                        </div>
                      )}
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
              )}
            </div>
          )}

          {/* 核心工具 */}
          {activeTab === 'builtins' && (
            <div className="space-y-3">
              <div className="text-[11px] text-zinc-600 dark:text-zinc-400">
                核心运行引擎提供的最底层系统级工具，包含代码文件读写、沙箱终端执行、任务清单管理与子智能体协作调度。
                这里列出的是**注册表里的名字**——插件要提供工具只能用这些名字（重实现），所以下面会标出谁被顶替了。
              </div>

              <div className="text-[10px] text-zinc-500 font-mono">
                共 {builtinCatalog.length} 个 · 只读 {builtinCatalog.filter((t) => t.isReadOnly).length} ·
                写入 {builtinCatalog.filter((t) => !t.isReadOnly).length}
                {replacedBuiltins.size > 0 && (
                  <span className="text-amber-600 dark:text-amber-400">
                    {' '}· 被插件重实现 {replacedBuiltins.size} 个
                  </span>
                )}
              </div>

              <div className="grid grid-cols-2 gap-2.5">
                {builtinCatalog.map((tool) => {
                  const replacedBy = replacedBuiltins.get(tool.name)
                  return (
                    <div
                      key={tool.name}
                      className={`p-3 rounded-xl border bg-zinc-50 dark:bg-[#141416] hover:border-zinc-300 dark:hover:border-zinc-700 transition-colors flex flex-col justify-between shadow-sm dark:shadow-none ${
                        replacedBy
                          ? 'border-amber-300 dark:border-amber-500/40'
                          : 'border-zinc-200 dark:border-[#27272a]'
                      }`}
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
                        {/* 交叉引用：这个内置实现现在跑的是插件的（决策 1：插件只能重实现已注册的名字） */}
                        {replacedBy && (
                          <div className="text-[10px] text-amber-600 dark:text-amber-400 pt-0.5">
                            ⚠ 已被插件「{replacedBy}」重实现（停用那个插件即恢复内置实现）
                          </div>
                        )}
                      </div>
                    </div>
                  )
                })}
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
                    label: '允许插件向系统提示词加内容',
                    desc: '插件声明的提示词片段会进入系统提示词；关掉即不注入（被挡的片段会记进诊断）',
                  },
                  {
                    key: 'allowTextRewrite',
                    label: '允许钩子追加旁注（note）',
                    desc: '插件可在工具结果后补一句说明（会进模型上下文）；关掉后这类旁注不再生效',
                  },
                  {
                    key: 'allowThreadDeleteBlock',
                    label: '允许插件拦下会话删除',
                    desc: '插件可在删除前拦下；关掉后任何人都不能靠插件保住一条会话',
                  },
                  {
                    key: 'allowCompactionReplace',
                    label: '允许插件定制上下文压缩',
                    desc: '插件可拦下或改写压缩；关掉后压缩一律走核心默认实现',
                  },
                  {
                    key: 'allowThirdPartyHooks',
                    label: '放行第三方插件的钩子',
                    desc: '关掉后第三方插件的钩子一条都不装（装配期即拒，理由可见）',
                  },
                  {
                    key: 'allowBuiltinShadow',
                    label: '允许插件同名覆盖内置工具',
                    desc: '关掉后同名插件工具不注册，保留内置实现（被挡的工具会记进诊断）',
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
          {/* 两条路径都说全：扫描目录是 `<ws>/.ada/{extensions,plugins}` 与 `~/.a-da/{extensions,plugins}`
              （`plugins/manager.rs::scan_plugins`），只写一半会让人把插件放进另一个目录里等它生效。 */}
          <span>扩展目录：.ada/extensions · .ada/plugins · ~/.a-da/extensions · ~/.a-da/plugins</span>
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

      {/* 上传插件到 plugin-hub（发布包 + 新版本） */}
      <PublishPluginModal
        isOpen={Boolean(publishTarget)}
        plugin={publishTarget}
        onClose={() => setPublishTarget(null)}
        onNotice={showNotice}
        onPublished={() => loadData()}
      />

      {/* 新建插件弹窗：**作用域在这里选**（不再让用户先挑一个按钮再输名字） */}
      {createDraft && (
        <div
          className="fixed inset-0 bg-black/40 dark:bg-black/60 backdrop-blur-sm z-[60] flex items-center justify-center p-4 select-none"
          onClick={() => setCreateDraft(null)}
        >
          <div
            className="w-full max-w-[440px] bg-white dark:bg-[#18181b] border border-zinc-200 dark:border-[#2e2e33] rounded-2xl shadow-2xl overflow-hidden flex flex-col text-xs"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between px-5 pt-4 pb-2">
              <div className="flex items-center space-x-2.5">
                <div className="p-1.5 rounded-xl border bg-blue-50 dark:bg-blue-950/40 border-blue-200/80 dark:border-blue-900/40">
                  <Puzzle size={18} className="text-blue-500 dark:text-blue-400" />
                </div>
                <h3 className="font-semibold text-zinc-900 dark:text-zinc-100 text-sm">新建插件</h3>
              </div>
              <button
                onClick={() => setCreateDraft(null)}
                className="p-1 text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200 rounded-lg transition-colors"
              >
                <X size={15} />
              </button>
            </div>

            <div className="px-5 py-3 space-y-3">
              <div>
                <div className="text-[11px] text-zinc-500 dark:text-zinc-400 mb-1.5">
                  插件名称（字母、数字、点、下划线、连字符；目录名就是插件标识）
                </div>
                <input
                  autoFocus
                  value={createDraft.name}
                  onChange={(e) => setCreateDraft({ ...createDraft, name: e.target.value })}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') handleCreateTemplateConfirm()
                  }}
                  placeholder="custom-tool"
                  className="w-full px-3 py-2 bg-zinc-50 dark:bg-zinc-900/70 border border-zinc-200 dark:border-[#2e2e33] rounded-xl text-xs text-zinc-900 dark:text-zinc-100 placeholder-zinc-400 focus:outline-none focus:ring-1 focus:ring-blue-500"
                />
              </div>

              <div>
                <div className="text-[11px] text-zinc-500 dark:text-zinc-400 mb-1.5">创建位置</div>
                <div className="grid grid-cols-2 gap-2">
                  {(
                    [
                      ['workspace', '当前工作区', '.ada/agent-coding/extensions/', '随项目版本库管理'],
                      ['global', '全局', '~/.a-da/agent-coding/extensions/', '对所有工程通用'],
                    ] as const
                  ).map(([value, label, path, hint]) => (
                    <button
                      key={value}
                      onClick={() => setCreateDraft({ ...createDraft, scope: value })}
                      className={`text-left p-2.5 rounded-xl border transition-colors ${
                        createDraft.scope === value
                          ? 'border-blue-500 bg-blue-50 dark:bg-blue-950/30'
                          : 'border-zinc-200 dark:border-[#2e2e33] bg-zinc-50 dark:bg-[#141416] hover:border-zinc-300 dark:hover:border-zinc-700'
                      }`}
                    >
                      <div className="font-medium text-zinc-900 dark:text-zinc-200 flex items-center justify-between">
                        <span>{label}</span>
                        {createDraft.scope === value && <Check size={12} className="text-blue-500" />}
                      </div>
                      <div className="text-[10px] text-zinc-500 dark:text-zinc-400 font-mono mt-0.5 break-all">
                        {path}
                      </div>
                      <div className="text-[10px] text-zinc-400 dark:text-zinc-500 mt-0.5">{hint}</div>
                    </button>
                  ))}
                </div>
              </div>

              <div className="text-[10px] text-zinc-500 dark:text-zinc-400 leading-relaxed">
                创建后宿主会<span className="font-semibold">立刻重装配</span>，新插件的工具与技能无需重启即可生效；
                之后手改 <code className="font-mono">index.ts</code> 请点「重新加载」。
              </div>
            </div>

            <div className="flex items-center justify-end space-x-2 px-5 py-3.5 bg-zinc-50/70 dark:bg-[#151518] border-t border-zinc-100 dark:border-[#232326]">
              <button
                onClick={() => setCreateDraft(null)}
                className="px-3.5 py-1.5 rounded-xl border border-zinc-200 dark:border-[#2e2e33] text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 text-xs font-medium transition-colors"
              >
                取消
              </button>
              <button
                onClick={handleCreateTemplateConfirm}
                disabled={!createDraft.name.trim()}
                className="px-4 py-1.5 rounded-xl text-xs font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed bg-blue-600 hover:bg-blue-700 text-white shadow-sm"
              >
                创建
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

/** 插件卡片与参数配置子组件 */
const PluginDetailCard: React.FC<{
  item: PluginItem
  configs: Record<string, unknown>
  /** 这个插件带来的**技能**条数（与「技能库」里那条 `pluginName` 徽标是同一件事）。 */
  skillCount?: number
  onToggle: () => void
  /** 省略 = 不可删（内置插件）；只有磁盘上的插件才有东西可删。 */
  onDelete?: () => void
  /** 省略 = 不可发布（内置插件没有磁盘包）。 */
  onPublish?: () => void
  onSaveConfig: () => void
  onDraftChange: (key: string, val: string) => void
}> = ({ item, configs, skillCount = 0, onToggle, onDelete, onPublish, onSaveConfig, onDraftChange }) => {
  const [editingConfig, setEditingConfig] = useState(false)
  const schemaProps = item.plugin.contributions.configSchema?.properties

  return (
    <div className="p-3.5 rounded-xl border border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416] hover:border-zinc-300 dark:hover:border-zinc-700 transition-colors space-y-3 shadow-sm dark:shadow-none">
      <div className="flex items-center justify-between">
        <div className="flex items-center space-x-2">
          <Wrench size={14} className="text-blue-500 dark:text-blue-400" />
          <span className="font-semibold text-zinc-900 dark:text-zinc-200">{item.name}</span>
          {/* 来源徽标：一页里混了三种来源，没有它就分不出"这个扩展是谁给的、去哪儿找它"。
              内置插件不显示 `.ts` 文件名——那是编译进来的，磁盘上并没有那个文件。 */}
          <span
            className={`px-1.5 py-0.2 rounded text-[10px] border ${
              item.scope === 'builtin'
                ? 'bg-zinc-100 dark:bg-zinc-800 border-zinc-200 dark:border-zinc-700 text-zinc-500'
                : item.scope === 'workspace'
                  ? 'bg-blue-50 dark:bg-blue-950/30 border-blue-200 dark:border-blue-500/30 text-blue-700 dark:text-blue-300'
                  : 'bg-violet-50 dark:bg-violet-950/30 border-violet-200 dark:border-violet-500/30 text-violet-700 dark:text-violet-300'
            }`}
          >
            {item.scope === 'builtin' ? '内置' : item.scope === 'workspace' ? '工作区' : '全局'}
          </span>
          {item.scope !== 'builtin' && (
            <span className="text-[10px] text-zinc-500 font-mono">{item.fileName}</span>
          )}
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

          {onPublish && (
            <button
              onClick={onPublish}
              className="p-1 text-zinc-400 dark:text-zinc-500 hover:text-blue-500 hover:bg-zinc-200 dark:hover:bg-zinc-800 rounded transition-colors"
              title="上传到插件库（plugin-hub）"
            >
              <Upload size={13} />
            </button>
          )}

          {onDelete && (
            <button
              onClick={onDelete}
              className="p-1 text-zinc-400 dark:text-zinc-500 hover:text-rose-500 hover:bg-zinc-200 dark:hover:bg-zinc-800 rounded transition-colors"
              title="删除插件"
            >
              <Trash2 size={13} />
            </button>
          )}

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

      {/* 这个扩展到底带来了什么：工具 / 技能 / 提示词片段三类一起摆出来，
          省得用户去「技能库」「核心工具」两页自己拼。 */}
      {((item.tools && item.tools.length > 0) || skillCount > 0 || (item.prompts && item.prompts.length > 0) || (item.hooks && item.hooks.length > 0)) && (
        <div className="flex items-center flex-wrap gap-x-3 gap-y-0.5 text-[10px] text-zinc-500 dark:text-zinc-400">
          <span>贡献：</span>
          <span>工具 {item.tools?.length ?? 0}</span>
          <span>技能 {skillCount}</span>
          <span>提示词片段 {item.prompts?.length ?? 0}</span>
          <span>钩子点位 {item.hooks?.length ?? 0}</span>
        </div>
      )}

      {/* P4：这个插件**会动手做哪些事**——只列真的注册上的点位（分级/开关挡下的不在里面） */}
      {item.hooks && item.hooks.length > 0 && (
        <div className="flex items-center flex-wrap gap-1.5">
          <span className="text-[10px] text-zinc-500 dark:text-zinc-400">钩子：</span>
          {item.hooks.map((point) => (
            <span
              key={point}
              className="px-2 py-0.5 rounded bg-white dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700 text-[10px] font-mono text-zinc-700 dark:text-zinc-300 shadow-sm dark:shadow-none"
            >
              {point}
            </span>
          ))}
        </div>
      )}

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

      {/* SP-74：插件自述的诊断（加载失败 / 已停用 / 工具清单未经执行验证 / 越级声明…） */}
      <PluginDiagnostics diagnostics={item.diagnostics} />

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
