/**
 * 快捷指令（Slash Commands）面板组件
 * 位于输入框顶部浮动层，聚合系统内置指令与插件/工作区提示词模板
 */

import React, { useEffect, useState } from 'react'
import type { AgentStore } from '../agent/store'
import { defaultPromptManager, type PromptItem } from '../agent/prompts'
import { C, FONT_MONO, M } from '../theme'
import { Icon } from './controls'
import type { IconName } from '../icons'

export interface SlashCommandItem {
  id: string
  name: string
  command: string
  description: string
  argumentHint?: string
  category: 'system' | 'builtin' | 'plugin' | 'workspace' | 'global'
  scopeLabel: string
  icon: IconName
  /** 若为即时动作命令，点击直接执行该操作 */
  action?: () => void
}

/** 获取所有系统内置动作指令 */
export function getSystemCommands(store: AgentStore): SlashCommandItem[] {
  return [
    {
      id: 'sys-clear',
      name: 'clear',
      command: '/clear',
      description: '清空当前上下文并新建会话',
      category: 'system',
      scopeLabel: '系统',
      icon: 'plus',
      action: () => {
        store.newThread(store.active.workspace)
      },
    },
    {
      id: 'sys-compact',
      name: 'compact',
      command: '/compact',
      description: '压缩当前会话历史并提取持久化上下文摘要',
      category: 'system',
      scopeLabel: '系统',
      icon: 'sparkles',
      action: () => {
        void store.compactThread(store.active.id, { trigger: 'manual' })
      },
    },
    {
      id: 'sys-code',
      name: 'code',
      command: '/code',
      description: '切换到 Code 模式（具备完整代码读写与执行权限）',
      category: 'system',
      scopeLabel: '系统',
      icon: 'code',
      action: () => {
        store.setMode('code')
      },
    },
    {
      id: 'sys-plan',
      name: 'plan',
      command: '/plan',
      description: '切换到 Plan 模式（只读安全防误写，专注规划与分析）',
      category: 'system',
      scopeLabel: '系统',
      icon: 'compass',
      action: () => {
        store.setMode('plan')
      },
    },
    {
      id: 'sys-create',
      name: 'create',
      command: '/create',
      description: '切换到 Create 模式（激活工具与技能元编程 CRUD）',
      category: 'system',
      scopeLabel: '系统',
      icon: 'sparkles',
      action: () => {
        store.setMode('create')
      },
    },
    {
      id: 'sys-settings',
      name: 'settings',
      command: '/settings',
      description: '打开系统配置面板（模型供应商、API Key 与环境）',
      category: 'system',
      scopeLabel: '系统',
      icon: 'settings',
      action: () => {
        store.setSettings(true)
      },
    },
    {
      id: 'sys-plugins',
      name: 'plugins',
      command: '/plugins',
      description: '打开插件与技能库面板（管理扩展、技能与提示词）',
      category: 'system',
      scopeLabel: '系统',
      icon: 'plug',
      action: () => {
        store.setPlugins(true)
      },
    },
  ]
}

/** 将提示词条目映射为快捷指令 */
export function mapPromptToCommand(prompt: PromptItem): SlashCommandItem {
  const scopeMap: Record<string, string> = {
    builtin: '内置',
    plugin: '插件',
    workspace: '工作区',
    global: '全局',
  }
  return {
    id: `prompt-${prompt.id}`,
    name: prompt.name,
    command: `/${prompt.name}`,
    description: prompt.description,
    argumentHint: prompt.argumentHint,
    category: prompt.scope as any,
    scopeLabel: scopeMap[prompt.scope] ?? '提示词',
    icon: prompt.scope === 'builtin' ? 'zap' : 'thread',
  }
}

/** 过滤快捷指令辅助逻辑 */
export function filterCommands(
  commands: SlashCommandItem[],
  query: string,
  categoryFilter: string = 'all'
): SlashCommandItem[] {
  const cleanQuery = query.trim().replace(/^\//, '').toLowerCase()

  return commands.filter((cmd) => {
    if (categoryFilter !== 'all') {
      if (categoryFilter === 'system' && cmd.category !== 'system') return false
      if (categoryFilter === 'builtin' && cmd.category !== 'builtin') return false
      if (
        categoryFilter === 'custom' &&
        cmd.category !== 'plugin' &&
        cmd.category !== 'workspace' &&
        cmd.category !== 'global'
      )
        return false
    }

    if (!cleanQuery) return true

    const matchName = cmd.name.toLowerCase().includes(cleanQuery)
    const matchCmd = cmd.command.toLowerCase().includes(cleanQuery)
    const matchDesc = cmd.description.toLowerCase().includes(cleanQuery)
    const matchHint = cmd.argumentHint?.toLowerCase().includes(cleanQuery)

    return matchName || matchCmd || matchDesc || Boolean(matchHint)
  })
}

export function SlashCommandMenu({
  store,
  filterQuery = '',
  onSelect,
  onClose,
}: {
  store: AgentStore
  filterQuery?: string
  onSelect: (command: SlashCommandItem, isActionExecuted?: boolean) => void
  onClose: () => void
}) {
  const [prompts, setPrompts] = useState<PromptItem[]>([])
  const [activeTab, setActiveTab] = useState<'all' | 'system' | 'builtin' | 'custom'>('all')

  useEffect(() => {
    let unmounted = false
    void defaultPromptManager.scanPrompts(store.active.workspace).then((list) => {
      if (!unmounted) {
        setPrompts(list.filter((p) => p.enabled && !p.isSystem))
      }
    })
    return () => {
      unmounted = true
    }
  }, [store.active.workspace])

  const systemCommands = getSystemCommands(store)
  const promptCommands = prompts.map(mapPromptToCommand)
  const allCommands = [...systemCommands, ...promptCommands]

  const filtered = filterCommands(allCommands, filterQuery, activeTab)

  const handlePick = (cmd: SlashCommandItem) => {
    if (cmd.action) {
      cmd.action()
      onSelect(cmd, true)
    } else {
      onSelect(cmd, false)
    }
  }

  return (
    <div
      testId="slash-command-menu"
      style={{
        width: '100%',
        maxWidth: M.composerMax,
        marginBottom: 8,
        backgroundColor: C.raised,
        borderWidth: 1,
        borderColor: C.borderStrong,
        borderRadius: 12,
        boxShadow: {
          offsetX: 0,
          offsetY: 6,
          blurRadius: 18,
          spreadRadius: 0,
          color: 'rgba(0, 0, 0, 0.22)',
        },
        padding: 8,
        display: 'flex',
        flexDirection: 'column',
        gap: 6,
      }}
      onClick={(e: any) => e?.stopPropagation?.()}
    >
      {/* 头部标题与分类筛选栏 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'space-between',
          paddingLeft: 6,
          paddingRight: 6,
          paddingBottom: 4,
          borderBottomWidth: 1,
          borderColor: C.cardBorder,
        }}
      >
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <Icon name="terminal" size={12} color={C.link} />
          <text style={{ fontSize: 11.5, fontWeight: 600, color: C.text }}>快捷指令</text>
          <div
            style={{
              paddingLeft: 5,
              paddingRight: 5,
              height: 16,
              borderRadius: 4,
              backgroundColor: C.overlay,
              display: 'flex',
              alignItems: 'center',
            }}
          >
            <text style={{ fontSize: 10, color: C.tertiary }}>{`${filtered.length} 项`}</text>
          </div>
        </div>

        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 4 }}>
          {/* 分类切换药丸 */}
          {[
            { id: 'all', label: '全部' },
            { id: 'system', label: '系统' },
            { id: 'builtin', label: '内置' },
            { id: 'custom', label: '插件/自定义' },
          ].map((tab) => {
            const isSelected = activeTab === tab.id
            return (
              <div
                key={tab.id}
                role="button"
                testId={`slash-tab-${tab.id}`}
                onClick={() => setActiveTab(tab.id as any)}
                style={{
                  paddingLeft: 6,
                  paddingRight: 6,
                  height: 20,
                  borderRadius: 4,
                  cursor: 'pointer',
                  backgroundColor: isSelected ? C.chipHover : '#00000000',
                  hover: { backgroundColor: C.overlay },
                  display: 'flex',
                  alignItems: 'center',
                }}
              >
                <text
                  style={{
                    fontSize: 10.5,
                    color: isSelected ? C.text : C.tertiary,
                    fontWeight: isSelected ? 600 : 400,
                  }}
                >
                  {tab.label}
                </text>
              </div>
            )
          })}

          <div
            role="button"
            aria-label="关闭指令菜单"
            testId="slash-menu-close"
            onClick={onClose}
            style={{
              cursor: 'pointer',
              opacity: 0.6,
              hover: { opacity: 1 },
              paddingLeft: 4,
              paddingRight: 2,
              paddingTop: 2,
              paddingBottom: 2,
            }}
          >
            <Icon name="close" size={11} color={C.tertiary} />
          </div>
        </div>
      </div>

      {/* 指令列表 */}
      <div
        testId="slash-command-list"
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 2,
          maxHeight: 260,
          minHeight: 0,
          overflowY: 'scroll',
          paddingRight: 4,
        }}
      >
        {filtered.length > 0 ? (
          filtered.map((cmd) => (
            <div
              key={cmd.id}
              testId={`slash-item-${cmd.name}`}
              role="button"
              aria-label={cmd.command}
              onClick={() => handlePick(cmd)}
              style={{
                display: 'flex',
                flexDirection: 'row',
                alignItems: 'center',
                justifyContent: 'space-between',
                paddingLeft: 8,
                paddingRight: 8,
                paddingTop: 5,
                paddingBottom: 5,
                borderRadius: 6,
                cursor: 'pointer',
                flexShrink: 0,
                hover: { backgroundColor: C.chipHover },
              }}
            >
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 8,
                  flexGrow: 1,
                  minWidth: 0,
                }}
              >
                <div
                  style={{
                    width: 22,
                    height: 22,
                    borderRadius: 5,
                    backgroundColor: cmd.category === 'system' ? C.overlayStrong : C.chip,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    flexShrink: 0,
                  }}
                >
                  <Icon
                    name={cmd.icon}
                    size={11}
                    color={cmd.category === 'system' ? C.secondary : C.link}
                  />
                </div>

                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 1,
                    flexGrow: 1,
                    minWidth: 0,
                  }}
                >
                  <div
                    style={{
                      display: 'flex',
                      flexDirection: 'row',
                      alignItems: 'center',
                      gap: 6,
                    }}
                  >
                    <text style={{ fontSize: 12, fontWeight: 600, color: C.text }}>
                      {cmd.command}
                    </text>
                    {cmd.argumentHint ? (
                      <text
                        style={{
                          fontFamily: FONT_MONO,
                          fontSize: 10.5,
                          color: C.tertiary,
                          whiteSpace: 'nowrap',
                        }}
                      >
                        {cmd.argumentHint}
                      </text>
                    ) : null}
                  </div>
                  <text
                    style={{
                      fontSize: 10.5,
                      color: C.secondary,
                      whiteSpace: 'nowrap',
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                    }}
                  >
                    {cmd.description}
                  </text>
                </div>
              </div>

              {/* 作用域徽标 */}
              <div
                style={{
                  paddingLeft: 5,
                  paddingRight: 5,
                  height: 17,
                  borderRadius: 4,
                  backgroundColor:
                    cmd.category === 'builtin'
                      ? '#8b5cf618'
                      : cmd.category === 'system'
                      ? C.overlay
                      : cmd.category === 'workspace'
                      ? '#10b98118'
                      : C.overlayStrong,
                  borderWidth: cmd.category === 'builtin' || cmd.category === 'workspace' ? 1 : 0,
                  borderColor:
                    cmd.category === 'builtin'
                      ? '#8b5cf640'
                      : cmd.category === 'workspace'
                      ? '#10b98140'
                      : undefined,
                  display: 'flex',
                  alignItems: 'center',
                  flexShrink: 0,
                  marginLeft: 8,
                }}
              >
                <text
                  style={{
                    fontSize: 9.5,
                    color:
                      cmd.category === 'builtin'
                        ? '#8b5cf6'
                        : cmd.category === 'workspace'
                        ? '#10b981'
                        : C.tertiary,
                    whiteSpace: 'nowrap',
                  }}
                >
                  {cmd.scopeLabel}
                </text>
              </div>
            </div>
          ))
        ) : (
          <div
            style={{
              paddingTop: 16,
              paddingBottom: 16,
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              justifyContent: 'center',
              gap: 4,
            }}
          >
            <Icon name="search" size={14} color={C.faint} />
            <text style={{ fontSize: 11, color: C.faint }}>未找到匹配的快捷指令</text>
          </div>
        )}
      </div>
    </div>
  )
}
