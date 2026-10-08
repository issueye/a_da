/**
 * @ 提及（Mention）面板组件
 * 位于输入框顶部浮动层，聚合工作区文件、扩展技能规范与子智能体
 */

import React, { useEffect, useMemo, useState } from 'react'
import type { AgentClient, SkillSummary, SubagentProfile } from './client'
import { C, FONT_MONO, M } from '../theme'
import { Icon } from './controls'
import type { IconName } from '../icons'

export interface MentionItem {
  id: string
  title: string
  subtitle?: string
  category: 'file' | 'skill' | 'subagent'
  categoryLabel: string
  icon: IconName
  insertText: string
}

export interface MentionMenuProps {
  client: AgentClient
  filterQuery: string
  onSelect: (item: MentionItem) => void
  onClose: () => void
}

/** 智能推断文件类型对应的图标 */
export function getFileIcon(path: string): IconName {
  const lower = path.toLowerCase()
  if (/\.(ts|tsx|js|jsx|rs|py|go|c|cpp|h|java|json|toml|yaml|yml)$/.test(lower)) {
    return 'code'
  }
  if (/\.(png|jpe?g|gif|svg|webp|ico)$/.test(lower)) {
    return 'sparkles'
  }
  if (/\.(md|txt|doc|pdf)$/.test(lower)) {
    return 'file'
  }
  return 'file'
}

/** 按分类与关键词过滤提及项（纯函数） */
export function filterMentions(
  items: MentionItem[],
  query: string,
  category: 'all' | 'file' | 'skill' | 'subagent' = 'all'
): MentionItem[] {
  let result = items
  if (category !== 'all') {
    result = result.filter((item) => item.category === category)
  }
  const q = query.trim().toLowerCase()
  if (q) {
    result = result.filter(
      (item) =>
        item.title.toLowerCase().includes(q) ||
        item.insertText.toLowerCase().includes(q) ||
        (item.subtitle && item.subtitle.toLowerCase().includes(q))
    )
  }
  return result.slice(0, 30)
}

export function MentionMenu({ client, filterQuery, onSelect, onClose }: MentionMenuProps) {
  const [activeTab, setActiveTab] = useState<'all' | 'file' | 'skill' | 'subagent'>('all')
  const [skills, setSkills] = useState<SkillSummary[]>([])
  const [subagents, setSubagents] = useState<SubagentProfile[]>([])
  const [selectedIndex, setSelectedIndex] = useState(0)

  // 异步拉取技能与子智能体列表
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const [skList, subList] = await Promise.all([
          client.request('skill.list', { workspace: client.state.project }).catch(() => [] as SkillSummary[]),
          client.request('subagentProfile.list', { workspace: client.state.project }).catch(() => [] as SubagentProfile[]),
        ])
        if (!cancelled) {
          setSkills(skList.filter((s) => s.enabled))
          setSubagents(subList.filter((s) => s.enabled))
        }
      } catch {}
    })()
    return () => {
      cancelled = true
    }
  }, [client, client.state.project])

  // 聚合所有候选条目
  const allItems = useMemo<MentionItem[]>(() => {
    const list: MentionItem[] = []

    // 1. 工作区文件
    const entries = client.state.entries || []
    for (const file of entries) {
      const parts = file.split(/[\\/]/)
      const fileName = parts[parts.length - 1] || file
      const dirPath = parts.length > 1 ? parts.slice(0, -1).join('/') : ''
      list.push({
        id: `file:${file}`,
        title: fileName,
        subtitle: dirPath ? dirPath : undefined,
        category: 'file',
        categoryLabel: '文件',
        icon: getFileIcon(file),
        insertText: file,
      })
    }

    // 2. 技能库
    for (const skill of skills) {
      list.push({
        id: `skill:${skill.name}`,
        title: skill.name,
        subtitle: skill.description || '扩展技能规范',
        category: 'skill',
        categoryLabel: '技能',
        icon: 'sparkles',
        insertText: `skill:${skill.name}`,
      })
    }

    // 3. 子智能体
    for (const sub of subagents) {
      list.push({
        id: `subagent:${sub.id}`,
        title: sub.name,
        subtitle: sub.description || '特化子智能体',
        category: 'subagent',
        categoryLabel: '智能体',
        icon: 'compass',
        insertText: `subagent:${sub.name}`,
      })
    }

    return list
  }, [client.state.entries, skills, subagents])

  // 按分类与关键词过滤
  const filtered = useMemo(() => {
    return filterMentions(allItems, filterQuery, activeTab)
  }, [allItems, activeTab, filterQuery])

  // 过滤变动时自动重置高亮选中位置
  useEffect(() => {
    setSelectedIndex(0)
  }, [filtered.length, activeTab, filterQuery])

  const handlePick = (item: MentionItem) => {
    onSelect(item)
  }

  return (
    <div
      testId="mention-menu"
      style={{
        display: 'flex',
        flexDirection: 'column',
        width: '100%',
        maxWidth: M.composerMax,
        backgroundColor: C.card,
        borderWidth: 1,
        borderColor: C.borderStrong,
        borderRadius: 12,
        paddingTop: 8,
        paddingBottom: 8,
        paddingLeft: 8,
        paddingRight: 8,
        marginBottom: 8,
        boxShadow: {
          offsetX: 0,
          offsetY: 6,
          blurRadius: 18,
          spreadRadius: 0,
          color: C.shadow,
        },
        gap: 6,
      }}
    >
      {/* 头部导航与分类切换 */}
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
          <text style={{ fontSize: 13, fontWeight: 700, color: C.link }}>@</text>
          <text style={{ fontSize: 11.5, fontWeight: 600, color: C.text }}>提及引用</text>
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
            { id: 'file', label: '文件' },
            { id: 'skill', label: '技能' },
            { id: 'subagent', label: '智能体' },
          ].map((tab) => {
            const isSelected = activeTab === tab.id
            return (
              <div
                key={tab.id}
                role="button"
                testId={`mention-tab-${tab.id}`}
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
            aria-label="关闭提及菜单"
            testId="mention-menu-close"
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

      {/* 提及条目列表 */}
      <div
        testId="mention-item-list"
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 2,
          maxHeight: 240,
          minHeight: 0,
          overflowY: 'scroll',
          paddingRight: 4,
        }}
      >
        {filtered.length > 0 ? (
          filtered.map((item, idx) => {
            const isSelected = idx === selectedIndex
            return (
              <div
                key={item.id}
                testId={`mention-item-${item.category}-${item.title}`}
                role="button"
                aria-label={item.insertText}
                onClick={() => handlePick(item)}
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
                  backgroundColor: isSelected ? C.overlayStrong : '#00000000',
                  hover: { backgroundColor: C.chipHover },
                }}
              >
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    gap: 8,
                    minWidth: 0,
                    flexGrow: 1,
                    flexShrink: 1,
                  }}
                >
                  <Icon
                    name={item.icon}
                    size={13}
                    color={
                      item.category === 'file'
                        ? C.link
                        : item.category === 'skill'
                        ? C.accent
                        : C.secondary
                    }
                  />
                  <div
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      minWidth: 0,
                      flexGrow: 1,
                      flexShrink: 1,
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
                      <text
                        style={{
                          fontSize: 12,
                          fontWeight: 500,
                          color: C.text,
                          fontFamily: item.category === 'file' ? FONT_MONO : undefined,
                        }}
                      >
                        {item.title}
                      </text>
                      {item.subtitle ? (
                        <text
                          style={{
                            fontSize: 10.5,
                            color: C.tertiary,
                            fontFamily: item.category === 'file' ? FONT_MONO : undefined,
                          }}
                        >
                          {item.subtitle}
                        </text>
                      ) : null}
                    </div>
                  </div>
                </div>

                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'row',
                    alignItems: 'center',
                    gap: 6,
                    flexShrink: 0,
                  }}
                >
                  <div
                    style={{
                      paddingLeft: 5,
                      paddingRight: 5,
                      paddingTop: 1,
                      paddingBottom: 1,
                      borderRadius: 3,
                      backgroundColor: C.overlay,
                    }}
                  >
                    <text style={{ fontSize: 9.5, color: C.secondary }}>
                      {item.categoryLabel}
                    </text>
                  </div>
                </div>
              </div>
            )
          })
        ) : (
          <div
            style={{
              paddingTop: 16,
              paddingBottom: 16,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
          >
            <text style={{ fontSize: 11.5, color: C.tertiary }}>
              {filterQuery ? `未找到与 "${filterQuery}" 匹配的内容` : '暂无可提及的条目'}
            </text>
          </div>
        )}
      </div>

      {/* 底部交互指引 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'space-between',
          paddingTop: 4,
          paddingLeft: 6,
          paddingRight: 6,
          borderTopWidth: 1,
          borderColor: C.cardBorder,
        }}
      >
        <text style={{ fontSize: 10, color: C.tertiary }}>
          点击或按回车将引用插入到当前光标处
        </text>
        <div style={{ display: 'flex', flexDirection: 'row', gap: 6 }}>
          <text style={{ fontSize: 9.5, color: C.tertiary }}>[Esc] 关闭</text>
        </div>
      </div>
    </div>
  )
}
