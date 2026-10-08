/**
 * 改动审阅面板 (ChangesPanel)
 *
 * 悬浮在会话区右上角（任务规划面板下方），把本会话所有被跟踪的文件改动
 * （write_file / edit_file）按文件聚合：看 diff、逐文件「恢复原状」、或一键
 * 全部恢复。数据来自客户端（`change.list` 的派生），回滚能力来自检查点模块
 * （agent/checkpoint.ts）——批准即落盘，但随时有得退。
 */

import React, { useState } from 'react'
import type { AgentClient, FileChange } from './client'
import { C, FONT_MONO } from '../theme'
import { docTheme } from '../theme'
import { Icon } from './controls'

function baseName(path: string): string {
  const normalized = path.replace(/\\/g, '/')
  const idx = normalized.lastIndexOf('/')
  return idx >= 0 ? normalized.slice(idx + 1) : normalized
}

export function ChangesPanel({ client }: { client: AgentClient }) {
  const [expandedPath, setExpandedPath] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const threadId = client.state.activeId
  const changes: FileChange[] = client.state.getThreadFileChanges(threadId)
  const activeCount = changes.filter((change) => !change.reverted).length
  const running = client.state.isThreadRunning(threadId)

  const revertFile = async (path: string): Promise<void> => {
    if (busy) return
    setBusy(true)
    try {
      const { ok } = await client.request('change.revertFile', { threadId, path })
      if (ok) {
        client.ui.notify({ message: `已恢复 ${baseName(path)}`, detail: path })
      } else {
        // 命令回了 ok:false（不是抛错）：一样是失败，不能静默
        client.ui.notify({ level: 'error', message: `恢复 ${baseName(path)} 失败`, detail: path })
      }
    } catch (err) {
      // 以前这里只有 finally 清 busy：**失败被整个吞掉**，用户以为恢复了
      client.ui.notify({
        level: 'error',
        message: `恢复 ${baseName(path)} 失败：${(err as Error).message}`,
      })
    } finally {
      setBusy(false)
    }
  }

  const revertAll = async (): Promise<void> => {
    if (busy) return
    setBusy(true)
    const count = activeCount
    try {
      const { ok } = await client.request('change.revertAll', { threadId })
      if (ok) {
        client.ui.notify({ message: `已恢复全部 ${count} 个文件改动` })
      } else {
        client.ui.notify({ level: 'error', message: '全部恢复失败', detail: '主机没有完成回滚' })
      }
    } catch (err) {
      client.ui.notify({ level: 'error', message: `全部恢复失败：${(err as Error).message}` })
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      testId="changes-panel"
      style={{
        position: 'absolute',
        top: 56,
        right: 28,
        width: 420,
        maxWidth: 480,
        display: 'flex',
        flexDirection: 'column',
        backgroundColor: C.raised,
        borderWidth: 1,
        borderColor: C.borderStrong,
        borderRadius: 12,
        padding: 12,
        boxShadow: { offsetX: 0, offsetY: 4, blurRadius: 18, spreadRadius: 0, color: C.shadow },
      }}
    >
      {/* 标题行 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          justifyContent: 'space-between',
          paddingBottom: 8,
          borderBottomWidth: 1,
          borderColor: C.cardBorder,
        }}
      >
        <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 7 }}>
          <Icon name="edit" size={14} color={activeCount > 0 ? C.link : C.tertiary} />
          <text style={{ fontSize: 12.5, fontWeight: 600, color: C.text }}>文件改动</text>
          <div
            style={{
              paddingLeft: 6,
              paddingRight: 6,
              height: 18,
              borderRadius: 9,
              backgroundColor: C.overlay,
              display: 'flex',
              alignItems: 'center',
            }}
          >
            <text style={{ fontSize: 10.5, fontFamily: FONT_MONO, color: C.tertiary }}>
              {`${activeCount} 个文件待保留`}
            </text>
          </div>
        </div>
        <div
          testId="close-changes-panel"
          role="button"
          aria-label="关闭改动审阅"
          onClick={() => client.ui.setChangesOpen(false)}
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            width: 22,
            height: 22,
            borderRadius: 4,
            cursor: 'pointer',
            backgroundColor: C.overlay,
            hover: { backgroundColor: C.chipHover },
          }}
        >
          <Icon name="close" size={11} color={C.tertiary} />
        </div>
      </div>

      {/* 文件列表 */}
      <div
        testId="changes-file-list"
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: 4,
          marginTop: 8,
          maxHeight: 360,
          overflowY: 'scroll',
          paddingRight: 4,
        }}
      >
        {changes.length === 0 ? (
          <text style={{ fontSize: 12, color: C.faint, padding: 6 }}>
            本会话还没有被跟踪的文件改动。
          </text>
        ) : null}
        {changes.map((change) => {
          const expanded = expandedPath === change.path
          return (
            <div
              key={change.path}
              testId={`change-row-${change.path}`}
              style={{
                display: 'flex',
                flexDirection: 'column',
                borderRadius: 6,
                backgroundColor: change.reverted ? undefined : C.overlay,
                opacity: change.reverted ? 0.55 : 1,
              }}
            >
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 7,
                  paddingTop: 6,
                  paddingBottom: 6,
                  paddingLeft: 6,
                  paddingRight: 6,
                }}
              >
                <Icon
                  name={change.reverted ? 'circleCheck' : 'edit'}
                  size={12}
                  color={change.reverted ? C.success : C.tertiary}
                />
                <div style={{ display: 'flex', flexDirection: 'column', flexShrink: 1, minWidth: 0 }}>
                  <text
                    style={{
                      fontSize: 12,
                      lineHeight: 16,
                      fontFamily: FONT_MONO,
                      color: change.reverted ? C.faint : C.text,
                      whiteSpace: 'nowrap',
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                    }}
                  >
                    {baseName(change.path)}
                  </text>
                  <text
                    style={{
                      fontSize: 10.5,
                      lineHeight: 14,
                      fontFamily: FONT_MONO,
                      color: C.faint,
                      whiteSpace: 'nowrap',
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                    }}
                  >
                    {change.path}
                  </text>
                </div>
                <div style={{ flexGrow: 1 }} />
                {change.additions > 0 ? (
                  <text style={{ fontSize: 11, fontFamily: FONT_MONO, color: C.success, flexShrink: 0 }}>
                    {`+${change.additions}`}
                  </text>
                ) : null}
                {change.deletions > 0 ? (
                  <text style={{ fontSize: 11, fontFamily: FONT_MONO, color: C.danger, flexShrink: 0 }}>
                    {`−${change.deletions}`}
                  </text>
                ) : null}
                {change.reverted ? (
                  <text style={{ fontSize: 11, color: C.success, flexShrink: 0 }}>已撤销</text>
                ) : (
                  <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 4, flexShrink: 0 }}>
                    {change.latestPatch ? (
                      <div
                        testId={`change-diff-${change.path}`}
                        role="button"
                        aria-label={`查看 ${change.path} 的改动内容`}
                        onClick={() => setExpandedPath(expanded ? null : change.path)}
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          height: 22,
                          paddingLeft: 8,
                          paddingRight: 8,
                          borderRadius: 5,
                          cursor: 'pointer',
                          backgroundColor: C.raised,
                          borderWidth: 1,
                          borderColor: C.borderStrong,
                          hover: { backgroundColor: C.chip },
                        }}
                      >
                        <text style={{ fontSize: 11, color: C.secondary }}>Diff</text>
                      </div>
                    ) : null}
                    <div
                      testId={`change-revert-${change.path}`}
                      role="button"
                      aria-label={`恢复 ${change.path} 到改动前`}
                      onClick={() => void revertFile(change.path)}
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        height: 22,
                        paddingLeft: 8,
                        paddingRight: 8,
                        borderRadius: 5,
                        cursor: running ? 'default' : 'pointer',
                        backgroundColor: C.raised,
                        borderWidth: 1,
                        borderColor: C.borderStrong,
                        opacity: running ? 0.5 : 1,
                        hover: running ? undefined : { opacity: 0.85 },
                      }}
                    >
                      <text style={{ fontSize: 11, color: C.danger }}>恢复原状</text>
                    </div>
                  </div>
                )}
              </div>
              {expanded && change.latestPatch ? (
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    borderWidth: 1,
                    borderColor: C.cardBorder,
                    borderRadius: 8,
                    overflow: 'hidden',
                    marginBottom: 6,
                    marginLeft: 6,
                    marginRight: 6,
                  }}
                >
                  <diff patch={change.latestPatch} wordDiff maxLines={18} theme={docTheme()} />
                </div>
              ) : null}
            </div>
          )
        })}
      </div>

      {/* 底部：一键恢复与说明 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'row',
          alignItems: 'center',
          gap: 8,
          marginTop: 8,
          paddingTop: 8,
          borderTopWidth: 1,
          borderColor: C.cardBorder,
        }}
      >
        <div
          testId="revert-all-changes"
          role="button"
          aria-label="恢复本会话的全部文件改动"
          onClick={() => void revertAll()}
          style={{
            display: 'flex',
            alignItems: 'center',
            height: 24,
            paddingLeft: 10,
            paddingRight: 10,
            borderRadius: 6,
            cursor: activeCount === 0 || running ? 'default' : 'pointer',
            backgroundColor: C.raised,
            borderWidth: 1,
            borderColor: C.borderStrong,
            opacity: activeCount === 0 || running ? 0.5 : 1,
            hover: activeCount === 0 || running ? undefined : { opacity: 0.85 },
            flexShrink: 0,
          }}
        >
          <text style={{ fontSize: 12, color: C.danger }}>全部恢复原状</text>
        </div>
        <div style={{ flexGrow: 1 }} />
        <text style={{ fontSize: 10.5, color: C.faint, flexShrink: 1 }} >
          仅跟踪文件的写入与编辑；命令造成的改动不在其中
        </text>
      </div>
    </div>
  )
}
