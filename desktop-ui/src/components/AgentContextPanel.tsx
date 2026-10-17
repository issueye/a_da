import React, { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Puzzle,
  RefreshCw,
  Sparkles,
  Wrench,
} from 'lucide-react'
import type { BuiltinToolInfo, PluginItem, SkillSummary } from '../types'
import { agentClient } from '../client/ws-client'

/**
 * 右侧「Agent 上下文」面板：当前 agent 真正加载的插件 / 工具 / 技能。
 *
 * 数据源（都是只读 RPC）：
 * - `plugin.list` —— 插件与它们的贡献；磁盘插件的贡献来自**装配期活体视图**
 *   （静态目录扫描恒为空，所以这里读到的就是引擎正在用的那份）。
 * - `plugin.builtinCatalog` —— 内置工具注册表。
 * - `skill.list` —— 技能（含插件技能与 `pluginName` 归因）。
 *
 * 🔴 口径：**插件工具优先于同名内置工具**（装配期就是"插件胜出"），
 * 所以内置那一栏要先把被插件重实现的同名工具摘掉，否则同一行会出现两次。
 */

const STORAGE_KEY = 'a_da_agent_context_open'

interface AgentContextPanelProps {
  workspace?: string
}

interface Snapshot {
  plugins: PluginItem[]
  tools: BuiltinToolInfo[]
  skills: SkillSummary[]
}

const scopeLabel = (scope?: string) =>
  scope === 'builtin' ? '内置' : scope === 'workspace' ? '工作区' : scope === 'global' ? '全局' : ''

const sectionTitle = 'flex items-center justify-between w-full px-3 py-1.5 text-[11px] font-medium text-zinc-500 dark:text-zinc-400 hover:text-zinc-800 dark:hover:text-zinc-200 cursor-pointer select-none'

const AgentContextPanel: React.FC<AgentContextPanelProps> = ({ workspace }) => {
  const [open, setOpen] = useState(() => localStorage.getItem(STORAGE_KEY) !== '0')
  const [snap, setSnap] = useState<Snapshot | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [updatedAt, setUpdatedAt] = useState('')
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({})

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [plugins, skills, tools] = await Promise.all([
        agentClient.fetchPlugins(workspace),
        agentClient.fetchSkills(workspace),
        agentClient.fetchBuiltinCatalog(),
      ])
      setSnap({
        plugins: plugins?.plugins || [],
        skills: skills || [],
        tools: tools || [],
      })
      setUpdatedAt(new Date().toLocaleTimeString('zh-CN', { hour12: false }))
    } catch (e: any) {
      setError(e?.message || '加载失败')
    } finally {
      setLoading(false)
    }
  }, [workspace])

  useEffect(() => {
    void load()
  }, [load])

  const toggleOpen = () =>
    setOpen((prev) => {
      localStorage.setItem(STORAGE_KEY, prev ? '0' : '1')
      return !prev
    })

  const toggleSection = (key: string) =>
    setCollapsed((prev) => ({ ...prev, [key]: !prev[key] }))

  const enabledPlugins = useMemo(() => (snap?.plugins || []).filter((p) => p.enabled), [snap])
  const disabledCount = useMemo(
    () => (snap?.plugins || []).filter((p) => !p.enabled).length,
    [snap]
  )

  /** 插件带来的工具（同名只算一次；装配期是"插件胜出"）。 */
  const pluginTools = useMemo(() => {
    const out: Array<{ name: string; isWrite: boolean; plugin: string }> = []
    const seen = new Set<string>()
    for (const p of enabledPlugins) {
      for (const t of p.tools || []) {
        if (seen.has(t.name)) continue
        seen.add(t.name)
        out.push({ name: t.name, isWrite: t.isWrite, plugin: p.name })
      }
    }
    return out
  }, [enabledPlugins])

  const builtinTools = useMemo(() => {
    const shadowed = new Set(pluginTools.map((t) => t.name))
    return (snap?.tools || []).filter((t) => !shadowed.has(t.name))
  }, [snap, pluginTools])

  const enabledSkills = useMemo(() => (snap?.skills || []).filter((s) => s.enabled), [snap])
  const toolCount = pluginTools.length + builtinTools.length

  if (!open) {
    return (
      <aside className="w-12 bg-zinc-50 dark:bg-[#121214] border-l border-zinc-200 dark:border-[#27272a] flex flex-col items-center py-2.5 gap-3 flex-shrink-0 select-none">
        <button
          type="button"
          onClick={toggleOpen}
          title="展开 Agent 上下文"
          className="p-1.5 rounded text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 cursor-pointer transition-colors"
        >
          <ChevronLeft size={14} />
        </button>
        <RailStat icon={<Puzzle size={13} />} value={enabledPlugins.length} title={`${enabledPlugins.length} 个已启用插件`} />
        <RailStat icon={<Wrench size={13} />} value={toolCount} title={`${toolCount} 个工具`} />
        <RailStat icon={<Sparkles size={13} />} value={enabledSkills.length} title={`${enabledSkills.length} 个已启用技能`} />
      </aside>
    )
  }

  return (
    <aside className="w-[264px] bg-zinc-50 dark:bg-[#121214] border-l border-zinc-200 dark:border-[#27272a] flex flex-col flex-shrink-0 select-none transition-colors duration-150">
      <div className="h-9 px-2.5 border-b border-zinc-200 dark:border-[#27272a] flex items-center justify-between flex-shrink-0">
        <div className="flex items-center gap-1.5 min-w-0">
          <span className="text-[11.5px] font-medium text-zinc-700 dark:text-zinc-200 truncate">
            Agent 上下文
          </span>
          <span className="text-[10px] text-zinc-400 dark:text-zinc-500 font-mono flex-shrink-0">
            {enabledPlugins.length}/{toolCount}/{enabledSkills.length}
          </span>
        </div>
        <div className="flex items-center gap-0.5">
          <button
            type="button"
            onClick={() => void load()}
            disabled={loading}
            title="刷新"
            className="p-1 rounded text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 cursor-pointer transition-colors disabled:opacity-50"
          >
            <RefreshCw size={12} className={loading ? 'animate-spin' : ''} />
          </button>
          <button
            type="button"
            onClick={toggleOpen}
            title="收起"
            className="p-1 rounded text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 cursor-pointer transition-colors"
          >
            <ChevronRight size={14} />
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto overflow-x-hidden py-1">
        {error && (
          <div className="mx-2.5 my-1.5 px-2 py-1.5 rounded border border-rose-200 dark:border-rose-900/60 bg-rose-50 dark:bg-rose-950/30 text-[11px] text-rose-600 dark:text-rose-300 flex items-start gap-1.5">
            <AlertTriangle size={12} className="flex-shrink-0 mt-0.5" />
            <span className="break-all">{error}</span>
          </div>
        )}

        {!snap && !error && (
          <div className="px-3 py-2 text-[11px] text-zinc-400 dark:text-zinc-500">
            {loading ? '正在读取装配结果…' : '暂无数据'}
          </div>
        )}

        {snap && (
          <>
            <Section
              title="插件"
              count={enabledPlugins.length}
              collapsed={!!collapsed.plugins}
              onToggle={() => toggleSection('plugins')}
            >
              {enabledPlugins.length === 0 && <Empty text="没有已启用的插件" />}
              {enabledPlugins.map((p) => (
                <PluginRow key={p.id} plugin={p} />
              ))}
              {disabledCount > 0 && (
                <div className="px-3 pt-1 pb-1.5 text-[10px] text-zinc-400 dark:text-zinc-500">
                  另有 {disabledCount} 个已停用（不参与装配）
                </div>
              )}
            </Section>

            <Section
              title="工具"
              count={toolCount}
              collapsed={!!collapsed.tools}
              onToggle={() => toggleSection('tools')}
            >
              {toolCount === 0 && <Empty text="没有可用的工具" />}
              {pluginTools.length > 0 && (
                <>
                  <SubTitle text={`插件工具 ${pluginTools.length}`} />
                  {pluginTools.map((t) => (
                    <ToolRow key={`p:${t.name}`} name={t.name} isWrite={t.isWrite} source={t.plugin} />
                  ))}
                </>
              )}
              {builtinTools.length > 0 && (
                <>
                  <SubTitle text={`内置工具 ${builtinTools.length}`} />
                  {builtinTools.map((t) => (
                    <ToolRow key={`b:${t.name}`} name={t.name} isWrite={!t.isReadOnly} source="内置" />
                  ))}
                </>
              )}
            </Section>

            <Section
              title="技能"
              count={enabledSkills.length}
              collapsed={!!collapsed.skills}
              onToggle={() => toggleSection('skills')}
            >
              {enabledSkills.length === 0 && <Empty text="没有已启用的技能" />}
              {enabledSkills.map((s) => (
                <SkillRow key={s.id} skill={s} />
              ))}
            </Section>
          </>
        )}
      </div>

      <div className="px-2.5 py-1.5 border-t border-zinc-200 dark:border-[#27272a] text-[10px] text-zinc-400 dark:text-zinc-500 flex items-center justify-between flex-shrink-0">
        <span className="truncate" title={workspace || ''}>
          {workspace ? workspace.split(/[\\/]/).filter(Boolean).pop() : '默认工作区'}
        </span>
        <span className="font-mono flex-shrink-0">{updatedAt || '—'}</span>
      </div>
    </aside>
  )
}

const RailStat: React.FC<{ icon: React.ReactNode; value: number; title: string }> = ({
  icon,
  value,
  title,
}) => (
  <div
    className="flex flex-col items-center gap-0.5 text-zinc-400 dark:text-zinc-500"
    title={title}
  >
    {icon}
    <span className="text-[9.5px] font-mono leading-none">{value}</span>
  </div>
)

const Section: React.FC<{
  title: string
  count: number
  collapsed: boolean
  onToggle: () => void
  children: React.ReactNode
}> = ({ title, count, collapsed, onToggle, children }) => (
  <div className="mb-0.5">
    <button type="button" className={sectionTitle} onClick={onToggle}>
      <span className="flex items-center gap-1">
        {collapsed ? <ChevronRight size={11} /> : <ChevronDown size={11} />}
        {title}
      </span>
      <span className="font-mono text-[10px] text-zinc-400 dark:text-zinc-500">{count}</span>
    </button>
    {!collapsed && <div className="pb-1">{children}</div>}
  </div>
)

const SubTitle: React.FC<{ text: string }> = ({ text }) => (
  <div className="px-3 pt-1 pb-0.5 text-[10px] text-zinc-400 dark:text-zinc-500">{text}</div>
)

const Empty: React.FC<{ text: string }> = ({ text }) => (
  <div className="px-3 py-1 text-[11px] text-zinc-400 dark:text-zinc-500">{text}</div>
)

const Badge: React.FC<{ children: React.ReactNode; tone?: 'amber' | 'zinc' | 'blue' }> = ({
  children,
  tone = 'zinc',
}) => {
  const cls =
    tone === 'amber'
      ? 'bg-amber-50 dark:bg-amber-950/40 text-amber-600 dark:text-amber-400 border-amber-200 dark:border-amber-900/60'
      : tone === 'blue'
      ? 'bg-blue-50 dark:bg-blue-950/40 text-blue-600 dark:text-blue-400 border-blue-200 dark:border-blue-900/60'
      : 'bg-zinc-100 dark:bg-zinc-800/60 text-zinc-500 dark:text-zinc-400 border-zinc-200 dark:border-zinc-700/60'
  return (
    <span className={`px-1 py-0 rounded text-[9px] leading-[14px] border flex-shrink-0 ${cls}`}>
      {children}
    </span>
  )
}

const PluginRow: React.FC<{ plugin: PluginItem }> = ({ plugin }) => {
  const bad = (plugin.diagnostics || []).filter(
    (d) => d.level === 'error' || d.level === 'warn'
  ).length
  const counts = [
    plugin.tools?.length ? `${plugin.tools.length} 工具` : '',
    plugin.skills?.length ? `${plugin.skills.length} 技能` : '',
    plugin.prompts?.length ? `${plugin.prompts.length} 提示词` : '',
    plugin.hooks?.length ? `${plugin.hooks.length} 钩子` : '',
  ]
    .filter(Boolean)
    .join(' · ')
  return (
    <div className="px-3 py-1 hover:bg-zinc-100/70 dark:hover:bg-zinc-800/40">
      <div className="flex items-center gap-1 min-w-0">
        <span className="text-[11.5px] text-zinc-700 dark:text-zinc-200 truncate flex-1" title={plugin.name}>
          {plugin.name}
        </span>
        {bad > 0 && (
          <span title={`${bad} 条诊断`} className="flex-shrink-0">
            <AlertTriangle size={11} className="text-amber-500" />
          </span>
        )}
        <Badge tone={plugin.scope === 'builtin' ? 'zinc' : 'blue'}>
          {scopeLabel(plugin.scope) || plugin.scope}
        </Badge>
      </div>
      <div className="text-[10px] text-zinc-400 dark:text-zinc-500 truncate">
        {counts || '无贡献'}
      </div>
    </div>
  )
}

const ToolRow: React.FC<{ name: string; isWrite: boolean; source: string }> = ({
  name,
  isWrite,
  source,
}) => (
  <div className="px-3 py-0.5 hover:bg-zinc-100/70 dark:hover:bg-zinc-800/40 flex items-center gap-1 min-w-0">
    <span
      className="font-mono text-[11px] text-zinc-700 dark:text-zinc-200 truncate flex-1"
      title={name}
    >
      {name}
    </span>
    {isWrite && <Badge tone="amber">写</Badge>}
    <span
      className="text-[9.5px] text-zinc-400 dark:text-zinc-500 truncate max-w-[72px] flex-shrink-0"
      title={source}
    >
      {source}
    </span>
  </div>
)

const SkillRow: React.FC<{ skill: SkillSummary }> = ({ skill }) => (
  <div className="px-3 py-0.5 hover:bg-zinc-100/70 dark:hover:bg-zinc-800/40 flex items-center gap-1 min-w-0">
    <span
      className="text-[11px] text-zinc-700 dark:text-zinc-200 truncate flex-1"
      title={skill.description || skill.name}
    >
      {skill.name}
    </span>
    {skill.unknownFrontmatterKeys && skill.unknownFrontmatterKeys.length > 0 && (
      <span title={`frontmatter 认不出的键：${skill.unknownFrontmatterKeys.join('、')}`}>
        <AlertTriangle size={11} className="text-amber-500" />
      </span>
    )}
    {skill.pluginName ? (
      <Badge tone="blue">{skill.pluginName}</Badge>
    ) : (
      <Badge>{scopeLabel(skill.scope) || skill.scope}</Badge>
    )}
  </div>
)

export { AgentContextPanel }
