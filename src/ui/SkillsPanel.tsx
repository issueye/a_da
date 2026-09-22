/**
 * 技能库管理面板组件 (SkillsPanel)
 * 参考 ZCode Skills 架构设计
 * 支持多作用域技能浏览、启停开关、正文查看、新建技能模板与删除
 */

import React, { useState } from 'react'
import { defaultSkillManager, type SkillSummary } from '../agent/skills'
import { copyToClipboard } from '../platform/clipboard'
import { C, FONT_MONO } from '../theme'
import { Icon, IconButton } from './controls'

export function SkillsPanel({
  skills,
  onRefresh,
  loading,
  workspaceRoot,
  onTrace,
}: {
  skills: SkillSummary[]
  onRefresh: () => Promise<void>
  loading?: boolean
  workspaceRoot?: string
  onTrace?: (msg: string) => void
}) {
  const [filter, setFilter] = useState<'all' | 'builtin' | 'workspace' | 'global' | 'plugin'>('all')
  const [search, setSearch] = useState('')
  const [expandedIds, setExpandedIds] = useState<Record<string, boolean>>({})
  const [armedDeleteId, setArmedDeleteId] = useState<string | null>(null)
  const [copiedPathId, setCopiedPathId] = useState<string | null>(null)

  // 新建技能表单状态
  const [creating, setCreating] = useState(false)
  const [newSkillName, setNewSkillName] = useState('')
  const [newSkillDesc, setNewSkillDesc] = useState('')
  const [newSkillScope, setNewSkillScope] = useState<'workspace' | 'global'>('workspace')
  const [newSkillBody, setNewSkillBody] = useState('')
  const [createNotice, setCreateNotice] = useState<string | null>(null)

  // 过滤技能列表
  const filteredSkills = skills.filter((s) => {
    if (filter !== 'all' && s.scope !== filter) return false
    if (search.trim()) {
      const q = search.trim().toLowerCase()
      return s.name.toLowerCase().includes(q) || s.description.toLowerCase().includes(q)
    }
    return true
  })

  // 切换展开/收起
  const toggleExpand = (id: string) => {
    setExpandedIds((prev) => ({ ...prev, [id]: !prev[id] }))
  }

  // 切换启用/停用
  const handleToggle = async (skill: SkillSummary) => {
    const nextState = !skill.enabled
    await defaultSkillManager.toggleSkill(skill.id, nextState)
    onTrace?.(`已${nextState ? '启用' : '停用'}技能：${skill.name}`)
    await onRefresh()
  }

  // 删除技能
  const handleDelete = async (skill: SkillSummary) => {
    if (armedDeleteId !== skill.id) {
      setArmedDeleteId(skill.id)
      return
    }
    try {
      await defaultSkillManager.deleteSkill(skill.id, workspaceRoot)
      onTrace?.(`已删除技能：${skill.name}`)
      setArmedDeleteId(null)
      await onRefresh()
    } catch (err) {
      onTrace?.(`删除技能失败：${(err as Error).message}`)
    }
  }

  // 复制路径
  const handleCopyPath = async (skill: SkillSummary) => {
    const ok = await copyToClipboard(skill.path)
    if (ok) {
      setCopiedPathId(skill.id)
      setTimeout(() => setCopiedPathId(null), 1500)
    }
  }

  // 提交新建技能
  const handleCreateSkill = async () => {
    const name = newSkillName.trim()
    const desc = newSkillDesc.trim()
    if (!name) {
      setCreateNotice('请输入技能短名称')
      return
    }
    if (!desc) {
      setCreateNotice('请输入技能描述说明')
      return
    }

    try {
      const createdPath = await defaultSkillManager.createSkillTemplate({
        name,
        description: desc,
        scope: newSkillScope,
        workspaceRoot,
        body: newSkillBody.trim() || undefined,
      })
      onTrace?.(`已创建技能模板：${createdPath}`)
      setCreating(false)
      setNewSkillName('')
      setNewSkillDesc('')
      setNewSkillBody('')
      setCreateNotice(null)
      await onRefresh()
    } catch (err) {
      setCreateNotice(`创建失败：${(err as Error).message}`)
    }
  }

  return (
    <div testId="skills-panel" style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      {/* 顶部工具条：过滤器与新建按键 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 8,
          paddingBottom: 6,
          borderBottomWidth: 1,
          borderColor: C.border,
        }}
      >
        {/* 作用域 Filter Chips */}
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 4, flexGrow: 1, minWidth: 0 }}>
          {[
            { id: 'all', label: '全部' },
            { id: 'builtin', label: '系统预装' },
            { id: 'workspace', label: '工作区项目' },
            { id: 'global', label: '全局通用' },
            { id: 'plugin', label: '插件内建' },
          ].map((f) => {
            const isFilterActive = filter === f.id
            const count = f.id === 'all' ? skills.length : skills.filter((s) => s.scope === f.id).length
            return (
              <div
                key={f.id}
                testId={`skill-filter-${f.id}`}
                role="button"
                onClick={() => setFilter(f.id as any)}
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
                <text style={{ fontSize: 10, color: C.faint }}>
                  {count}
                </text>
              </div>
            )
          })}
        </div>

        {/* 搜索框 */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 4,
            height: 26,
            paddingLeft: 8,
            paddingRight: 8,
            backgroundColor: C.overlay,
            borderRadius: 6,
            borderWidth: 1,
            borderColor: C.border,
            width: 140,
          }}
        >
          <Icon name="search" size={11} color={C.tertiary} />
          <input
            testId="skill-search-input"
            value={search}
            onChange={(e: any) => setSearch(e?.value ?? e?.target?.value ?? '')}
            placeholder="搜索技能..."
            style={{
              flexGrow: 1,
              backgroundColor: 'transparent',
              fontSize: 11,
              color: C.text,
              borderWidth: 0,
            }}
          />
        </div>

        {/* 新建技能按键 */}
        <div
          testId="skill-create-btn"
          role="button"
          onClick={() => {
            setCreating(!creating)
            setCreateNotice(null)
          }}
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 4,
            height: 26,
            paddingLeft: 10,
            paddingRight: 10,
            borderRadius: 6,
            cursor: 'pointer',
            backgroundColor: creating ? C.chipHover : C.chip,
            borderWidth: 1,
            borderColor: C.borderStrong,
          }}
        >
          <Icon name={creating ? 'close' : 'plus'} size={11} color={C.text} />
          <text style={{ fontSize: 11, fontWeight: 500, color: C.text }}>
            {creating ? '取消' : '新建技能'}
          </text>
        </div>
      </div>

      {/* 新建技能表单卡片 */}
      {creating ? (
        <div
          testId="skill-create-form"
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 10,
            padding: 12,
            backgroundColor: C.raised,
            borderRadius: 8,
            borderWidth: 1,
            borderColor: C.accent,
          }}
        >
          <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
            <Icon name="zap" size={14} color={C.accent} />
            <text style={{ fontSize: 12, fontWeight: 600, color: C.text }}>
              新建技能规范 (SKILL.md)
            </text>
          </div>

          <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            <text style={{ fontSize: 11, color: C.secondary }}>技能短名称 (Slug)</text>
            <input
              testId="skill-input-name"
              value={newSkillName}
              onChange={(e: any) => setNewSkillName(e?.value ?? e?.target?.value ?? '')}
              placeholder="例如: code-reviewer, sql-optimizer"
              style={{
                height: 28,
                paddingLeft: 8,
                paddingRight: 8,
                backgroundColor: C.overlay,
                borderRadius: 5,
                borderWidth: 1,
                borderColor: C.border,
                fontSize: 11,
                color: C.text,
              }}
            />
          </div>

          <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            <text style={{ fontSize: 11, color: C.secondary }}>技能功能描述与适用场景</text>
            <input
              testId="skill-input-desc"
              value={newSkillDesc}
              onChange={(e: any) => setNewSkillDesc(e?.value ?? e?.target?.value ?? '')}
              placeholder="简述该技能专注于解决什么问题、何时调用"
              style={{
                height: 28,
                paddingLeft: 8,
                paddingRight: 8,
                backgroundColor: C.overlay,
                borderRadius: 5,
                borderWidth: 1,
                borderColor: C.border,
                fontSize: 11,
                color: C.text,
              }}
            />
          </div>

          <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 12 }}>
            <text style={{ fontSize: 11, color: C.secondary }}>作用域：</text>
            <div
              role="button"
              onClick={() => setNewSkillScope('workspace')}
              style={{
                display: 'flex',
                flexDirection: 'row',
                alignItems: 'center',
                gap: 4,
                cursor: 'pointer',
              }}
            >
              <Icon name={newSkillScope === 'workspace' ? 'circleCheck' : 'circle'} size={12} color={newSkillScope === 'workspace' ? C.link : C.tertiary} />
              <text style={{ fontSize: 11, color: newSkillScope === 'workspace' ? C.text : C.tertiary }}>
                当前工作区 (.ada/skills)
              </text>
            </div>
            <div
              role="button"
              onClick={() => setNewSkillScope('global')}
              style={{
                display: 'flex',
                flexDirection: 'row',
                alignItems: 'center',
                gap: 4,
                cursor: 'pointer',
              }}
            >
              <Icon name={newSkillScope === 'global' ? 'circleCheck' : 'circle'} size={12} color={newSkillScope === 'global' ? C.link : C.tertiary} />
              <text style={{ fontSize: 11, color: newSkillScope === 'global' ? C.text : C.tertiary }}>
                全局用户目录 (~/.ada/skills)
              </text>
            </div>
          </div>

          {createNotice ? (
            <text style={{ fontSize: 11, color: C.danger }}>{createNotice}</text>
          ) : null}

          <div style={{ display: 'flex', flexDirection: 'row', justifyContent: 'flex-end', gap: 8 }}>
            <div
              role="button"
              onClick={() => setCreating(false)}
              style={{
                paddingLeft: 12,
                paddingRight: 12,
                paddingTop: 4,
                paddingBottom: 4,
                borderRadius: 5,
                cursor: 'pointer',
                backgroundColor: C.overlay,
              }}
            >
              <text style={{ fontSize: 11, color: C.tertiary }}>取消</text>
            </div>

            <div
              testId="skill-submit-create"
              role="button"
              onClick={handleCreateSkill}
              style={{
                paddingLeft: 14,
                paddingRight: 14,
                paddingTop: 4,
                paddingBottom: 4,
                borderRadius: 5,
                cursor: 'pointer',
                backgroundColor: C.link,
              }}
            >
              <text style={{ fontSize: 11, fontWeight: 600, color: '#ffffff' }}>创建技能</text>
            </div>
          </div>
        </div>
      ) : null}

      {/* 技能卡片列表 */}
      {filteredSkills.length === 0 ? (
        <div style={{ padding: 24, display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 8 }}>
          <Icon name="zap" size={24} color={C.faint} />
          <text style={{ fontSize: 12, color: C.faint }}>
            {skills.length === 0
              ? '当前尚未配置任何技能。点击右上角【新建技能】即可创建首个技能规范。'
              : '未找到符合条件的技能。'}
          </text>
        </div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          {filteredSkills.map((skill) => {
            const isExpanded = Boolean(expandedIds[skill.id])
            const scopeColor =
              skill.scope === 'builtin'
                ? '#ec4899'
                : skill.scope === 'workspace'
                ? '#3b82f6'
                : skill.scope === 'global'
                ? '#10b981'
                : '#8b5cf6'
            const scopeLabel =
              skill.scope === 'builtin'
                ? '系统预装'
                : skill.scope === 'workspace'
                ? '工作区'
                : skill.scope === 'global'
                ? '全局'
                : `插件[${skill.pluginName || '内建'}]`

            return (
              <div
                key={skill.id}
                testId={`skill-card-${skill.name}`}
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 6,
                  padding: 10,
                  backgroundColor: C.card,
                  borderRadius: 8,
                  borderWidth: 1,
                  borderColor: skill.enabled ? C.borderStrong : C.border,
                  opacity: skill.enabled ? 1 : 0.65,
                }}
              >
                {/* 技能卡片头部 */}
                <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between' }}>
                  <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 8 }}>
                    <Icon name="zap" size={13} color={scopeColor} />
                    <text style={{ fontFamily: FONT_MONO, fontSize: 12, fontWeight: 600, color: C.text }}>
                      {skill.name}
                    </text>
                    <div
                      style={{
                        paddingLeft: 6,
                        paddingRight: 6,
                        paddingTop: 1,
                        paddingBottom: 1,
                        borderRadius: 4,
                        backgroundColor: C.overlay,
                      }}
                    >
                      <text style={{ fontSize: 10, color: scopeColor, fontWeight: 500 }}>
                        {scopeLabel}
                      </text>
                    </div>

                    {skill.disableModelInvocation ? (
                      <div
                        style={{
                          paddingLeft: 5,
                          paddingRight: 5,
                          paddingTop: 1,
                          paddingBottom: 1,
                          borderRadius: 4,
                          backgroundColor: `${C.accent}18`,
                          borderWidth: 1,
                          borderColor: `${C.accent}30`,
                        }}
                      >
                        <text style={{ fontSize: 9.5, color: C.accent, fontWeight: 500 }}>
                          仅指令唤醒
                        </text>
                      </div>
                    ) : null}

                    {skill.isFileSkill ? (
                      <div
                        style={{
                          paddingLeft: 4,
                          paddingRight: 4,
                          paddingTop: 1,
                          paddingBottom: 1,
                          borderRadius: 3,
                          backgroundColor: C.overlay,
                        }}
                      >
                        <text style={{ fontSize: 9.5, color: C.faint }}>单文件</text>
                      </div>
                    ) : null}
                  </div>

                  {/* 右侧操作区：Switch、展开正文与删除 */}
                  <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 8 }}>
                    {/* 启用/停用 Switch */}
                    <div
                      testId={`skill-toggle-${skill.name}`}
                      role="button"
                      onClick={() => handleToggle(skill)}
                      style={{
                        width: 32,
                        height: 18,
                        borderRadius: 9,
                        cursor: 'pointer',
                        backgroundColor: skill.enabled ? C.link : C.overlay,
                        borderWidth: 1,
                        borderColor: skill.enabled ? C.link : C.borderStrong,
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: skill.enabled ? 'flex-end' : 'flex-start',
                        paddingLeft: 2,
                        paddingRight: 2,
                      }}
                    >
                      <div
                        style={{
                          width: 12,
                          height: 12,
                          borderRadius: 6,
                          backgroundColor: skill.enabled ? '#ffffff' : C.tertiary,
                        }}
                      />
                    </div>

                    {/* 展开查看正文 */}
                    <div
                      testId={`skill-expand-${skill.name}`}
                      role="button"
                      onClick={() => toggleExpand(skill.id)}
                      style={{
                        cursor: 'pointer',
                        padding: 2,
                        opacity: 0.7,
                        hover: { opacity: 1 },
                      }}
                    >
                      <Icon name={isExpanded ? 'chevronUp' : 'chevronDown'} size={13} color={C.secondary} />
                    </div>

                    {/* 删除按键（仅非插件且非系统预置支持） */}
                    {skill.scope !== 'plugin' && skill.scope !== 'builtin' ? (
                      <div
                        testId={`skill-delete-${skill.name}`}
                        role="button"
                        onClick={() => handleDelete(skill)}
                        style={{
                          cursor: 'pointer',
                          padding: 2,
                          opacity: armedDeleteId === skill.id ? 1 : 0.6,
                          hover: { opacity: 1 },
                        }}
                      >
                        <Icon
                          name="trash"
                          size={13}
                          color={armedDeleteId === skill.id ? C.danger : C.tertiary}
                        />
                      </div>
                    ) : null}
                  </div>
                </div>

                {/* 技能描述 */}
                <text style={{ fontSize: 11, lineHeight: 16, color: C.secondary }}>
                  {skill.description}
                </text>

                {/* 适用场景 / 标签 / 插件归属 */}
                <div style={{ display: 'flex', flexDirection: 'row', flexWrap: 'wrap', gap: 8, alignItems: 'center' }}>
                  {skill.metadata?.whenToUse ? (
                    <text style={{ fontSize: 10.5, color: C.tertiary }}>
                      {`适用时机：${skill.metadata.whenToUse}`}
                    </text>
                  ) : null}
                  {skill.allowedTools && skill.allowedTools.length > 0 ? (
                    <text style={{ fontSize: 10.5, color: C.link }}>
                      {`推荐工具：${skill.allowedTools.join(', ')}`}
                    </text>
                  ) : null}
                  {skill.scope === 'plugin' && skill.pluginName ? (
                    <text style={{ fontSize: 10.5, color: C.faint }}>
                      {`所属插件包：${skill.pluginName}（在插件管理中联动控制启停）`}
                    </text>
                  ) : null}
                </div>

                {/* 展开的 Markdown 正文视图 */}
                {isExpanded ? (
                  <div
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 6,
                      marginTop: 4,
                      padding: 10,
                      backgroundColor: C.raised,
                      borderRadius: 6,
                      borderWidth: 1,
                      borderColor: C.borderStrong,
                    }}
                  >
                    <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between' }}>
                      <text style={{ fontSize: 10.5, fontWeight: 600, color: C.accent }}>
                        【SKILL.md 正文指令】
                      </text>

                      {/* 复制文件路径 */}
                      <div
                        role="button"
                        onClick={() => handleCopyPath(skill)}
                        style={{
                          display: 'flex',
                          flexDirection: 'row',
                          alignItems: 'center',
                          gap: 4,
                          cursor: 'pointer',
                          paddingLeft: 6,
                          paddingRight: 6,
                          paddingTop: 2,
                          paddingBottom: 2,
                          borderRadius: 4,
                          backgroundColor: C.overlay,
                        }}
                      >
                        <Icon name={copiedPathId === skill.id ? 'check' : 'copy'} size={11} color={copiedPathId === skill.id ? '#10b981' : C.tertiary} />
                        <text style={{ fontSize: 10, color: copiedPathId === skill.id ? '#10b981' : C.tertiary }}>
                          {copiedPathId === skill.id ? '已复制路径' : '复制文件路径'}
                        </text>
                      </div>
                    </div>

                    <text
                      style={{
                        fontFamily: FONT_MONO,
                        fontSize: 10.5,
                        lineHeight: 16,
                        color: C.text,
                        whiteSpace: 'normal',
                      }}
                    >
                      {skill.body}
                    </text>

                    <div style={{ paddingTop: 4, borderTopWidth: 1, borderColor: C.border }}>
                      <text style={{ fontSize: 10, color: C.faint, fontFamily: FONT_MONO }}>
                        {`路径: ${skill.path}`}
                      </text>
                    </div>
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
