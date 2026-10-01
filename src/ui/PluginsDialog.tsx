/**
 * 插件管理模态弹窗组件
 * 采用全屏遮罩居中卡片布局，支持工作区插件与全局插件的扫描、启用/停用、新建模板、删除以及内置核心工具一览。
 */

import React, { useEffect, useState } from 'react'
import type { AgentClient } from './client'
import {
  DEFAULT_PLUGIN_CAPABILITIES,
  type BuiltinToolInfo,
  type PluginCapabilities,
  type PluginItem,
  type PromptItem,
  type SkillSummary,
  type SubagentProfile,
} from './client'
import { SUBAGENT_HEX_COLORS } from '../agent/subagents/types'
import { SkillsPanel } from './SkillsPanel'
import {
  capabilityToggleNotice,
  pluginToggleNotice,
  promptToggleFailedNotice,
  promptToggleNotice,
  subagentToggleNotice,
  type ActionNotice,
} from './action-notices'
import { copyToClipboard } from '../platform/clipboard'
import { C, docTheme, editorTheme, FONT_MONO, M } from '../theme'
import { Icon, IconButton } from './controls'
import type { PluginStatus } from '../agent/plugins/types'
import {
  CAPABILITY_SWITCHES,
  describePluginRestrictions,
  parseHookTimeout,
} from '../agent/plugins/capabilities-view'

import type { IconName } from '../icons'
import { join } from 'node:path'

/**
 * 插件状态徽标（M3-1）。
 *
 * 只给"不是就绪"的状态做徽标——每个正常插件卡上都挂一个"就绪"纯属噪音。
 * `conflict` 单列一种颜色：它不致命（后注册者生效），但会让模型看到的名字与
 * 预期不符，属于"能用但要你知道"。
 */
const PLUGIN_STATUS_BADGE: Partial<Record<PluginStatus, { label: string; color: string; background: string }>> = {
  'not-ready': { label: '待配置', color: '#b45309', background: '#f59e0b18' },
  incompatible: { label: '版本不兼容', color: '#b45309', background: '#f59e0b18' },
  broken: { label: '加载失败', color: '#b91c1c', background: '#ef444418' },
  conflict: { label: '工具名冲突', color: '#7c3aed', background: '#8b5cf618' },
}

type TabType =
  | 'skills'
  | 'subagents'
  | 'prompts'
  | 'builtin-plugins'
  | 'workspace'
  | 'global'
  | 'builtins'
  | 'capabilities'

const TABS: { id: TabType; label: string; icon: IconName }[] = [
  { id: 'skills', label: '技能库 (Skills)', icon: 'zap' },
  { id: 'subagents', label: '子智能体', icon: 'bot' },
  { id: 'prompts', label: '提示词管理', icon: 'sparkles' },
  { id: 'builtin-plugins', label: '内置辅助插件', icon: 'plug' },
  { id: 'workspace', label: '工作区插件', icon: 'folder' },
  { id: 'global', label: '全局插件', icon: 'settings' },
  { id: 'builtins', label: '内置核心工具', icon: 'shield' },
  { id: 'capabilities', label: '能力开关', icon: 'settings' },
]

const TOOL_ICONS: Record<string, IconName> = {
  list_files: 'folder',
  read_file: 'file',
  search_files: 'search',
  todo: 'listTodo',
  invoke_subagent: 'bot',
  write_file: 'file',
  edit_file: 'file',
  run_command: 'terminal',
}

export function PluginsDialog({ client }: { client: AgentClient }) {
  /**
   * UI 侧动作留痕：走协议 `debug.trace`（M0 进程内直连主机的 trace，M3 起进主机日志）。
   * 抽成一行是因为这里要写十几条，直接展开 `client.request(...)` 会把业务代码淹掉。
   */
  const trace = (text: string): void => void client.request('debug.trace', { text })

  /**
   * 动作完成后的**用户可见**回执。
   *
   * 与 {@link trace} 的分工要分清楚：`trace` 进的是调试日志（默认不在屏幕上，
   * 要开调试面板才看得到），所以它**不能**当作"操作有反馈"——用户在插件页点一下
   * 开关，看到的只有开关自己的颜色变了，没有一件事告诉他"这次点真的生效了"。
   * 所有"点一下就该有回执"的动作走这里；trace 继续留着做排查用的留痕。
   */
  const notify = (notice: ActionNotice): void => client.ui.notify(notice)
  const [tab, setTab] = useState<TabType>('workspace')
  const [plugins, setPlugins] = useState<PluginItem[]>([])
  const [loading, setLoading] = useState(false)
  const [armedDeleteId, setArmedDeleteId] = useState<string | null>(null)

  // 内置工具过滤状态
  const [toolFilter, setToolFilter] = useState<'all' | 'readonly' | 'write'>('all')

  // 提示词管理状态
  const [prompts, setPrompts] = useState<PromptItem[]>([])
  const [promptFilter, setPromptFilter] = useState<'all' | 'builtin' | 'workspace' | 'global' | 'plugin'>('all')
  const [creatingPrompt, setCreatingPrompt] = useState(false)
  const [editingPromptId, setEditingPromptId] = useState<string | null>(null)
  const [promptName, setPromptName] = useState('')
  const [promptDesc, setPromptDesc] = useState('')
  const [promptArgumentHint, setPromptArgumentHint] = useState('')
  const [promptContent, setPromptContent] = useState('')
  const [promptScope, setPromptScope] = useState<'workspace' | 'global'>('workspace')
  const [promptIsSystem, setPromptIsSystem] = useState(false)
  const [promptNotice, setPromptNotice] = useState<string | null>(null)
  const [expandedPromptIds, setExpandedPromptIds] = useState<Record<string, boolean>>({})
  const [armedDeletePromptId, setArmedDeletePromptId] = useState<string | null>(null)
  const [copiedPromptId, setCopiedPromptId] = useState<string | null>(null)

  // 子智能体管理状态
  const [subagents, setSubagents] = useState<SubagentProfile[]>([])
  const [subagentFilter, setSubagentFilter] = useState<'all' | 'builtin' | 'workspace' | 'global'>('all')
  const [expandedSubagentIds, setExpandedSubagentIds] = useState<Record<string, boolean>>({})
  const [armedDeleteSubagentId, setArmedDeleteSubagentId] = useState<string | null>(null)

  // 新建插件表单状态
  const [creating, setCreating] = useState(false)
  const [newPluginName, setNewPluginName] = useState('')
  const [createNotice, setCreateNotice] = useState<string | null>(null)

  // 技能库管理状态
  const [skills, setSkills] = useState<SkillSummary[]>([])

  // 内置工具目录与主机环境（M2：都从主机取，界面不再 import 注册表与 home 模块）
  const [builtinCatalog, setBuiltinCatalog] = useState<BuiltinToolInfo[]>([])
  const [homeDir, setHomeDir] = useState('')

  // 能力开关状态（M3-2）：全局默认值 + 手输的超时
  const [capabilities, setCapabilities] = useState<PluginCapabilities>(DEFAULT_PLUGIN_CAPABILITIES)
  const [capabilityOverrides, setCapabilityOverrides] = useState<Record<string, Partial<PluginCapabilities>>>({})
  const [invalidCapabilityKeys, setInvalidCapabilityKeys] = useState<string[]>([])
  const [hookTimeoutDraft, setHookTimeoutDraft] = useState('500')
  const [capabilityNotice, setCapabilityNotice] = useState<string | null>(null)

  // 插件配置表单（M3-3）：pluginId → 键值草稿；secret 单独存"是否已设置"
  const [configDrafts, setConfigDrafts] = useState<Record<string, Record<string, string>>>({})
  const [secretSet, setSecretSet] = useState<Record<string, boolean>>({})
  const [configNotice, setConfigNotice] = useState<Record<string, string | null>>({})

  /**
   * 页面级错误条：动作失败时**在页面里说出来**。
   *
   * 为什么必须有：GPUIX 在 `process.on('unhandledRejection')` 上把整个窗口换成红色错误页
   * （"Uncaught runtime errors" + Reload）。一次"停用插件失败"如果没人接住，用户看到的不是
   * "这条命令失败了"，而是**整个应用变成错误页**——信息量与可恢复性都更差。
   * 所以这一页的每个动作都走 {@link runAction}，失败落到这里（同时记一条 trace 备查）。
   */
  const [actionError, setActionError] = useState<string | null>(null)

  /** 动作统一入口：失败在页面里说出来，**不往上抛**（抛出去就是整窗错误页）。 */
  const runAction = async (label: string, action: () => Promise<void>): Promise<void> => {
    try {
      await action()
      setActionError(null)
    } catch (err) {
      const message = `${label}失败：${(err as Error).message}`
      setActionError(message)
      trace(message)
    }
  }

  // 加载与刷新插件列表、提示词列表、子智能体与技能库
  //
  // M2：这一页原先要自己去摸四个管理器 + 配置文件（4 次扫描 + 逐插件读配置），
  // 现在一次 `plugin.list` 全拿到——界面不再知道"插件是怎么被加载的"。
  const refreshList = async () => {
    setLoading(true)
    try {
      const snapshot = await client.request('plugin.list', { workspace: client.state.project })
      setPlugins(snapshot.plugins)
      setCapabilities(snapshot.capabilities.capabilities)
      setHookTimeoutDraft(String(snapshot.capabilities.capabilities.hookTimeoutMs))
      setInvalidCapabilityKeys(snapshot.capabilities.invalid)
      setCapabilityOverrides(snapshot.capabilities.overrides)

      const [promptItems, subagentItems, skillItems, catalog, hostInfo] = await Promise.all([
        client.request('prompt.list', { workspace: client.state.project }),
        client.request('subagentProfile.list', { workspace: client.state.project }),
        client.request('skill.list', { workspace: client.state.project }),
        client.request('plugin.builtinCatalog', {}),
        client.request('debug.hostInfo', {}),
      ])
      setPrompts(promptItems)
      setSubagents(subagentItems)
      setSkills(skillItems)
      setBuiltinCatalog(catalog)
      setHomeDir(hostInfo.homeDir)

      // 每个插件的配置草稿：文件里有值就用它，否则用 schema 里的默认值。
      // secret 类型**不回显**——只记"是否已设置"，避免密钥出现在界面上
      const drafts: Record<string, Record<string, string>> = {}
      const secrets: Record<string, boolean> = {}
      for (const item of snapshot.plugins) {
        const properties = item.plugin.contributions.configSchema?.properties
        if (!properties) continue
        const stored = snapshot.configs[item.id] ?? {}
        drafts[item.id] = {}
        for (const [key, property] of Object.entries(properties)) {
          if (property.type === 'secret') {
            drafts[item.id]![key] = ''
            secrets[`${item.id}:${key}`] = snapshot.secrets[`${item.id}:${key}`] === true
            continue
          }
          const value = stored[key] ?? property.default
          drafts[item.id]![key] = value === undefined || value === null ? '' : String(value)
        }
      }
      setConfigDrafts(drafts)
      setSecretSet(secrets)
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    void refreshList()
  }, [client.state.project])

  /** 切换一个能力开关并落盘：关掉之后用到它的插件会显示"受限"。 */
  const handleToggleCapability = (key: keyof Omit<PluginCapabilities, 'hookTimeoutMs'>) => runAction('切换能力开关', async () => {
    const next = { ...capabilities, [key]: !capabilities[key] }
    setCapabilities(next)
    await client.request('plugin.capabilities.set', { patch: { [key]: next[key] } })
    void client.request('debug.trace', {
      text: `[插件] 能力开关 ${key} 已${next[key] ? '开启' : '关闭'}${
        next[key] ? '' : '——用到它的插件会显示受限原因'
      }`,
    })
    // 能力开关不是"这个插件"的开关：关掉之后**别的插件**会静默少做一步，所以关的时候
    // 用 warn 并把影响说出来（effect 取 capabilities-view 里现成的那句，不手写第二份）
    const described = CAPABILITY_SWITCHES.find((item) => item.key === key)
    const label = described?.label ?? key
    notify(capabilityToggleNotice(label, described?.effect, next[key]))
  })

  /** 保存超时值：非法输入当场说明，不写进配置（写进去只会变成"设了没生效"）。 */
  const handleSaveHookTimeout = () => runAction('保存钩子超时', async () => {
    const parsed = parseHookTimeout(hookTimeoutDraft)
    if (!parsed.ok) {
      setCapabilityNotice(parsed.reason)
      return
    }
    setCapabilityNotice(null)
    setCapabilities((current) => ({ ...current, hookTimeoutMs: parsed.value }))
    await client.request('plugin.capabilities.set', { patch: { hookTimeoutMs: parsed.value } })
    const humanized = parsed.value === 0 ? '不限' : `${parsed.value}ms`
    trace(`[插件] 钩子超时已设为 ${humanized}`)
    notify({
      message: `钩子超时已设为 ${humanized}`,
      detail: parsed.value === 0 ? '插件钩子不再被超时打断' : '超时按"没有意见"放行，不会变成隐式拒绝',
    })
  })

  /** 保存某个插件的配置项（非 secret 与 secret 分开写）。 */
  const handleSavePluginConfig = (item: PluginItem) => runAction('保存插件配置', async () => {
    const properties = item.plugin.contributions.configSchema?.properties
    if (!properties) return
    const draft = configDrafts[item.id] ?? {}

    const values: Record<string, unknown> = {}
    const secrets: Record<string, string> = {}
    for (const [key, property] of Object.entries(properties)) {
      const raw = draft[key] ?? ''
      if (property.type === 'secret') {
        // 空着就是"不改"，避免每次保存都把密钥清掉
        if (raw.trim()) secrets[key] = raw.trim()
        continue
      }
      if (property.type === 'number') {
        const parsed = Number(raw)
        if (raw.trim() === '' || !Number.isFinite(parsed)) {
          setConfigNotice((current) => ({
            ...current,
            [item.id]: `${property.title} 需要一个数字`,
          }))
          return
        }
        values[key] = parsed
      } else if (property.type === 'boolean') {
        values[key] = raw === 'true'
      } else {
        values[key] = raw
      }
    }

    await client.request('plugin.config.set', { pluginId: item.id, values })
    for (const [key, value] of Object.entries(secrets)) {
      await client.request('plugin.secret.set', { pluginId: item.id, key, value })
    }
    if (Object.keys(secrets).length > 0) {
      setSecretSet((current) => {
        const next = { ...current }
        for (const key of Object.keys(secrets)) next[`${item.id}:${key}`] = true
        return next
      })
    }
    setConfigNotice((current) => ({ ...current, [item.id]: '已保存' }))
    trace( `[插件] 已保存「${item.name}」的配置`)
    await refreshList()
  })

  // 切换子智能体启用状态
  const handleToggleSubagent = (item: SubagentProfile) => runAction('切换子智能体', async () => {
    const next = !item.enabled
    await client.request('subagentProfile.setEnabled', {
      id: item.id,
      enabled: next,
      workspace: client.state.project,
    })
    trace(`已${item.enabled ? '停用' : '启用'}子智能体：${item.name}`)
    notify(subagentToggleNotice(item.name, next))
    await refreshList()
  })

  // 删除自定义子智能体
  const handleDeleteSubagent = (item: SubagentProfile) => runAction('删除子智能体', async () => {
    if (armedDeleteSubagentId !== item.id) {
      setArmedDeleteSubagentId(item.id)
      return
    }
    const { ok: success } = await client.request('subagentProfile.delete', {
      id: item.id,
      workspace: client.state.project,
    })
    if (success) {
      trace(`已删除子智能体：${item.name}`)
    } else {
      trace(`删除子智能体失败：${item.name}`)
    }
    setArmedDeleteSubagentId(null)
    await refreshList()
  })

  const toggleSubagentExpand = (id: string) => {
    setExpandedSubagentIds((prev) => ({
      ...prev,
      [id]: !prev[id],
    }))
  }

  // 切换插件启用状态
  const handleToggle = (item: PluginItem) => runAction('切换插件', async () => {
    const next = !item.enabled
    await client.request('plugin.setEnabled', {
      pluginId: item.id,
      enabled: next,
      workspace: client.state.project,
    })
    trace(`已${item.enabled ? '停用' : '启用'}插件：${item.fileName}`)
    // 文案在 action-notices 里（纯函数，可就地断言），这里只负责喂数据
    notify(pluginToggleNotice(item.name, item.tools.length, next))
    await refreshList()
  })

  // 删除插件
  const handleDelete = (item: PluginItem) => runAction('删除插件', async () => {
    if (armedDeleteId !== item.id) {
      setArmedDeleteId(item.id)
      return
    }
    const { ok: success } = await client.request('plugin.delete', {
      filePath: item.filePath,
      workspace: client.state.project,
    })
    if (success) {
      trace(`已删除插件文件：${item.fileName}`)
    } else {
      trace(`删除插件文件失败：${item.fileName}`)
    }
    setArmedDeleteId(null)
    await refreshList()
  })

  // 提交新建插件
  const handleCreate = () => runAction('新建插件', async () => {
    const name = newPluginName.trim()
    if (!name) return
    try {
      const targetScope = tab === 'global' ? 'global' : 'workspace'
      const { filePath } = await client.request('plugin.createTemplate', {
        workspace: client.state.project,
        scope: targetScope,
        name,
      })
      trace(`已创建新插件模板：${filePath}`)
      setNewPluginName('')
      setCreating(false)
      setCreateNotice(null)
      await refreshList()
    } catch (err) {
      setCreateNotice(`创建失败：${(err as Error).message}`)
    }
  })

  // 切换提示词启用状态
  const handleTogglePrompt = (item: PromptItem) => runAction('切换提示词', async () => {
    const next = !item.enabled
    const { ok } = await client.request('prompt.setEnabled', {
      id: item.id,
      enabled: next,
      workspace: client.state.project,
    })
    if (!ok) {
      // 命令回了 ok:false（而不是抛错）时**也是失败**，不能报"已停用"
      notify(promptToggleFailedNotice(item.name))
      trace(`切换提示词失败：${item.name}`)
      return
    }
    trace(`已${item.enabled ? '停用' : '启用'}提示词：${item.name}`)
    notify(promptToggleNotice(item.name, item.isSystem, next))
    await refreshList()
  })

  // 开启新建提示词表单
  const startCreatePrompt = () => {
    setEditingPromptId(null)
    setPromptName('')
    setPromptDesc('')
    setPromptContent('')
    setPromptScope('workspace')
    setPromptIsSystem(false)
    setPromptNotice(null)
    setCreatingPrompt(true)
  }

  // 开启编辑提示词表单
  const startEditPrompt = (item: PromptItem) => {
    setCreatingPrompt(false)
    setEditingPromptId(item.id)
    setPromptName(item.name)
    setPromptDesc(item.description)
    setPromptArgumentHint(item.argumentHint || '')
    setPromptContent(item.content)
    setPromptScope(item.scope === 'global' ? 'global' : 'workspace')
    setPromptIsSystem(item.isSystem)
    setPromptNotice(null)
  }

  // 取消新建/编辑提示词
  const cancelPromptForm = () => {
    setCreatingPrompt(false)
    setEditingPromptId(null)
    setPromptArgumentHint('')
    setPromptNotice(null)
  }

  // 提交新建或保存编辑提示词
  const handleSavePrompt = () => runAction('保存提示词', async () => {
    const name = promptName.trim()
    const content = promptContent.trim()
    if (!name) {
      setPromptNotice('请输入提示词名称')
      return
    }
    if (!content) {
      setPromptNotice('请输入提示词正文内容')
      return
    }

    try {
      if (editingPromptId) {
        const target = prompts.find((p) => p.id === editingPromptId)
        if (target) {
          target.name = name
          target.description = promptDesc.trim()
          target.argumentHint = promptArgumentHint.trim() || undefined
          target.content = content
          target.isSystem = promptIsSystem
          await client.request('prompt.update', { item: target })
          trace(`已更新提示词：${name}`)
        }
      } else {
        await client.request('prompt.create', {
          workspace: client.state.project,
          options: {
            name,
            description: promptDesc.trim(),
            argumentHint: promptArgumentHint.trim() || undefined,
            content,
            scope: promptScope,
            isSystem: promptIsSystem,
            enabled: true,
          },
        })
        trace(`已创建新提示词：${name}`)
      }
      cancelPromptForm()
      await refreshList()
    } catch (err) {
      setPromptNotice(`保存失败：${(err as Error).message}`)
    }
  })

  // 删除提示词
  const handleDeletePrompt = (item: PromptItem) => runAction('删除提示词', async () => {
    if (armedDeletePromptId !== item.id) {
      setArmedDeletePromptId(item.id)
      return
    }
    if (item.filePath) {
      const { ok: success } = await client.request('prompt.delete', { filePath: item.filePath })
      if (success) {
        trace(`已删除提示词：${item.name}`)
      } else {
        trace(`删除提示词失败：${item.name}`)
      }
    }
    setArmedDeletePromptId(null)
    await refreshList()
  })

  // 复制提示词内容
  const handleCopyPrompt = async (item: PromptItem) => {
    const ok = await copyToClipboard(item.content)
    if (ok) {
      setCopiedPromptId(item.id)
      setTimeout(() => setCopiedPromptId(null), 1500)
    }
  }

  // 切换提示词卡片正文展开/收起
  const togglePromptExpand = (id: string) => {
    setExpandedPromptIds((prev) => ({
      ...prev,
      [id]: !prev[id],
    }))
  }

  // 过滤当前作用域的插件
  const currentPlugins =
    tab === 'builtin-plugins'
      ? plugins.filter((p) => p.scope === 'builtin')
      : plugins.filter((p) => p.scope === tab)

  const builtinPluginsCount = plugins.filter((p) => p.scope === 'builtin').length
  const workspaceCount = plugins.filter((p) => p.scope === 'workspace').length
  const globalCount = plugins.filter((p) => p.scope === 'global').length

  const currentDir =
    tab === 'builtin-plugins'
      ? '系统内置辅助 Coding 插件库（开箱即用，支持单独自由启用/停用）'
      : tab === 'workspace'
      ? join(client.state.project, '.ada', 'extensions')
      : join(homeDir, 'extensions')

  return (
    <div
      testId="plugins-modal"
      style={{
        position: 'absolute',
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: C.scrim,
        pointerEvents: 'auto',
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          width: '70%',
          maxWidth: '92%',
          minWidth: 500,
          height: '80%',
          borderRadius: 12,
          backgroundColor: C.raised,
          borderWidth: 1,
          borderColor: C.borderStrong,
          boxShadow: {
            offsetX: 0,
            offsetY: 18,
            blurRadius: 48,
            spreadRadius: 0,
            color: C.shadowStrong,
          },
          overflow: 'hidden',
        }}
      >
        {/* 顶部标题栏 */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            height: M.settingsHeader,
            flexShrink: 0,
            paddingLeft: 16,
            paddingRight: 10,
          }}
        >
          <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 8 }}>
            <Icon name="plug" size={15} color={C.link} />
            <text style={{ fontSize: 13.5, lineHeight: 18, fontWeight: 600, color: C.text }}>
              插件管理
            </text>
            <text style={{ fontSize: 11, color: C.faint }}>
              扩展 Agent 工具库与自动化能力
            </text>
          </div>
          <div style={{ flexGrow: 1 }} />
          <IconButton
            icon="close"
            testId="plugins-close"
            label="关闭插件管理"
            onClick={() => client.ui.setPlugins(false)}
          />
        </div>

        {/* 标题栏与主体之间的分割线 */}
        <div style={{ height: 1, flexShrink: 0, backgroundColor: C.border }} />

        {/* 动作失败时在这里说出来（而不是让未捕获的 rejection 把整窗换成错误页） */}
        {actionError ? (
          <div
            testId="plugins-action-error"
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 8,
              flexShrink: 0,
              paddingTop: 7,
              paddingBottom: 7,
              paddingLeft: 16,
              paddingRight: 16,
              backgroundColor: C.raised,
              borderBottomWidth: 1,
              borderColor: C.borderStrong,
            }}
          >
            <Icon name="alertTriangle" size={13} color={C.accent} />
            <text style={{ fontSize: 11.5, color: C.text, flexGrow: 1 }}>{actionError}</text>
            <div
              testId="plugins-action-error-dismiss"
              role="button"
              aria-label="知道了"
              onClick={() => setActionError(null)}
              style={{
                display: 'flex',
                alignItems: 'center',
                height: 20,
                paddingLeft: 8,
                paddingRight: 8,
                borderRadius: 6,
                cursor: 'pointer',
                backgroundColor: C.chip,
              }}
            >
              <text style={{ fontSize: 11, color: C.secondary }}>知道了</text>
            </div>
          </div>
        ) : null}

        {/* 主体两栏布局 */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            flexGrow: 1,
            minHeight: 0,
            borderBottomLeftRadius: 11,
            borderBottomRightRadius: 11,
            overflow: 'hidden',
          }}
        >
          {/* 左侧导航分栏 */}
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              width: 170,
              flexShrink: 0,
              padding: 10,
              gap: 4,
              backgroundColor: C.sidebar,
              borderRightWidth: 1,
              borderColor: C.border,
              borderBottomLeftRadius: 11,
            }}
          >
            {TABS.map((item) => {
              const count =
                item.id === 'skills'
                  ? skills.length
                  : item.id === 'subagents'
                  ? subagents.length
                  : item.id === 'prompts'
                  ? prompts.length
                  : item.id === 'builtin-plugins'
                  ? builtinPluginsCount
                  : item.id === 'workspace'
                  ? workspaceCount
                  : item.id === 'global'
                  ? globalCount
                  : builtinCatalog.length
              const isSelected = tab === item.id
              return (
                <div
                  key={item.id}
                  testId={`plugins-nav-${item.id}`}
                  role="button"
                  aria-label={item.label}
                  onClick={() => {
                    setTab(item.id)
                    setCreating(false)
                    cancelPromptForm()
                  }}
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    gap: 8,
                    height: 32,
                    paddingLeft: 8,
                    paddingRight: 8,
                    borderRadius: 6,
                    cursor: 'pointer',
                    backgroundColor: isSelected ? C.raised : '#00000000',
                    borderWidth: isSelected ? 1 : 0,
                    borderColor: C.border,
                    hover: { backgroundColor: isSelected ? C.raised : C.overlay },
                  }}
                >
                  <Icon
                    name={item.icon}
                    size={13}
                    color={isSelected ? C.link : C.secondary}
                  />
                  <text
                    style={{
                      fontSize: 12,
                      fontWeight: isSelected ? 600 : 400,
                      color: isSelected ? C.text : C.secondary,
                      flexGrow: 1,
                    }}
                  >
                    {item.label}
                  </text>
                  <div
                    style={{
                      paddingLeft: 5,
                      paddingRight: 5,
                      height: 16,
                      borderRadius: 8,
                      backgroundColor: isSelected ? C.chipHover : C.chip,
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'center',
                    }}
                  >
                    <text style={{ fontSize: 10, lineHeight: 14, color: C.faint }}>
                      {count}
                    </text>
                  </div>
                </div>
              )
            })}
          </div>

          {/* 右侧面板内容 */}
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              flexGrow: 1,
              minWidth: 0,
              padding: 16,
              overflowY: 'scroll',
              gap: 12,
              borderBottomRightRadius: 11,
            }}
          >
            {tab === 'skills' ? (
              <SkillsPanel
                client={client}
                skills={skills}
                onRefresh={refreshList}
                loading={loading}
                workspaceRoot={client.state.project}
                onTrace={(msg) => trace(msg)}
              />
            ) : tab === 'subagents' ? (
              <SubagentsPanel
                subagents={subagents}
                filter={subagentFilter}
                setFilter={setSubagentFilter}
                expandedIds={expandedSubagentIds}
                toggleExpand={toggleSubagentExpand}
                armedDeleteId={armedDeleteSubagentId}
                onToggle={handleToggleSubagent}
                onDelete={handleDeleteSubagent}
                onRefresh={() => void refreshList()}
                loading={loading}
              />
            ) : tab === 'prompts' ? (
              /* 提示词管理面板 */
              <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
                {/* 顶部过滤药丸与操作按钮栏 */}
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    gap: 8,
                    paddingBottom: 4,
                    borderBottomWidth: 1,
                    borderColor: C.border,
                  }}
                >
                  {/* 分类药丸 */}
                  <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 4, flexGrow: 1, minWidth: 0 }}>
                    {[
                      { id: 'all', label: '全部' },
                      { id: 'builtin', label: '内置预装' },
                      { id: 'workspace', label: '工作区项目' },
                      { id: 'global', label: '全局通用' },
                      { id: 'plugin', label: '插件内建' },
                    ].map((f) => {
                      const isFilterActive = promptFilter === f.id
                      const count =
                        f.id === 'all'
                          ? prompts.length
                          : prompts.filter((p) => p.scope === f.id).length
                      return (
                        <div
                          key={f.id}
                          testId={`prompt-filter-${f.id}`}
                          role="button"
                          onClick={() => setPromptFilter(f.id as any)}
                          style={{
                            display: 'flex',
                            flexDirection: 'row',
                            alignItems: 'center',
                            gap: 4,
                            height: 24,
                            paddingLeft: 7,
                            paddingRight: 7,
                            borderRadius: 12,
                            cursor: 'pointer',
                            backgroundColor: isFilterActive ? C.chipHover : C.overlay,
                            borderWidth: 1,
                            borderColor: isFilterActive ? C.link : '#00000000',
                          }}
                        >
                          <text style={{ fontSize: 11, fontWeight: isFilterActive ? 600 : 400, color: isFilterActive ? C.link : C.secondary }}>
                            {f.label}
                          </text>
                          <text style={{ fontSize: 9.5, color: C.faint }}>
                            {count}
                          </text>
                        </div>
                      )
                    })}
                  </div>

                  {/* 新建提示词按钮 */}
                  <div
                    testId="prompt-create-btn"
                    role="button"
                    onClick={() => {
                      if (creatingPrompt || editingPromptId) {
                        cancelPromptForm()
                      } else {
                        startCreatePrompt()
                      }
                    }}
                    style={{
                      display: 'flex',
                      flexDirection: 'row',
                      alignItems: 'center',
                      gap: 4,
                      height: 26,
                      paddingLeft: 8,
                      paddingRight: 8,
                      borderRadius: 6,
                      cursor: 'pointer',
                      backgroundColor: creatingPrompt || editingPromptId ? C.raised : C.link,
                      hover: { opacity: 0.9 },
                    }}
                  >
                    <Icon name={creatingPrompt || editingPromptId ? 'minus' : 'plus'} size={11} color="#ffffff" />
                    <text style={{ fontSize: 11, fontWeight: 600, color: '#ffffff' }}>
                      {creatingPrompt || editingPromptId ? '取消' : '新建提示词'}
                    </text>
                  </div>

                  {/* 刷新按钮 */}
                  <div
                    testId="prompt-refresh-btn"
                    role="button"
                    onClick={() => void refreshList()}
                    style={{
                      display: 'flex',
                      flexDirection: 'row',
                      alignItems: 'center',
                      gap: 4,
                      height: 26,
                      paddingLeft: 8,
                      paddingRight: 8,
                      borderRadius: 6,
                      cursor: 'pointer',
                      backgroundColor: C.chip,
                      hover: { backgroundColor: C.chipHover },
                    }}
                  >
                    <Icon name="refresh" size={11} color={C.secondary} />
                    <text style={{ fontSize: 11, color: C.secondary }}>
                      {loading ? '刷新中…' : '刷新'}
                    </text>
                  </div>
                </div>

                {/* 新建/编辑提示词内联表单 */}
                {creatingPrompt || editingPromptId ? (
                  <div
                    testId="prompt-form"
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 8,
                      padding: 12,
                      borderRadius: 8,
                      backgroundColor: C.raised,
                      borderWidth: 1,
                      borderColor: C.link,
                    }}
                  >
                    <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between' }}>
                      <text style={{ fontSize: 12, fontWeight: 600, color: C.text }}>
                        {editingPromptId ? '编辑提示词模板' : '快速新建提示词模板 (.md)'}
                      </text>
                      {!editingPromptId ? (
                        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                          <text style={{ fontSize: 11, color: C.secondary }}>存储范围：</text>
                          <div
                            role="button"
                            onClick={() => setPromptScope('workspace')}
                            style={{
                              paddingLeft: 6,
                              paddingRight: 6,
                              height: 20,
                              borderRadius: 4,
                              cursor: 'pointer',
                              backgroundColor: promptScope === 'workspace' ? C.link : C.chip,
                            }}
                          >
                            <text style={{ fontSize: 10.5, color: promptScope === 'workspace' ? '#ffffff' : C.secondary }}>
                              工作区
                            </text>
                          </div>
                          <div
                            role="button"
                            onClick={() => setPromptScope('global')}
                            style={{
                              paddingLeft: 6,
                              paddingRight: 6,
                              height: 20,
                              borderRadius: 4,
                              cursor: 'pointer',
                              backgroundColor: promptScope === 'global' ? C.link : C.chip,
                            }}
                          >
                            <text style={{ fontSize: 10.5, color: promptScope === 'global' ? '#ffffff' : C.secondary }}>
                              全局
                            </text>
                          </div>
                        </div>
                      ) : null}
                    </div>

                    <div style={{ display: 'flex', flexDirection: 'row', gap: 6 }}>
                      <input
                        testId="prompt-name-input"
                        value={promptName}
                        placeholder="提示词名称（例如：代码审查助手）"
                        autoFocus
                        theme={editorTheme()}
                        style={{
                          flexGrow: 1,
                          height: 28,
                          paddingLeft: 8,
                          paddingRight: 8,
                          borderRadius: 6,
                          fontSize: 12,
                          color: C.text,
                          backgroundColor: C.card,
                          borderWidth: 1,
                          borderColor: C.borderStrong,
                        }}
                        onChange={(e) => setPromptName(e.value ?? '')}
                      />
                    </div>

                    <input
                      testId="prompt-desc-input"
                      value={promptDesc}
                      placeholder="简要用途说明（例如：从架构、并发和安全维度审查代码）"
                      theme={editorTheme()}
                      style={{
                        width: '100%',
                        height: 28,
                        paddingLeft: 8,
                        paddingRight: 8,
                        borderRadius: 6,
                        fontSize: 12,
                        color: C.text,
                        backgroundColor: C.card,
                        borderWidth: 1,
                        borderColor: C.borderStrong,
                      }}
                      onChange={(e) => setPromptDesc(e.value ?? '')}
                    />

                    <input
                      testId="prompt-arghint-input"
                      value={promptArgumentHint}
                      placeholder="参数提示占位符（可选，例如：<pr-url> 或 [instructions]，对齐 pi 模板参数规范）"
                      theme={editorTheme()}
                      style={{
                        width: '100%',
                        height: 28,
                        paddingLeft: 8,
                        paddingRight: 8,
                        borderRadius: 6,
                        fontSize: 12,
                        color: C.text,
                        backgroundColor: C.card,
                        borderWidth: 1,
                        borderColor: C.borderStrong,
                      }}
                      onChange={(e) => setPromptArgumentHint(e.value ?? '')}
                    />

                    {/* 系统提示词开关 */}
                    <div
                      role="button"
                      onClick={() => setPromptIsSystem(!promptIsSystem)}
                      style={{
                        display: 'flex',
                        flexDirection: 'row',
                        alignItems: 'center',
                        gap: 6,
                        cursor: 'pointer',
                        paddingTop: 2,
                        paddingBottom: 2,
                      }}
                    >
                      <Icon
                        name={promptIsSystem ? 'circleCheck' : 'circle'}
                        size={13}
                        color={promptIsSystem ? C.link : C.faint}
                      />
                      <text style={{ fontSize: 11, color: promptIsSystem ? C.text : C.secondary }}>
                        设为系统提示词（启用后将作为 System Prompt 自动注入到模型上下文中）
                      </text>
                    </div>

                    {/* 正文多行输入框 */}
                    <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                      <text style={{ fontSize: 11, color: C.secondary }}>提示词正文内容 (Markdown)：</text>
                      <textarea
                        testId="prompt-content-input"
                        value={promptContent}
                        placeholder="输入提示词正文与指令模板..."
                        minRows={4}
                        maxRows={10}
                        theme={editorTheme()}
                        style={{
                          width: '100%',
                          minHeight: 80,
                          padding: 8,
                          borderRadius: 6,
                          fontSize: 12,
                          lineHeight: 18,
                          fontFamily: FONT_MONO,
                          color: C.text,
                          backgroundColor: C.card,
                          borderWidth: 1,
                          borderColor: C.borderStrong,
                        }}
                        onChange={(e) => setPromptContent(e.value ?? '')}
                      />
                    </div>

                    {promptNotice ? (
                      <text style={{ fontSize: 10.5, color: C.accent }}>{promptNotice}</text>
                    ) : null}

                    {/* 保存与取消按钮 */}
                    <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', justifyContent: 'flex-end', gap: 6 }}>
                      <div
                        role="button"
                        onClick={cancelPromptForm}
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          paddingLeft: 10,
                          paddingRight: 10,
                          height: 26,
                          borderRadius: 6,
                          cursor: 'pointer',
                          backgroundColor: C.chip,
                        }}
                      >
                        <text style={{ fontSize: 11, color: C.secondary }}>取消</text>
                      </div>
                      <div
                        testId="prompt-submit-btn"
                        role="button"
                        onClick={() => void handleSavePrompt()}
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          paddingLeft: 12,
                          paddingRight: 12,
                          height: 26,
                          borderRadius: 6,
                          cursor: 'pointer',
                          backgroundColor: C.link,
                        }}
                      >
                        <text style={{ fontSize: 11, fontWeight: 600, color: '#ffffff' }}>保存提示词</text>
                      </div>
                    </div>
                  </div>
                ) : null}

                {/* 提示词卡片列表 */}
                {(() => {
                  const filteredList =
                    promptFilter === 'all'
                      ? prompts
                      : prompts.filter((p) => p.scope === promptFilter)

                  if (filteredList.length === 0) {
                    return (
                      <div
                        style={{
                          display: 'flex',
                          flexDirection: 'column',
                          alignItems: 'center',
                          justifyContent: 'center',
                          gap: 8,
                          height: 180,
                          borderRadius: 8,
                          borderWidth: 1,
                          borderColor: C.borderStrong,
                        }}
                      >
                        <Icon name="sparkles" size={24} color={C.faint} />
                        <text style={{ fontSize: 12, color: C.faint }}>
                          暂无符合条件的提示词
                        </text>
                        <text style={{ fontSize: 11, color: C.ghost }}>
                          点击右上角「新建提示词」即可快速添加自定义角色或任务模板
                        </text>
                      </div>
                    )
                  }

                  return (
                    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                      {filteredList.map((item) => {
                        const isExpanded = Boolean(expandedPromptIds[item.id])
                        const isCopied = copiedPromptId === item.id
                        const isDeleteArmed = armedDeletePromptId === item.id
                        const isBuiltin = item.scope === 'builtin'

                        return (
                          <div
                            key={item.id}
                            testId={`prompt-card-${item.id}`}
                            style={{
                              display: 'flex',
                              flexDirection: 'column',
                              gap: 6,
                              padding: 12,
                              borderRadius: 8,
                              backgroundColor: C.raised,
                              borderWidth: 1,
                              borderColor: item.enabled ? C.borderStrong : C.border,
                              opacity: item.enabled ? 1 : 0.7,
                            }}
                          >
                            {/* 提示词标题行与开关控制 */}
                            <div
                              style={{
                                display: 'flex',
                                flexDirection: 'row',
                                alignItems: 'center',
                                gap: 8,
                              }}
                            >
                              <Icon
                                name="sparkles"
                                size={13}
                                color={item.enabled ? C.link : C.faint}
                              />
                              <text
                                style={{
                                  fontSize: 12.5,
                                  fontWeight: 600,
                                  color: C.text,
                                  whiteSpace: 'nowrap',
                                }}
                              >
                                {item.name}
                              </text>

                                {/* 作用域徽章 */}
                              <div
                                style={{
                                  paddingLeft: 6,
                                  paddingRight: 6,
                                  height: 18,
                                  borderRadius: 4,
                                  backgroundColor: item.scope === 'plugin' ? '#8b5cf618' : C.chip,
                                  borderWidth: item.scope === 'plugin' ? 1 : 0,
                                  borderColor: '#8b5cf640',
                                  display: 'flex',
                                  alignItems: 'center',
                                  flexShrink: 0,
                                }}
                              >
                                <text
                                  style={{
                                    fontSize: 10,
                                    lineHeight: 14,
                                    color: item.scope === 'plugin' ? '#8b5cf6' : C.faint,
                                    whiteSpace: 'nowrap',
                                  }}
                                >
                                  {item.scope === 'builtin'
                                    ? '内置预装'
                                    : item.scope === 'workspace'
                                    ? '工作区'
                                    : item.scope === 'global'
                                    ? '全局'
                                    : `插件: ${item.pluginName || '内建'}`}
                                </text>
                              </div>

                              {/* 参数提示占位符徽标 */}
                              {item.argumentHint ? (
                                <div
                                  style={{
                                    paddingLeft: 5,
                                    paddingRight: 5,
                                    height: 18,
                                    borderRadius: 4,
                                    backgroundColor: C.chipHover,
                                    display: 'flex',
                                    alignItems: 'center',
                                    flexShrink: 0,
                                  }}
                                >
                                  <text
                                    style={{
                                      fontSize: 9.5,
                                      lineHeight: 14,
                                      fontFamily: FONT_MONO,
                                      color: C.secondary,
                                      whiteSpace: 'nowrap',
                                    }}
                                  >
                                    {item.argumentHint}
                                  </text>
                                </div>
                              ) : null}

                              {/* 系统提示词徽章 */}
                              {item.isSystem ? (
                                <div
                                  style={{
                                    paddingLeft: 6,
                                    paddingRight: 6,
                                    height: 18,
                                    borderRadius: 4,
                                    backgroundColor: '#3b82f622',
                                    display: 'flex',
                                    alignItems: 'center',
                                    flexShrink: 0,
                                  }}
                                >
                                  <text style={{ fontSize: 10, lineHeight: 14, color: C.link, fontWeight: 500, whiteSpace: 'nowrap' }}>
                                    系统设定
                                  </text>
                                </div>
                              ) : null}

                              <div style={{ flexGrow: 1 }} />

                              {/* 启用/停用按钮开关 */}
                              <div
                                testId={`prompt-toggle-${item.id}`}
                                role="button"
                                aria-label={item.enabled ? '停用此提示词' : '启用此提示词'}
                                onClick={() => void handleTogglePrompt(item)}
                                style={{
                                  display: 'flex',
                                  flexDirection: 'row',
                                  alignItems: 'center',
                                  gap: 4,
                                  height: 22,
                                  paddingLeft: 7,
                                  paddingRight: 7,
                                  borderRadius: 11,
                                  cursor: 'pointer',
                                  backgroundColor: item.enabled ? '#10b98122' : C.chip,
                                  borderWidth: 1,
                                  borderColor: item.enabled ? '#10b98155' : C.border,
                                }}
                              >
                                <div
                                  style={{
                                    width: 7,
                                    height: 7,
                                    borderRadius: 4,
                                    backgroundColor: item.enabled ? C.success : C.faint,
                                  }}
                                />
                                <text
                                  style={{
                                    fontSize: 10.5,
                                    fontWeight: 500,
                                    color: item.enabled ? C.success : C.faint,
                                    whiteSpace: 'nowrap',
                                  }}
                                >
                                  {item.enabled ? '已启用' : '已停用'}
                                </text>
                              </div>
                            </div>

                            {/* 描述信息 */}
                            {item.description ? (
                              <text style={{ fontSize: 11.5, lineHeight: 16, color: C.secondary }}>
                                {item.description}
                              </text>
                            ) : null}

                            {/* 工具操作栏 */}
                            <div
                              style={{
                                display: 'flex',
                                flexDirection: 'row',
                                alignItems: 'center',
                                gap: 6,
                                paddingTop: 4,
                                borderTopWidth: 1,
                                borderColor: C.cardBorder,
                              }}
                            >
                              {/* 应用到输入框按钮 */}
                              <div
                                testId={`prompt-apply-${item.id}`}
                                role="button"
                                onClick={() => client.ui.applyPromptToComposer(item.content)}
                                style={{
                                  display: 'flex',
                                  flexDirection: 'row',
                                  alignItems: 'center',
                                  gap: 4,
                                  height: 22,
                                  paddingLeft: 6,
                                  paddingRight: 6,
                                  borderRadius: 4,
                                  cursor: 'pointer',
                                  backgroundColor: C.chip,
                                  hover: { backgroundColor: C.chipHover },
                                }}
                              >
                                <Icon name="arrowRight" size={11} color={C.link} />
                                <text style={{ fontSize: 10.5, color: C.link }}>
                                  应用到输入框
                                </text>
                              </div>

                              {/* 复制正文按钮 */}
                              <div
                                testId={`prompt-copy-${item.id}`}
                                role="button"
                                onClick={() => void handleCopyPrompt(item)}
                                style={{
                                  display: 'flex',
                                  flexDirection: 'row',
                                  alignItems: 'center',
                                  gap: 4,
                                  height: 22,
                                  paddingLeft: 6,
                                  paddingRight: 6,
                                  borderRadius: 4,
                                  cursor: 'pointer',
                                  backgroundColor: C.chip,
                                  hover: { backgroundColor: C.chipHover },
                                }}
                              >
                                <Icon
                                  name={isCopied ? 'check' : 'copy'}
                                  size={11}
                                  color={isCopied ? C.success : C.secondary}
                                />
                                <text style={{ fontSize: 10.5, color: isCopied ? C.success : C.secondary }}>
                                  {isCopied ? '已复制' : '复制内容'}
                                </text>
                              </div>

                              {/* 展开/收起正文 */}
                              <div
                                testId={`prompt-expand-${item.id}`}
                                role="button"
                                onClick={() => togglePromptExpand(item.id)}
                                style={{
                                  display: 'flex',
                                  flexDirection: 'row',
                                  alignItems: 'center',
                                  gap: 4,
                                  height: 22,
                                  paddingLeft: 6,
                                  paddingRight: 6,
                                  borderRadius: 4,
                                  cursor: 'pointer',
                                  backgroundColor: C.chip,
                                  hover: { backgroundColor: C.chipHover },
                                }}
                              >
                                <Icon
                                  name={isExpanded ? 'chevronUp' : 'chevronDown'}
                                  size={11}
                                  color={C.secondary}
                                />
                                <text style={{ fontSize: 10.5, color: C.secondary }}>
                                  {isExpanded ? '收起预览' : '预览正文'}
                                </text>
                              </div>

                              <div style={{ flexGrow: 1 }} />

                              {/* 编辑与删除（仅工作区与全局自定义提示词可编辑/删除） */}
                              {!isBuiltin && item.scope !== 'plugin' ? (
                                <>
                                  <div
                                    testId={`prompt-edit-${item.id}`}
                                    role="button"
                                    onClick={() => startEditPrompt(item)}
                                    style={{
                                      display: 'flex',
                                      alignItems: 'center',
                                      height: 22,
                                      paddingLeft: 6,
                                      paddingRight: 6,
                                      borderRadius: 4,
                                      cursor: 'pointer',
                                      backgroundColor: C.chip,
                                      hover: { backgroundColor: C.chipHover },
                                    }}
                                  >
                                    <text style={{ fontSize: 10.5, color: C.secondary }}>编辑</text>
                                  </div>

                                  <div
                                    testId={`prompt-delete-${item.id}`}
                                    role="button"
                                    onClick={() => void handleDeletePrompt(item)}
                                    style={{
                                      display: 'flex',
                                      alignItems: 'center',
                                      height: 22,
                                      paddingLeft: 6,
                                      paddingRight: 6,
                                      borderRadius: 4,
                                      cursor: 'pointer',
                                      backgroundColor: isDeleteArmed ? '#ef444422' : C.chip,
                                      borderWidth: isDeleteArmed ? 1 : 0,
                                      borderColor: C.danger,
                                    }}
                                  >
                                    <text
                                      style={{
                                        fontSize: 10.5,
                                        color: isDeleteArmed ? C.danger : C.faint,
                                        fontWeight: isDeleteArmed ? 600 : 400,
                                      }}
                                    >
                                      {isDeleteArmed ? '确认删除?' : '删除'}
                                    </text>
                                  </div>
                                </>
                              ) : null}
                            </div>

                            {/* 展开的 Markdown 正文预览 */}
                            {isExpanded ? (
                              <div
                                testId={`prompt-preview-${item.id}`}
                                style={{
                                  display: 'flex',
                                  flexDirection: 'column',
                                  padding: 8,
                                  marginTop: 4,
                                  borderRadius: 6,
                                  backgroundColor: C.card,
                                  borderWidth: 1,
                                  borderColor: C.cardBorder,
                                }}
                              >
                                <markdown source={item.content} theme={docTheme()} />
                              </div>
                            ) : null}
                          </div>
                        )
                      })}
                    </div>
                  )
                })()}
              </div>
            ) : tab === 'capabilities' ? (
              /* 能力开关（M3-2）：默认全开，关掉后用到的插件会显示受限原因 */
              <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
                <text style={{ fontSize: 11.5, color: C.faint }}>
                  插件能做什么由这里决定。默认全部开放，关掉后用到的插件会在卡片上显示受限原因——
                  不允许静默失效。
                </text>

                {invalidCapabilityKeys.length > 0 ? (
                  <text style={{ fontSize: 11, color: C.accent }}>
                    {`配置文件里有 ${invalidCapabilityKeys.length} 处取值不可用，已按默认值处理：${invalidCapabilityKeys.join('、')}`}
                  </text>
                ) : null}

                {CAPABILITY_SWITCHES.map((item) => {
                  const enabled = capabilities[item.key]
                  return (
                    <div
                      key={item.key}
                      style={{
                        display: 'flex',
                        flexDirection: 'row',
                        alignItems: 'flex-start',
                        gap: 10,
                        paddingTop: 8,
                        paddingBottom: 8,
                        borderBottomWidth: 1,
                        borderColor: C.border,
                      }}
                    >
                      <div style={{ display: 'flex', flexDirection: 'column', gap: 2, flexGrow: 1 }}>
                        <text style={{ fontSize: 12.5, color: C.text }}>{item.label}</text>
                        <text style={{ fontSize: 11, color: C.faint }}>{item.description}</text>
                        {!enabled ? (
                          <text style={{ fontSize: 11, color: C.accent }}>{`关掉后：${item.effect}`}</text>
                        ) : null}
                      </div>
                      <div
                        testId={`capability-toggle-${item.key}`}
                        role="button"
                        aria-label={enabled ? `关闭 ${item.label}` : `开启 ${item.label}`}
                        onClick={() => void handleToggleCapability(item.key)}
                        style={{
                          display: 'flex',
                          flexDirection: 'row',
                          alignItems: 'center',
                          height: 22,
                          paddingLeft: 8,
                          paddingRight: 8,
                          borderRadius: 11,
                          cursor: 'pointer',
                          flexShrink: 0,
                          backgroundColor: enabled ? '#10b98126' : C.chip,
                          borderWidth: 1,
                          borderColor: enabled ? C.success : C.borderStrong,
                        }}
                      >
                        <text
                          style={{
                            fontSize: 10,
                            color: enabled ? C.success : C.faint,
                            whiteSpace: 'nowrap',
                          }}
                        >
                          {enabled ? '已开启' : '已关闭'}
                        </text>
                      </div>
                    </div>
                  )
                })}

                {/* 钩子超时：0 = 不限。超时一律**放行**并记 trace，不变成隐式拒绝 */}
                <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                  <text style={{ fontSize: 12.5, color: C.text }}>单个钩子的超时（毫秒）</text>
                  <text style={{ fontSize: 11, color: C.faint }}>
                    超时后按"没有意见"放行并写进调试日志——超时不该变成隐式拒绝。0 表示不限。
                  </text>
                  <div style={{ display: 'flex', flexDirection: 'row', gap: 6, alignItems: 'center' }}>
                    <input
                      testId="capability-hook-timeout-input"
                      value={hookTimeoutDraft}
                      placeholder="500"
                      theme={editorTheme()}
                      style={{
                        width: 110,
                        height: 28,
                        paddingLeft: 8,
                        paddingRight: 8,
                        borderRadius: 6,
                        fontSize: 12,
                        color: C.text,
                        backgroundColor: C.card,
                        borderWidth: 1,
                        borderColor: C.borderStrong,
                      }}
                      onChange={(e) => setHookTimeoutDraft(e.value ?? '')}
                    />
                    <div
                      testId="capability-hook-timeout-save"
                      role="button"
                      onClick={() => void handleSaveHookTimeout()}
                      style={{
                        display: 'flex',
                        flexDirection: 'row',
                        alignItems: 'center',
                        height: 28,
                        paddingLeft: 10,
                        paddingRight: 10,
                        borderRadius: 6,
                        cursor: 'pointer',
                        backgroundColor: C.link,
                      }}
                    >
                      <text style={{ fontSize: 11.5, color: '#fff' }}>保存</text>
                    </div>
                    {capabilityNotice ? (
                      <text style={{ fontSize: 11, color: C.accent }}>{capabilityNotice}</text>
                    ) : null}
                  </div>
                </div>
              </div>
            ) : tab === 'builtins' ? (
              /* 内置核心工具展示 */
              <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
                {/* 顶部过滤药丸 */}
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    gap: 8,
                    paddingBottom: 4,
                    borderBottomWidth: 1,
                    borderColor: C.border,
                  }}
                >
                  <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 4, flexGrow: 1, minWidth: 0 }}>
                    {[
                      { id: 'all', label: '全部' },
                      { id: 'readonly', label: '只读安全' },
                      { id: 'write', label: '需审批写入' },
                    ].map((f) => {
                      const isFilterActive = toolFilter === f.id
                      return (
                        <div
                          key={f.id}
                          role="button"
                          onClick={() => setToolFilter(f.id as any)}
                          style={{
                            display: 'flex',
                            alignItems: 'center',
                            height: 24,
                            paddingLeft: 10,
                            paddingRight: 10,
                            borderRadius: 12,
                            cursor: 'pointer',
                            backgroundColor: isFilterActive ? C.link : C.chip,
                            hover: { backgroundColor: isFilterActive ? C.link : C.chipHover },
                          }}
                        >
                          <text
                            style={{
                              fontSize: 11,
                              fontWeight: isFilterActive ? 600 : 400,
                              color: isFilterActive ? '#ffffff' : C.secondary,
                            }}
                          >
                            {f.label}
                          </text>
                        </div>
                      )
                    })}
                  </div>
                </div>

                {/* 说明横幅 */}
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'flex-start',
                    gap: 10,
                    padding: 10,
                    borderRadius: 8,
                    backgroundColor: C.overlay,
                    borderWidth: 1,
                    borderColor: C.border,
                  }}
                >
                  <div style={{ flexShrink: 0, paddingTop: 1 }}>
                    <Icon name="shield" size={16} color={C.link} />
                  </div>
                  <text
                    style={{
                      fontSize: 11.5,
                      lineHeight: 17,
                      color: C.secondary,
                      flexGrow: 1,
                      minWidth: 0,
                      whiteSpace: 'normal',
                    }}
                  >
                    核心内置工具（系统预装）：以下核心工具由 Agent 引擎内置托管，具备只读白名单校验与文件写入审批安全闸门。主 Agent 可在规划与编码任务时自主调度，保障工作区操作的安全可控。
                  </text>
                </div>

                {/* 工具列表渲染 */}
                {(() => {
                  const filteredTools = builtinCatalog.filter((bt) => {
                    if (toolFilter === 'readonly') return bt.isReadOnly
                    if (toolFilter === 'write') return !bt.isReadOnly
                    return true
                  })

                  return (
                    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                      {filteredTools.map((bt) => (
                        <div
                          key={bt.name}
                          style={{
                            display: 'flex',
                            flexDirection: 'column',
                            gap: 8,
                            padding: 12,
                            borderRadius: 8,
                            backgroundColor: C.raised,
                            borderWidth: 1,
                            borderColor: C.borderStrong,
                          }}
                        >
                          <div
                            style={{
                              display: 'flex',
                              flexDirection: 'row',
                              alignItems: 'center',
                              gap: 8,
                            }}
                          >
                            <Icon
                              name={TOOL_ICONS[bt.name] ?? 'terminal'}
                              size={14}
                              color={bt.isReadOnly ? C.link : '#f59e0b'}
                            />
                            <text
                              style={{
                                fontSize: 12.5,
                                fontWeight: 600,
                                color: C.text,
                                whiteSpace: 'nowrap',
                              }}
                            >
                              {bt.label}
                            </text>
                            <text
                              style={{
                                fontFamily: FONT_MONO,
                                fontSize: 11,
                                color: C.tertiary,
                              }}
                            >
                              {`[${bt.name}]`}
                            </text>

                            {/* 模式徽章 */}
                            <div
                              style={{
                                paddingLeft: 6,
                                paddingRight: 6,
                                height: 18,
                                borderRadius: 4,
                                backgroundColor: bt.isReadOnly ? '#10b98118' : '#f59e0b18',
                                display: 'flex',
                                alignItems: 'center',
                              }}
                            >
                              <text
                                style={{
                                  fontSize: 10,
                                  lineHeight: 14,
                                  color: bt.isReadOnly ? C.success : '#f59e0b',
                                  fontWeight: 500,
                                  whiteSpace: 'nowrap',
                                }}
                              >
                                {bt.isReadOnly ? '只读安全' : '需审批写入'}
                              </text>
                            </div>

                            {/* 系统内置徽章 */}
                            <div
                              style={{
                                paddingLeft: 6,
                                paddingRight: 6,
                                height: 18,
                                borderRadius: 4,
                                backgroundColor: C.chip,
                                display: 'flex',
                                alignItems: 'center',
                              }}
                            >
                              <text style={{ fontSize: 10, lineHeight: 14, color: C.faint, whiteSpace: 'nowrap' }}>
                                系统内置
                              </text>
                            </div>

                            <div style={{ flexGrow: 1 }} />

                            {/* 状态徽标 */}
                            <div
                              style={{
                                display: 'flex',
                                flexDirection: 'row',
                                alignItems: 'center',
                                gap: 4,
                                height: 22,
                                paddingLeft: 7,
                                paddingRight: 7,
                                borderRadius: 11,
                                backgroundColor: '#10b98122',
                                borderWidth: 1,
                                borderColor: '#10b98155',
                              }}
                            >
                              <div
                                style={{
                                  width: 7,
                                  height: 7,
                                  borderRadius: 4,
                                  backgroundColor: C.success,
                                }}
                              />
                              <text
                                style={{
                                  fontSize: 10.5,
                                  fontWeight: 500,
                                  color: C.success,
                                }}
                              >
                                核心常驻
                              </text>
                            </div>
                          </div>

                          {/* 工具描述（自动换行） */}
                          <text
                            style={{
                              fontSize: 11.5,
                              lineHeight: 16,
                              color: C.secondary,
                              flexGrow: 1,
                              minWidth: 0,
                              whiteSpace: 'normal',
                            }}
                          >
                            {bt.description}
                          </text>
                        </div>
                      ))}
                    </div>
                  )
                })()}
              </div>
            ) : (
              /* 工作区 / 全局扩展插件管理面板 */
              <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
                {/* 顶部路径说明与操作按钮栏 */}
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    gap: 8,
                    paddingBottom: 4,
                    borderBottomWidth: 1,
                    borderColor: C.border,
                  }}
                >
                  <div style={{ display: 'flex', flexDirection: 'column', flexGrow: 1, minWidth: 0 }}>
                    <text style={{ fontSize: 11, color: C.faint }}>插件存储目录：</text>
                    <text
                      style={{
                        fontSize: 11,
                        fontFamily: FONT_MONO,
                        color: C.secondary,
                        whiteSpace: 'nowrap',
                        textOverflow: 'ellipsis',
                      }}
                    >
                      {currentDir}
                    </text>
                  </div>

                  {tab !== 'builtin-plugins' ? (
                    <div
                      testId="plugin-create-btn"
                      role="button"
                      onClick={() => setCreating((prev) => !prev)}
                      style={{
                        display: 'flex',
                        flexDirection: 'row',
                        alignItems: 'center',
                        gap: 4,
                        height: 26,
                        paddingLeft: 8,
                        paddingRight: 8,
                        borderRadius: 6,
                        cursor: 'pointer',
                        backgroundColor: creating ? C.raised : C.link,
                        hover: { opacity: 0.9 },
                      }}
                    >
                      <Icon name={creating ? 'minus' : 'plus'} size={11} color="#ffffff" />
                      <text style={{ fontSize: 11, fontWeight: 600, color: '#ffffff' }}>
                        {creating ? '取消' : '新建插件'}
                      </text>
                    </div>
                  ) : null}

                  <div
                    testId="plugin-refresh-btn"
                    role="button"
                    onClick={() => void refreshList()}
                    style={{
                      display: 'flex',
                      flexDirection: 'row',
                      alignItems: 'center',
                      gap: 4,
                      height: 26,
                      paddingLeft: 8,
                      paddingRight: 8,
                      borderRadius: 6,
                      cursor: 'pointer',
                      backgroundColor: C.chip,
                      hover: { backgroundColor: C.chipHover },
                    }}
                  >
                    <Icon name="refresh" size={11} color={C.secondary} />
                    <text style={{ fontSize: 11, color: C.secondary }}>
                      {loading ? '刷新中…' : '刷新'}
                    </text>
                  </div>
                </div>

                {/* 新建插件内联表单 */}
                {creating ? (
                  <div
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 6,
                      padding: 10,
                      borderRadius: 8,
                      backgroundColor: C.raised,
                      borderWidth: 1,
                      borderColor: C.link,
                    }}
                  >
                    <text style={{ fontSize: 11.5, fontWeight: 600, color: C.text }}>
                      快速新建扩展插件模板 (.ts)
                    </text>
                    <div style={{ display: 'flex', flexDirection: 'row', gap: 6 }}>
                      <input
                        testId="plugin-name-input"
                        value={newPluginName}
                        placeholder="例如: weather_tool 或 my_search"
                        autoFocus
                        theme={editorTheme()}
                        style={{
                          flexGrow: 1,
                          height: 28,
                          paddingLeft: 8,
                          paddingRight: 8,
                          borderRadius: 6,
                          fontSize: 12,
                          color: C.text,
                          backgroundColor: C.card,
                          borderWidth: 1,
                          borderColor: C.borderStrong,
                        }}
                        onChange={(e) => setNewPluginName(e.value ?? '')}
                        onSubmit={() => void handleCreate()}
                      />
                      <div
                        testId="plugin-submit-create"
                        role="button"
                        onClick={() => void handleCreate()}
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          justifyContent: 'center',
                          paddingLeft: 12,
                          paddingRight: 12,
                          height: 28,
                          borderRadius: 6,
                          cursor: 'pointer',
                          backgroundColor: C.link,
                        }}
                      >
                        <text style={{ fontSize: 11.5, fontWeight: 600, color: '#ffffff' }}>
                          生成
                        </text>
                      </div>
                    </div>
                    {createNotice ? (
                      <text style={{ fontSize: 10.5, color: C.accent }}>{createNotice}</text>
                    ) : null}
                  </div>
                ) : null}

                {/* 插件卡片列表 */}
                {currentPlugins.length === 0 ? (
                  <div
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      alignItems: 'center',
                      justifyContent: 'center',
                      gap: 8,
                      height: 180,
                      borderRadius: 8,
                      borderWidth: 1,
                      borderColor: C.borderStrong,
                    }}
                  >
                    <Icon name="plug" size={24} color={C.faint} />
                    <text style={{ fontSize: 12, color: C.faint }}>
                      当前目录下暂无扩展插件
                    </text>
                    <text style={{ fontSize: 11, color: C.ghost }}>
                      点击右上角「新建插件」即可一键生成标准 TypeScript 扩展脚本
                    </text>
                  </div>
                ) : (
                  currentPlugins.map((item) => {
                    // 只在"不是就绪"时挂徽标：正常插件卡上多一行"就绪"纯属噪音
                    const statusBadge = PLUGIN_STATUS_BADGE[item.status]
                    // 这个插件被哪些能力开关限制（关掉开关后必须说出来）
                    const restrictions = describePluginRestrictions(item.plugin, {
                      ...capabilities,
                      ...(capabilityOverrides[item.id] ?? {}),
                    })
                    return (
                    <div
                      key={item.id}
                      style={{
                        display: 'flex',
                        flexDirection: 'column',
                        gap: 6,
                        padding: 12,
                        borderRadius: 8,
                        backgroundColor: C.raised,
                        borderWidth: 1,
                        borderColor: item.enabled ? C.borderStrong : C.border,
                        opacity: item.enabled ? 1 : 0.65,
                      }}
                    >
                      {/* 插件标题行与开关控制 */}
                      <div
                        style={{
                          display: 'flex',
                          flexDirection: 'row',
                          alignItems: 'center',
                          gap: 8,
                        }}
                      >
                        <Icon
                          name="file"
                          size={14}
                          color={item.enabled ? C.link : C.faint}
                        />
                        <text
                          style={{
                            fontSize: 12.5,
                            fontWeight: 600,
                            fontFamily: FONT_MONO,
                            color: C.text,
                            whiteSpace: 'nowrap',
                          }}
                        >
                          {item.fileName}
                        </text>

                        <div
                          style={{
                            display: 'flex',
                            flexDirection: 'row',
                            alignItems: 'center',
                            height: 18,
                            paddingLeft: 6,
                            paddingRight: 6,
                            borderRadius: 4,
                            backgroundColor: item.scope === 'builtin' ? '#3b82f618' : C.chip,
                            borderWidth: item.scope === 'builtin' ? 1 : 0,
                            borderColor: '#3b82f640',
                            flexShrink: 0,
                          }}
                        >
                          <text
                            style={{
                              fontSize: 10,
                              lineHeight: 14,
                              color: item.scope === 'builtin' ? C.link : C.faint,
                              whiteSpace: 'nowrap',
                            }}
                          >
                            {item.scope === 'builtin' ? '系统内置' : `${Math.max(1, Math.round(item.sizeBytes / 1024))} KB`}
                          </text>
                        </div>

                        <div style={{ flexGrow: 1 }} />

                        {/* 启用/停用按钮开关 */}
                        <div
                          testId={`plugin-toggle-${item.name}`}
                          role="button"
                          aria-label={item.enabled ? '停用此插件' : '启用此插件'}
                          onClick={() => void handleToggle(item)}
                          style={{
                            display: 'flex',
                            flexDirection: 'row',
                            alignItems: 'center',
                            gap: 4,
                            height: 22,
                            paddingLeft: 8,
                            paddingRight: 8,
                            borderRadius: 11,
                            cursor: 'pointer',
                            backgroundColor: item.enabled ? '#10b98126' : C.chip,
                            borderWidth: 1,
                            borderColor: item.enabled ? C.success : C.borderStrong,
                          }}
                        >
                          <div
                            style={{
                              width: 6,
                              height: 6,
                              borderRadius: 3,
                              backgroundColor: item.enabled ? C.success : C.faint,
                            }}
                          />
                          <text
                            style={{
                              fontSize: 10.5,
                              fontWeight: 600,
                              color: item.enabled ? C.success : C.secondary,
                            }}
                          >
                            {item.enabled ? '已启用' : '已停用'}
                          </text>
                        </div>

                        {/* 删除插件按钮 (带二次确认，仅工作区与全局插件可删除) */}
                        {item.scope !== 'builtin' ? (
                          <div
                            testId={`plugin-delete-${item.name}`}
                            role="button"
                            aria-label={
                              armedDeleteId === item.id
                                ? `确认删除 ${item.fileName}`
                                : `删除 ${item.fileName}`
                            }
                            onClick={() => void handleDelete(item)}
                            style={{
                              display: 'flex',
                              flexDirection: 'row',
                              alignItems: 'center',
                              gap: 3,
                              height: 22,
                              paddingLeft: armedDeleteId === item.id ? 6 : 4,
                              paddingRight: armedDeleteId === item.id ? 6 : 4,
                              borderRadius: 4,
                              cursor: 'pointer',
                              backgroundColor:
                                armedDeleteId === item.id ? C.accentSoft : '#00000000',
                              hover: { backgroundColor: C.chipHover },
                            }}
                          >
                            {armedDeleteId === item.id ? (
                              <text style={{ fontSize: 10, color: C.accent, fontWeight: 600 }}>
                                确认删除
                              </text>
                            ) : null}
                            <Icon
                              name="trash"
                              size={12}
                              color={armedDeleteId === item.id ? C.accent : C.faint}
                            />
                          </div>
                        ) : null}
                      </div>

                      {/* 导出的工具标签与描述 */}
                      {item.tools.length > 0 ? (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 3 }}>
                          <div
                            style={{
                              display: 'flex',
                              flexDirection: 'row',
                              flexWrap: 'wrap',
                              gap: 4,
                              alignItems: 'center',
                            }}
                          >
                            <text style={{ fontSize: 10.5, color: C.faint }}>导出工具：</text>
                            {item.tools.map((t) => (
                              <div
                                key={t.name}
                                style={{
                                  paddingLeft: 6,
                                  paddingRight: 6,
                                  height: 18,
                                  borderRadius: 4,
                                  backgroundColor: C.card,
                                  borderWidth: 1,
                                  borderColor: C.border,
                                  display: 'flex',
                                  alignItems: 'center',
                                }}
                              >
                                <text
                                  style={{
                                    fontSize: 10.5,
                                    fontFamily: FONT_MONO,
                                    color: C.link,
                                  }}
                                >
                                  {t.name}
                                </text>
                              </div>
                            ))}
                          </div>
                          {item.tools[0]?.description ? (
                            <text
                              style={{
                                fontSize: 11,
                                lineHeight: 15,
                                color: C.faint,
                                paddingLeft: 2,
                              }}
                            >
                              {item.tools[0].description}
                            </text>
                          ) : null}
                        </div>
                      ) : null}

                      {/* 携带的技能规范列表（归纳到插件系统中） */}
                      {item.skills && item.skills.length > 0 ? (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 3, marginTop: item.tools.length > 0 ? 3 : 0 }}>
                          <div
                            style={{
                              display: 'flex',
                              flexDirection: 'row',
                              flexWrap: 'wrap',
                              gap: 4,
                              alignItems: 'center',
                            }}
                          >
                            <text style={{ fontSize: 10.5, color: C.faint }}>内建技能：</text>
                            {item.skills.map((s) => (
                              <div
                                key={s.name}
                                style={{
                                  paddingLeft: 6,
                                  paddingRight: 6,
                                  height: 18,
                                  borderRadius: 4,
                                  backgroundColor: `${C.accent}14`,
                                  borderWidth: 1,
                                  borderColor: `${C.accent}30`,
                                  display: 'flex',
                                  alignItems: 'center',
                                }}
                              >
                                <text
                                  style={{
                                    fontSize: 10.5,
                                    fontFamily: FONT_MONO,
                                    color: C.accent,
                                  }}
                                >
                                  {s.name}
                                </text>
                              </div>
                            ))}
                          </div>
                          {item.skills[0]?.description ? (
                            <text
                              style={{
                                fontSize: 11,
                                lineHeight: 15,
                                color: C.faint,
                                paddingLeft: 2,
                              }}
                            >
                              {item.skills[0].description}
                            </text>
                          ) : null}
                        </div>
                      ) : null}

                      {/* 携带的提示词列表（归纳到插件系统中） */}
                      {item.prompts && item.prompts.length > 0 ? (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 3, marginTop: (item.tools.length > 0 || (item.skills && item.skills.length > 0)) ? 3 : 0 }}>
                          <div
                            style={{
                              display: 'flex',
                              flexDirection: 'row',
                              flexWrap: 'wrap',
                              gap: 4,
                              alignItems: 'center',
                            }}
                          >
                            <text style={{ fontSize: 10.5, color: C.faint }}>内建提示词：</text>
                            {item.prompts.map((p) => (
                              <div
                                key={p.id}
                                style={{
                                  display: 'flex',
                                  flexDirection: 'row',
                                  alignItems: 'center',
                                  gap: 3,
                                  paddingLeft: 6,
                                  paddingRight: 6,
                                  height: 18,
                                  borderRadius: 4,
                                  backgroundColor: '#8b5cf614',
                                  borderWidth: 1,
                                  borderColor: '#8b5cf630',
                                }}
                              >
                                <text
                                  style={{
                                    fontSize: 10.5,
                                    fontFamily: FONT_MONO,
                                    color: '#8b5cf6',
                                  }}
                                >
                                  {`/${p.name}`}
                                </text>
                                {p.argumentHint ? (
                                  <text
                                    style={{
                                      fontSize: 9.5,
                                      color: C.faint,
                                      fontFamily: FONT_MONO,
                                    }}
                                  >
                                    {p.argumentHint}
                                  </text>
                                ) : null}
                              </div>
                            ))}
                          </div>
                          {item.prompts[0]?.description ? (
                            <text
                              style={{
                                fontSize: 11,
                                lineHeight: 15,
                                color: C.faint,
                                paddingLeft: 2,
                              }}
                            >
                              {item.prompts[0].description}
                            </text>
                          ) : null}
                        </div>
                      ) : null}

                      {item.tools.length === 0 && (!item.skills || item.skills.length === 0) && (!item.prompts || item.prompts.length === 0) ? (
                        <text style={{ fontSize: 10.5, color: C.faint }}>
                          未检测到已导出的 Agent 工具、技能或提示词
                        </text>
                      ) : null}

                      {/* 加载状态与诊断（M3-1）：不只在出问题时才有话说——
                          正常情况下这里什么都不显示，出问题时用户要能一眼看出
                          是"待配置""版本不兼容"还是"加载失败"，以及为什么 */}
                      {statusBadge ? (
                        <div
                          style={{
                            display: 'flex',
                            flexDirection: 'row',
                            alignItems: 'center',
                            gap: 6,
                            flexWrap: 'wrap',
                          }}
                        >
                          <div
                            style={{
                              display: 'flex',
                              flexDirection: 'row',
                              alignItems: 'center',
                              height: 18,
                              paddingLeft: 6,
                              paddingRight: 6,
                              borderRadius: 4,
                              backgroundColor: statusBadge.background,
                              flexShrink: 0,
                            }}
                          >
                            <text
                              style={{
                                fontSize: 10,
                                lineHeight: 14,
                                color: statusBadge.color,
                                whiteSpace: 'nowrap',
                              }}
                            >
                              {statusBadge.label}
                            </text>
                          </div>
                          {item.version ? (
                            <text style={{ fontSize: 10, color: C.faint }}>
                              {`v${item.version}`}
                            </text>
                          ) : null}
                        </div>
                      ) : null}

                      {/* 贡献计数（M3-1 后半）：一眼看清这个插件到底带来了什么 */}
                      <text style={{ fontSize: 10.5, color: C.faint }}>
                        {`贡献：工具 ${item.tools.length} · 技能 ${item.skills.length} · 提示词 ${item.prompts.length}`}
                      </text>

                      {item.diagnostics.map((diagnostic, index) => (
                        <text
                          key={`diag-${index}`}
                          style={{
                            fontSize: 10.5,
                            color: diagnostic.level === 'error' ? C.accent : C.faint,
                          }}
                        >
                          {`${diagnostic.level === 'error' ? '✕' : '!'} ${diagnostic.message}${
                            diagnostic.hint ? ` —— ${diagnostic.hint}` : ''
                          }`}
                        </text>
                      ))}

                      {/* 受限原因（M3-2）：关掉某个开关后，用到它的插件必须说清楚
                          自己哪一步会被忽略，而不是静默失效 */}
                      {restrictions.length > 0 ? (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                          {restrictions.map((reason, index) => (
                            <text key={`limit-${index}`} style={{ fontSize: 10.5, color: '#b45309' }}>
                              {`受限：${reason}`}
                            </text>
                          ))}
                        </div>
                      ) : null}

                      {/* 配置表单（M3-3）：由 configSchema 生成；secret 不回显，
                          只显示"已设置/未设置"，留空表示不改 */}
                      {item.plugin.contributions.configSchema ? (
                        <div
                          style={{
                            display: 'flex',
                            flexDirection: 'column',
                            gap: 6,
                            paddingTop: 6,
                            borderTopWidth: 1,
                            borderColor: C.border,
                          }}
                        >
                          <text style={{ fontSize: 11, color: C.faint }}>插件配置</text>
                          {Object.entries(item.plugin.contributions.configSchema.properties).map(
                            ([key, property]) => {
                              const draft = (configDrafts[item.id] ?? {})[key] ?? ''
                              const isSecret = property.type === 'secret'
                              const alreadySet = secretSet[`${item.id}:${key}`]
                              return (
                                <div
                                  key={key}
                                  style={{
                                    display: 'flex',
                                    flexDirection: 'row',
                                    alignItems: 'center',
                                    gap: 6,
                                  }}
                                >
                                  <text
                                    style={{ fontSize: 11, color: C.text, width: 130, whiteSpace: 'nowrap' }}
                                  >
                                    {property.title}
                                  </text>
                                  {property.type === 'boolean' ? (
                                    <div
                                      testId={`plugin-config-${item.id}-${key}`}
                                      role="button"
                                      onClick={() =>
                                        setConfigDrafts((current) => ({
                                          ...current,
                                          [item.id]: {
                                            ...(current[item.id] ?? {}),
                                            [key]: draft === 'true' ? 'false' : 'true',
                                          },
                                        }))
                                      }
                                      style={{
                                        display: 'flex',
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        height: 22,
                                        paddingLeft: 8,
                                        paddingRight: 8,
                                        borderRadius: 11,
                                        cursor: 'pointer',
                                        backgroundColor: draft === 'true' ? '#10b98126' : C.chip,
                                        borderWidth: 1,
                                        borderColor: draft === 'true' ? C.success : C.borderStrong,
                                      }}
                                    >
                                      <text style={{ fontSize: 10, color: draft === 'true' ? C.success : C.faint }}>
                                        {draft === 'true' ? '开' : '关'}
                                      </text>
                                    </div>
                                  ) : (
                                    <input
                                      testId={`plugin-config-${item.id}-${key}`}
                                      value={isSecret ? '' : draft}
                                      placeholder={
                                        isSecret
                                          ? alreadySet
                                            ? '已设置（留空表示不改）'
                                            : '未设置'
                                          : property.description || property.title
                                      }
                                      theme={editorTheme()}
                                      style={{
                                        flexGrow: 1,
                                        height: 26,
                                        paddingLeft: 8,
                                        paddingRight: 8,
                                        borderRadius: 6,
                                        fontSize: 11.5,
                                        color: C.text,
                                        backgroundColor: C.card,
                                        borderWidth: 1,
                                        borderColor: C.borderStrong,
                                      }}
                                      onChange={(e) =>
                                        setConfigDrafts((current) => ({
                                          ...current,
                                          [item.id]: { ...(current[item.id] ?? {}), [key]: e.value ?? '' },
                                        }))
                                      }
                                    />
                                  )}
                                </div>
                              )
                            }
                          )}
                          <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 8 }}>
                            <div
                              testId={`plugin-config-save-${item.id}`}
                              role="button"
                              onClick={() => void handleSavePluginConfig(item)}
                              style={{
                                display: 'flex',
                                flexDirection: 'row',
                                alignItems: 'center',
                                height: 24,
                                paddingLeft: 10,
                                paddingRight: 10,
                                borderRadius: 6,
                                cursor: 'pointer',
                                backgroundColor: C.link,
                              }}
                            >
                              <text style={{ fontSize: 11, color: '#fff' }}>保存配置</text>
                            </div>
                            {configNotice[item.id] ? (
                              <text style={{ fontSize: 11, color: C.faint }}>{configNotice[item.id]}</text>
                            ) : null}
                          </div>
                        </div>
                      ) : null}
                    </div>
                    )
                  })
                )}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}

function SubagentsPanel({
  subagents,
  filter,
  setFilter,
  expandedIds,
  toggleExpand,
  armedDeleteId,
  onToggle,
  onDelete,
  onRefresh,
  loading,
}: {
  subagents: SubagentProfile[]
  filter: 'all' | 'builtin' | 'workspace' | 'global'
  setFilter: (f: 'all' | 'builtin' | 'workspace' | 'global') => void
  expandedIds: Record<string, boolean>
  toggleExpand: (id: string) => void
  armedDeleteId: string | null
  onToggle: (item: SubagentProfile) => void
  onDelete: (item: SubagentProfile) => void
  onRefresh: () => void
  loading: boolean
}) {
  const filtered = subagents.filter((item) => {
    if (filter === 'all') return true
    return item.scope === filter
  })

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      {/* 顶部过滤药丸与刷新栏 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 8,
          paddingBottom: 4,
          borderBottomWidth: 1,
          borderColor: C.border,
        }}
      >
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 4, flexGrow: 1, minWidth: 0 }}>
          {[
            { id: 'all', label: '全部' },
            { id: 'builtin', label: '内置预装' },
            { id: 'workspace', label: '工作区项目' },
            { id: 'global', label: '全局通用' },
          ].map((f) => {
            const isFilterActive = filter === f.id
            return (
              <div
                key={f.id}
                role="button"
                onClick={() => setFilter(f.id as any)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  height: 24,
                  paddingLeft: 10,
                  paddingRight: 10,
                  borderRadius: 12,
                  cursor: 'pointer',
                  backgroundColor: isFilterActive ? C.link : C.chip,
                  hover: { backgroundColor: isFilterActive ? C.link : C.chipHover },
                }}
              >
                <text
                  style={{
                    fontSize: 11,
                    fontWeight: isFilterActive ? 600 : 400,
                    color: isFilterActive ? '#ffffff' : C.secondary,
                  }}
                >
                  {f.label}
                </text>
              </div>
            )
          })}
        </div>

        {/* 刷新按钮 */}
        <div
          role="button"
          onClick={onRefresh}
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 4,
            height: 26,
            paddingLeft: 8,
            paddingRight: 8,
            borderRadius: 6,
            cursor: 'pointer',
            backgroundColor: C.chip,
            hover: { backgroundColor: C.chipHover },
          }}
        >
          <Icon name="refresh" size={11} color={C.secondary} />
          <text style={{ fontSize: 11, color: C.secondary }}>
            {loading ? '刷新中…' : '刷新'}
          </text>
        </div>
      </div>

      {/* 说明横幅 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'flex-start',
          gap: 10,
          padding: 10,
          borderRadius: 8,
          backgroundColor: C.overlay,
          borderWidth: 1,
          borderColor: C.border,
        }}
      >
        <div style={{ flexShrink: 0, paddingTop: 1 }}>
          <Icon name="bot" size={16} color={C.link} />
        </div>
        <text
          style={{
            fontSize: 11.5,
            lineHeight: 17,
            color: C.secondary,
            flexGrow: 1,
            minWidth: 0,
            whiteSpace: 'normal',
          }}
        >
          子智能体拥有专属提示词、隔离上下文与工具白名单。主 Agent 遇到深层探索、代码审查或测试任务时，会自动通过 invoke_subagent 委派调度，避免主会话上下文过度膨胀。
        </text>
      </div>

      {/* 列表渲染 */}
      {filtered.length === 0 ? (
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            padding: 32,
            gap: 8,
          }}
        >
          <Icon name="bot" size={24} color={C.faint} />
          <text style={{ fontSize: 12, color: C.faint }}>暂无符合条件的子智能体</text>
        </div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          {filtered.map((item) => {
            const isExpanded = Boolean(expandedIds[item.id])
            const isDeleteArmed = armedDeleteId === item.id

            return (
              <div
                key={item.id}
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 8,
                  padding: 12,
                  borderRadius: 8,
                  backgroundColor: C.raised,
                  borderWidth: 1,
                  borderColor: item.enabled ? C.borderStrong : C.border,
                  opacity: item.enabled ? 1 : 0.7,
                }}
              >
                {/* 头部标题与开关 */}
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    gap: 8,
                  }}
                >
                  <Icon
                    name={(item.icon as IconName) ?? 'bot'}
                    size={14}
                    color={item.enabled ? C.link : C.faint}
                  />
                  {item.color ? (
                    <div
                      style={{
                        width: 8,
                        height: 8,
                        borderRadius: 4,
                        backgroundColor: SUBAGENT_HEX_COLORS[item.color] ?? '#3b82f6',
                        flexShrink: 0,
                      }}
                    />
                  ) : null}
                  <text
                    style={{
                      fontSize: 12.5,
                      fontWeight: 600,
                      color: C.text,
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {item.name}
                  </text>
                  <text style={{ fontFamily: FONT_MONO, fontSize: 11, color: C.tertiary }}>
                    {`[${item.id}]`}
                  </text>

                  {/* 作用域徽章 */}
                  <div
                    style={{
                      paddingLeft: 6,
                      paddingRight: 6,
                      height: 18,
                      borderRadius: 4,
                      backgroundColor: C.chip,
                      display: 'flex',
                      alignItems: 'center',
                    }}
                  >
                    <text style={{ fontSize: 10, lineHeight: 14, color: C.faint, whiteSpace: 'nowrap' }}>
                      {item.scope === 'builtin'
                        ? '内置预装'
                        : item.scope === 'workspace'
                        ? '工作区'
                        : '全局'}
                    </text>
                  </div>

                  {/* 模式徽章 */}
                  <div
                    style={{
                      paddingLeft: 6,
                      paddingRight: 6,
                      height: 18,
                      borderRadius: 4,
                      backgroundColor: item.mode === 'readonly' ? '#10b98118' : '#f59e0b18',
                      display: 'flex',
                      alignItems: 'center',
                    }}
                  >
                    <text
                      style={{
                        fontSize: 10,
                        lineHeight: 14,
                        color: item.mode === 'readonly' ? C.success : '#f59e0b',
                        fontWeight: 500,
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {item.mode === 'readonly' ? '只读安全' : '读写模式'}
                    </text>
                  </div>

                  {/* 最大步数 */}
                  <text style={{ fontSize: 10.5, color: C.faint, whiteSpace: 'nowrap' }}>
                    {item.maxSteps ? `最多 ${item.maxSteps} 步` : '无步数限制'}
                  </text>

                  <div style={{ flexGrow: 1 }} />

                  {/* 启用/停用按钮开关 */}
                  <div
                    role="button"
                    onClick={() => onToggle(item)}
                    style={{
                      display: 'flex',
                      flexDirection: 'row',
                      alignItems: 'center',
                      gap: 4,
                      height: 22,
                      paddingLeft: 7,
                      paddingRight: 7,
                      borderRadius: 11,
                      cursor: 'pointer',
                      backgroundColor: item.enabled ? '#10b98122' : C.chip,
                      borderWidth: 1,
                      borderColor: item.enabled ? '#10b98155' : C.border,
                    }}
                  >
                    <div
                      style={{
                        width: 7,
                        height: 7,
                        borderRadius: 4,
                        backgroundColor: item.enabled ? C.success : C.faint,
                      }}
                    />
                    <text
                      style={{
                        fontSize: 10.5,
                        fontWeight: 500,
                        color: item.enabled ? C.success : C.faint,
                      }}
                    >
                      {item.enabled ? '已启用' : '已停用'}
                    </text>
                  </div>
                </div>

                {/* 描述信息 */}
                <text
                  style={{
                    fontSize: 11.5,
                    lineHeight: 16,
                    color: C.secondary,
                    flexGrow: 1,
                    minWidth: 0,
                    whiteSpace: 'normal',
                  }}
                >
                  {item.description}
                </text>

                {/* 授权工具白名单 */}
                <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                  <text style={{ fontSize: 10.5, color: C.faint }}>授权工具：</text>
                  {item.allowedTools.map((t) => (
                    <div
                      key={t}
                      style={{
                        paddingLeft: 5,
                        paddingRight: 5,
                        height: 17,
                        borderRadius: 3,
                        backgroundColor: C.overlay,
                        display: 'flex',
                        alignItems: 'center',
                      }}
                    >
                      <text style={{ fontFamily: FONT_MONO, fontSize: 10, color: C.tertiary }}>
                        {t}
                      </text>
                    </div>
                  ))}
                </div>

                {/* 排除工具黑名单 */}
                {item.disallowedTools && item.disallowedTools.length > 0 ? (
                  <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                    <text style={{ fontSize: 10.5, color: C.faint }}>排除工具：</text>
                    {item.disallowedTools.map((t) => (
                      <div
                        key={t}
                        style={{
                          paddingLeft: 5,
                          paddingRight: 5,
                          height: 17,
                          borderRadius: 3,
                          backgroundColor: '#ef444415',
                          display: 'flex',
                          alignItems: 'center',
                        }}
                      >
                        <text style={{ fontFamily: FONT_MONO, fontSize: 10, color: '#ef4444' }}>
                          {t}
                        </text>
                      </div>
                    ))}
                  </div>
                ) : null}

                {/* 展开系统提示词 & 删除操作行 */}
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                    paddingTop: 4,
                    borderTopWidth: 1,
                    borderColor: C.border,
                  }}
                >
                  <div
                    role="button"
                    onClick={() => toggleExpand(item.id)}
                    style={{
                      display: 'flex',
                      flexDirection: 'row',
                      alignItems: 'center',
                      gap: 4,
                      cursor: 'pointer',
                    }}
                  >
                    <Icon
                      name={isExpanded ? 'chevronUp' : 'chevronDown'}
                      size={11}
                      color={C.link}
                    />
                    <text style={{ fontSize: 11, color: C.link }}>
                      {isExpanded ? '收起专属提示词' : '查看专属提示词'}
                    </text>
                  </div>

                  {item.scope !== 'builtin' ? (
                    <div
                      role="button"
                      onClick={() => onDelete(item)}
                      style={{
                        display: 'flex',
                        flexDirection: 'row',
                        alignItems: 'center',
                        gap: 3,
                        paddingLeft: 6,
                        paddingRight: 6,
                        height: 20,
                        borderRadius: 4,
                        cursor: 'pointer',
                        backgroundColor: isDeleteArmed ? '#ef444422' : C.chip,
                        borderWidth: isDeleteArmed ? 1 : 0,
                        borderColor: C.danger,
                      }}
                    >
                      <Icon name="trash" size={10} color={isDeleteArmed ? C.danger : C.faint} />
                      <text
                        style={{
                          fontSize: 10.5,
                          color: isDeleteArmed ? C.danger : C.faint,
                        }}
                      >
                        {isDeleteArmed ? '确认删除?' : '删除'}
                      </text>
                    </div>
                  ) : null}
                </div>

                {/* 展开的系统提示词内容 */}
                {isExpanded ? (
                  <div
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      padding: 8,
                      borderRadius: 6,
                      backgroundColor: C.card,
                      borderWidth: 1,
                      borderColor: C.cardBorder,
                    }}
                  >
                    <markdown source={item.systemPrompt} theme={docTheme()} />
                  </div>
                ) : null}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
