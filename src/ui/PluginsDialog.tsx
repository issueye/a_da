/**
 * 插件管理模态弹窗组件
 * 采用全屏遮罩居中卡片布局，支持工作区插件与全局插件的扫描、启用/停用、新建模板、删除以及内置核心工具一览。
 */

import React, { useEffect, useState } from 'react'
import type { AgentStore } from '../agent/store'
import {
  defaultExtensionLoader,
  type PluginItem,
} from '../agent/tools/loader'
import { BUILTIN_TOOLS_METADATA } from '../agent/tools/registry'
import { defaultPromptManager } from '../agent/prompts/manager'
import type { PromptItem } from '../agent/prompts/types'
import { defaultSubagentManager, type SubagentProfile, SUBAGENT_HEX_COLORS } from '../agent/subagents'
import { defaultSkillManager, type SkillSummary } from '../agent/skills'
import { SkillsPanel } from './SkillsPanel'
import { copyToClipboard } from '../platform/clipboard'
import { getAppHome } from '../agent/home'
import { C, docTheme, editorTheme, FONT_MONO, M } from '../theme'
import { Icon, IconButton } from './controls'
import type { IconName } from '../icons'
import { join } from 'node:path'

type TabType = 'skills' | 'subagents' | 'prompts' | 'workspace' | 'global' | 'builtins'

const TABS: { id: TabType; label: string; icon: IconName }[] = [
  { id: 'skills', label: '技能库 (Skills)', icon: 'zap' },
  { id: 'subagents', label: '子智能体', icon: 'bot' },
  { id: 'prompts', label: '提示词管理', icon: 'sparkles' },
  { id: 'workspace', label: '工作区插件', icon: 'folder' },
  { id: 'global', label: '全局插件', icon: 'settings' },
  { id: 'builtins', label: '内置核心工具', icon: 'shield' },
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

export function PluginsDialog({ store }: { store: AgentStore }) {
  const [tab, setTab] = useState<TabType>('workspace')
  const [plugins, setPlugins] = useState<PluginItem[]>([])
  const [loading, setLoading] = useState(false)
  const [armedDeleteId, setArmedDeleteId] = useState<string | null>(null)

  // 内置工具过滤状态
  const [toolFilter, setToolFilter] = useState<'all' | 'readonly' | 'write'>('all')

  // 提示词管理状态
  const [prompts, setPrompts] = useState<PromptItem[]>([])
  const [promptFilter, setPromptFilter] = useState<'all' | 'builtin' | 'workspace' | 'global'>('all')
  const [creatingPrompt, setCreatingPrompt] = useState(false)
  const [editingPromptId, setEditingPromptId] = useState<string | null>(null)
  const [promptName, setPromptName] = useState('')
  const [promptDesc, setPromptDesc] = useState('')
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

  // 加载与刷新插件列表、提示词列表、子智能体与技能库
  const refreshList = async () => {
    setLoading(true)
    try {
      const [pluginItems, promptItems, subagentItems, skillItems] = await Promise.all([
        defaultExtensionLoader.scanPlugins(store.project),
        defaultPromptManager.scanPrompts(store.project),
        defaultSubagentManager.getSubagents(store.project),
        defaultSkillManager.scanSkills(store.project),
      ])
      setPlugins(pluginItems)
      setPrompts(promptItems)
      setSubagents(subagentItems)
      setSkills(skillItems)
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    void refreshList()
  }, [store.project])

  // 切换子智能体启用状态
  const handleToggleSubagent = async (item: SubagentProfile) => {
    await defaultSubagentManager.toggleSubagent(item.id, !item.enabled, store.project)
    store.trace(`已${item.enabled ? '停用' : '启用'}子智能体：${item.name}`)
    await refreshList()
  }

  // 删除自定义子智能体
  const handleDeleteSubagent = async (item: SubagentProfile) => {
    if (armedDeleteSubagentId !== item.id) {
      setArmedDeleteSubagentId(item.id)
      return
    }
    const success = await defaultSubagentManager.deleteSubagent(item.id, store.project)
    if (success) {
      store.trace(`已删除子智能体：${item.name}`)
    } else {
      store.trace(`删除子智能体失败：${item.name}`)
    }
    setArmedDeleteSubagentId(null)
    await refreshList()
  }

  const toggleSubagentExpand = (id: string) => {
    setExpandedSubagentIds((prev) => ({
      ...prev,
      [id]: !prev[id],
    }))
  }

  // 切换插件启用状态
  const handleToggle = async (item: PluginItem) => {
    await defaultExtensionLoader.togglePlugin(item.id, !item.enabled, store.project)
    store.trace(`已${item.enabled ? '停用' : '启用'}插件：${item.fileName}`)
    await refreshList()
  }

  // 删除插件
  const handleDelete = async (item: PluginItem) => {
    if (armedDeleteId !== item.id) {
      setArmedDeleteId(item.id)
      return
    }
    const success = await defaultExtensionLoader.deletePlugin(item.filePath, store.project)
    if (success) {
      store.trace(`已删除插件文件：${item.fileName}`)
    } else {
      store.trace(`删除插件文件失败：${item.fileName}`)
    }
    setArmedDeleteId(null)
    await refreshList()
  }

  // 提交新建插件
  const handleCreate = async () => {
    const name = newPluginName.trim()
    if (!name) return
    try {
      const targetScope = tab === 'global' ? 'global' : 'workspace'
      const filePath = await defaultExtensionLoader.createPluginTemplate(
        store.project,
        targetScope,
        name
      )
      store.trace(`已创建新插件模板：${filePath}`)
      setNewPluginName('')
      setCreating(false)
      setCreateNotice(null)
      await refreshList()
    } catch (err) {
      setCreateNotice(`创建失败：${(err as Error).message}`)
    }
  }

  // 切换提示词启用状态
  const handleTogglePrompt = async (item: PromptItem) => {
    await defaultPromptManager.togglePrompt(item.id, !item.enabled, store.project)
    store.trace(`已${item.enabled ? '停用' : '启用'}提示词：${item.name}`)
    await refreshList()
  }

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
    setPromptContent(item.content)
    setPromptScope(item.scope === 'global' ? 'global' : 'workspace')
    setPromptIsSystem(item.isSystem)
    setPromptNotice(null)
  }

  // 取消新建/编辑提示词
  const cancelPromptForm = () => {
    setCreatingPrompt(false)
    setEditingPromptId(null)
    setPromptNotice(null)
  }

  // 提交新建或保存编辑提示词
  const handleSavePrompt = async () => {
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
          target.content = content
          target.isSystem = promptIsSystem
          await defaultPromptManager.updatePrompt(target)
          store.trace(`已更新提示词：${name}`)
        }
      } else {
        await defaultPromptManager.createPrompt(store.project, {
          name,
          description: promptDesc.trim(),
          content,
          scope: promptScope,
          isSystem: promptIsSystem,
          enabled: true,
        })
        store.trace(`已创建新提示词：${name}`)
      }
      cancelPromptForm()
      await refreshList()
    } catch (err) {
      setPromptNotice(`保存失败：${(err as Error).message}`)
    }
  }

  // 删除提示词
  const handleDeletePrompt = async (item: PromptItem) => {
    if (armedDeletePromptId !== item.id) {
      setArmedDeletePromptId(item.id)
      return
    }
    if (item.filePath) {
      const success = await defaultPromptManager.deletePrompt(item.filePath)
      if (success) {
        store.trace(`已删除提示词：${item.name}`)
      } else {
        store.trace(`删除提示词失败：${item.name}`)
      }
    }
    setArmedDeletePromptId(null)
    await refreshList()
  }

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
  const currentPlugins = plugins.filter((p) => p.scope === tab)
  const workspaceCount = plugins.filter((p) => p.scope === 'workspace').length
  const globalCount = plugins.filter((p) => p.scope === 'global').length

  const currentDir =
    tab === 'workspace'
      ? join(store.project, '.ada', 'extensions')
      : join(getAppHome(), 'extensions')

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
            onClick={() => store.setPlugins(false)}
          />
        </div>

        {/* 标题栏与主体之间的分割线 */}
        <div style={{ height: 1, flexShrink: 0, backgroundColor: C.border }} />

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
                  : item.id === 'workspace'
                  ? workspaceCount
                  : item.id === 'global'
                  ? globalCount
                  : BUILTIN_TOOLS_METADATA.length
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
                skills={skills}
                onRefresh={refreshList}
                loading={loading}
                workspaceRoot={store.project}
                onTrace={(msg) => store.trace(msg)}
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
                                  backgroundColor: C.chip,
                                  display: 'flex',
                                  alignItems: 'center',
                                  flexShrink: 0,
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
                                onClick={() => store.applyPromptToComposer(item.content)}
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

                              {/* 编辑与删除（仅自定义提示词可编辑/删除） */}
                              {!isBuiltin ? (
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
                  const filteredTools = BUILTIN_TOOLS_METADATA.filter((bt) => {
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
                  currentPlugins.map((item) => (
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
                            backgroundColor: C.chip,
                            flexShrink: 0,
                          }}
                        >
                          <text
                            style={{
                              fontSize: 10,
                              lineHeight: 14,
                              color: C.faint,
                              whiteSpace: 'nowrap',
                            }}
                          >
                            {`${Math.max(1, Math.round(item.sizeBytes / 1024))} KB`}
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

                        {/* 删除插件按钮 (带二次确认) */}
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
                      ) : (
                        <text style={{ fontSize: 10.5, color: C.faint }}>
                          未检测到已导出的 Agent 工具
                        </text>
                      )}

                      {/* 错误提示 */}
                      {item.error ? (
                        <text style={{ fontSize: 10.5, color: C.accent }}>
                          {`加载告警: ${item.error}`}
                        </text>
                      ) : null}
                    </div>
                  ))
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
