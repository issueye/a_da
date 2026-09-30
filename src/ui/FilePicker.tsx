/**
 * 应用内的文件/目录选择器（取代原生选择窗口）。
 *
 * 两个模式共用一套浏览界面：
 * - `directory`：选一个目录（工作区）。确认按钮选的是**当前所在目录**。
 * - `files`：多选文件（附件、图片），可按扩展名过滤。
 *
 * ## 为什么长得像"文件管理器"而不是"对话框"
 *
 * 原生选择窗口只有本机能用，Web/H5 前端没有（协议 §12）。既然要自己做，就顺手做对几件
 * 原生窗口给不了的事：**路径可以直接输入**（知道路径的人不用一层层点）、
 * **截断与隐藏如实说出来**（"还有 N 项没显示"）、**错误就地显示**（路径不存在时告诉你是哪个）。
 *
 * ## 状态都在这里，数据都来自 `fs.*`
 *
 * 浏览状态（当前路径、选中项、错误）是**客户端本地**的；目录内容一律通过
 * `client.request('fs.list', …)` 问主机。所以这套界面在进程内与 WebSocket 下完全一致。
 */

import React, { useEffect, useState } from 'react'
import type { AgentClient, FsEntry, FsRoot } from './client'
import { C, FONT_MONO, M } from '../theme'
import { Icon, IconButton } from './controls'

export interface FilePickerProps {
  client: AgentClient
  mode: 'directory' | 'files'
  title?: string
  /** 打开时停在哪个目录（通常是当前工作区）；不传就问主机要根列表里的第一个 */
  startPath?: string
  /** 只允许选这些文件（正则，按文件名匹配）；`files` 模式下生效 */
  accept?: RegExp
  onPicked: (paths: string[]) => void
  onClose: () => void
}

export function FilePicker({
  client,
  mode,
  title,
  startPath,
  accept,
  onPicked,
  onClose,
}: FilePickerProps) {
  const [roots, setRoots] = useState<FsRoot[]>([])
  const [current, setCurrent] = useState(startPath ?? '')
  const [parent, setParent] = useState<string | null>(null)
  const [entries, setEntries] = useState<FsEntry[]>([])
  const [selected, setSelected] = useState<string[]>([])
  const [pathDraft, setPathDraft] = useState(startPath ?? '')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [truncated, setTruncated] = useState<{ omitted: number } | null>(null)
  const [hiddenCount, setHiddenCount] = useState(0)
  const [showHidden, setShowHidden] = useState(false)
  const [creating, setCreating] = useState(false)
  const [newFolderName, setNewFolderName] = useState('')

  /** 列一个目录；失败就把错误显示出来（不静默、也不假装空目录）。 */
  const browse = async (target: string, options: { showHidden?: boolean } = {}) => {
    setLoading(true)
    setError(null)
    try {
      const listing = await client.request('fs.list', {
        path: target,
        showHidden: options.showHidden ?? showHidden,
      })
      setCurrent(listing.path)
      setPathDraft(listing.path)
      setParent(listing.parent)
      setEntries(listing.entries)
      setTruncated(listing.truncated ? { omitted: listing.omitted } : null)
      setHiddenCount(listing.hiddenCount)
      if (mode === 'files') setSelected([])
    } catch (err) {
      setError((err as Error).message)
    } finally {
      setLoading(false)
    }
  }

  // 首帧：先拿根（拿它当兜底的起始目录），再列起始目录
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const rootList = await client.request('fs.roots', {})
        if (cancelled) return
        setRoots(rootList)
        const fallback =
          startPath ||
          rootList.find((root) => root.kind === 'workspace')?.path ||
          rootList.find((root) => root.kind === 'home')?.path ||
          rootList[0]?.path
        if (fallback) await browse(fallback)
        else setError('主机没有可浏览的根目录')
      } catch (err) {
        if (!cancelled) setError((err as Error).message)
      }
    })()
    return () => {
      cancelled = true
    }
    // 只在挂载时跑一次：之后的浏览都由用户动作驱动
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const visibleEntries = mode === 'files' && accept ? entries.filter((e) => e.kind === 'dir' || accept.test(e.name)) : entries
  const filteredOut = entries.length - visibleEntries.length

  const confirmLabel =
    mode === 'directory'
      ? '选择此目录'
      : selected.length > 0
        ? `添加 ${selected.length} 个文件`
        : '添加文件'

  const handleConfirm = () => {
    if (mode === 'directory') {
      if (current) onPicked([current])
      return
    }
    if (selected.length > 0) onPicked(selected)
  }

  const handleCreateFolder = async () => {
    const name = newFolderName.trim()
    if (!name) {
      setError('文件夹名不能为空')
      return
    }
    const target = current ? `${current}${current.endsWith('/') || current.endsWith('\\') ? '' : sep()}${name}` : name
    try {
      const created = await client.request('fs.mkdir', { path: target })
      setCreating(false)
      setNewFolderName('')
      await browse(created.path)
    } catch (err) {
      setError((err as Error).message)
    }
  }

  const toggleSelect = (entry: FsEntry) => {
    setSelected((prev) =>
      prev.includes(entry.path) ? prev.filter((p) => p !== entry.path) : [...prev, entry.path]
    )
  }

  return (
    <div
      testId="file-picker"
      style={{
        position: 'absolute',
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        backgroundColor: 'rgba(0,0,0,0.45)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
      }}
    >
      <div
        style={{
          width: 720,
          height: 460,
          display: 'flex',
          flexDirection: 'column',
          backgroundColor: C.canvas,
          borderWidth: 1,
          borderColor: C.borderStrong,
          borderRadius: 10,
        }}
      >
        {/* 标题栏 */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            justifyContent: 'space-between',
            height: 38,
            paddingLeft: M.contentPadding,
            paddingRight: 8,
            borderBottomWidth: 1,
            borderColor: C.border,
          }}
        >
          <text style={{ fontSize: 13, fontWeight: 600, color: C.text }}>
            {title ?? (mode === 'directory' ? '选择工作区目录' : '选择文件')}
          </text>
          <IconButton testId="file-picker-close" label="关闭" icon="x" size={13} color={C.secondary} onClick={onClose} />
        </div>

        {/* 路径输入 + 上一级 + 新建文件夹 */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            gap: 6,
            height: 36,
            paddingLeft: M.contentPadding,
            paddingRight: M.contentPadding,
            borderBottomWidth: 1,
            borderColor: C.border,
          }}
        >
          <div
            testId="file-picker-up"
            role="button"
            aria-label="上一级"
            onClick={() => parent && void browse(parent)}
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              width: 24,
              height: 22,
              borderRadius: 6,
              cursor: parent ? 'pointer' : 'default',
              opacity: parent ? 1 : 0.4,
              hover: { backgroundColor: C.chip },
            }}
          >
            <Icon name="arrowUp" size={12} color={C.secondary} />
          </div>
          <input
            testId="file-picker-path"
            value={pathDraft}
            onChange={(event) => setPathDraft(event.value ?? '')}
            onSubmit={() => void browse(pathDraft)}
            placeholder="直接输入路径后回车"
            style={{
              flexGrow: 1,
              height: 24,
              paddingLeft: 8,
              paddingRight: 8,
              fontSize: 11.5,
              fontFamily: FONT_MONO,
              color: C.text,
              backgroundColor: C.raised,
              borderWidth: 1,
              borderColor: C.border,
              borderRadius: 6,
            }}
          />
          {/* 除了回车，也给它一个可点的"前往"：不是所有人都习惯键盘，而且按钮更好测 */}
          <div
            testId="file-picker-go"
            role="button"
            aria-label="前往"
            onClick={() => void browse(pathDraft)}
            style={{
              display: 'flex',
              alignItems: 'center',
              height: 22,
              paddingLeft: 8,
              paddingRight: 8,
              borderRadius: 6,
              cursor: 'pointer',
              backgroundColor: C.raised,
              hover: { backgroundColor: C.chip },
            }}
          >
            <text style={{ fontSize: 11, color: C.secondary }}>前往</text>
          </div>
          {mode === 'directory' ? (
            <div
              testId="file-picker-mkdir"
              role="button"
              aria-label="新建文件夹"
              onClick={() => {
                setCreating((v) => !v)
                setNewFolderName('')
              }}
              style={{
                display: 'flex',
                alignItems: 'center',
                height: 22,
                paddingLeft: 8,
                paddingRight: 8,
                borderRadius: 6,
                cursor: 'pointer',
                backgroundColor: creating ? C.chip : C.raised,
                hover: { backgroundColor: C.chip },
              }}
            >
              <text style={{ fontSize: 11, color: C.secondary }}>新建文件夹</text>
            </div>
          ) : null}
        </div>

        {creating ? (
          <div
            style={{
              display: 'flex',
              flexDirection: 'row',
              alignItems: 'center',
              gap: 6,
              height: 34,
              paddingLeft: M.contentPadding,
              paddingRight: M.contentPadding,
              backgroundColor: C.raised,
            }}
          >
            <input
              testId="file-picker-new-folder"
              value={newFolderName}
              onChange={(event) => setNewFolderName(event.value ?? '')}
              onSubmit={() => void handleCreateFolder()}
              placeholder="新文件夹名字"
              style={{
                flexGrow: 1,
                height: 22,
                paddingLeft: 8,
                fontSize: 11.5,
                color: C.text,
                backgroundColor: C.canvas,
                borderWidth: 1,
                borderColor: C.borderStrong,
                borderRadius: 6,
              }}
            />
            <div
              testId="file-picker-new-folder-ok"
              role="button"
              aria-label="创建"
              onClick={() => void handleCreateFolder()}
              style={{
                display: 'flex',
                alignItems: 'center',
                height: 22,
                paddingLeft: 10,
                paddingRight: 10,
                borderRadius: 6,
                cursor: 'pointer',
                backgroundColor: C.chip,
              }}
            >
              <text style={{ fontSize: 11, color: C.text }}>创建</text>
            </div>
          </div>
        ) : null}

        <div style={{ display: 'flex', flexDirection: 'row', flexGrow: 1, minHeight: 0 }}>
          {/* 根快捷 */}
          <div
            style={{
              width: 150,
              display: 'flex',
              flexDirection: 'column',
              paddingTop: 6,
              paddingBottom: 6,
              gap: 2,
              borderRightWidth: 1,
              borderColor: C.border,
            }}
          >
            {roots.map((root, index) => (
              <div
                key={root.path}
                testId={`file-picker-root-${index}`}
                role="button"
                aria-label={root.label}
                onClick={() => void browse(root.path)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  height: 24,
                  paddingLeft: M.contentPadding,
                  paddingRight: 6,
                  cursor: 'pointer',
                  backgroundColor: current === root.path ? C.chip : '#00000000',
                  hover: { backgroundColor: C.chip },
                }}
              >
                <text
                  style={{
                    fontSize: 11.5,
                    color: current === root.path ? C.text : C.secondary,
                  }}
                >
                  {root.kind === 'drive' ? `💽 ${root.label}` : root.kind === 'home' ? `🏠 ${root.label}` : `📁 ${root.label}`}
                </text>
              </div>
            ))}
          </div>

          {/* 条目列表 */}
          <div style={{ flexGrow: 1, display: 'flex', flexDirection: 'column', minWidth: 0 }}>
            <div style={{ flexGrow: 1, overflow: 'scroll', paddingTop: 6, paddingBottom: 6 }}>
              {loading ? (
                <text style={{ fontSize: 11.5, color: C.tertiary, paddingLeft: M.contentPadding }}>
                  读取中…
                </text>
              ) : error ? (
                <text
                  testId="file-picker-error"
                  style={{ fontSize: 11.5, color: C.accent, paddingLeft: M.contentPadding }}
                >
                  {error}
                </text>
              ) : visibleEntries.length === 0 ? (
                <text style={{ fontSize: 11.5, color: C.tertiary, paddingLeft: M.contentPadding }}>
                  {mode === 'files' && filteredOut > 0 ? '这里没有符合类型要求的文件' : '这个目录是空的'}
                </text>
              ) : (
                visibleEntries.map((entry) => {
                  const isSelected = selected.includes(entry.path)
                  return (
                    <div
                      key={entry.path}
                      testId={`file-picker-entry-${entry.name}`}
                      role="button"
                      aria-label={entry.name}
                      onClick={() => {
                        if (entry.kind === 'dir') {
                          void browse(entry.path)
                          return
                        }
                        if (mode === 'files') toggleSelect(entry)
                      }}
                      style={{
                        display: 'flex',
                        flexDirection: 'row',
                        alignItems: 'center',
                        gap: 7,
                        height: 24,
                        paddingLeft: M.contentPadding,
                        paddingRight: M.contentPadding,
                        cursor: entry.kind === 'dir' || mode === 'files' ? 'pointer' : 'default',
                        backgroundColor: isSelected ? C.chip : '#00000000',
                        hover: { backgroundColor: C.chip },
                      }}
                    >
                      <text style={{ fontSize: 12 }}>
                        {entry.kind === 'dir' ? '📁' : '📄'}
                      </text>
                      <text
                        style={{
                          fontSize: 11.5,
                          color: entry.kind === 'dir' ? C.text : C.secondary,
                          flexGrow: 1,
                        }}
                      >
                        {entry.name}
                      </text>
                      {entry.kind === 'file' && typeof entry.sizeBytes === 'number' ? (
                        <text style={{ fontSize: 10.5, color: C.tertiary }}>
                          {formatSize(entry.sizeBytes)}
                        </text>
                      ) : null}
                    </div>
                  )
                })
              )}
            </div>

            {/* 如实说明：被隐藏的、被截断的、以及"有没有被过滤掉" */}
            {(hiddenCount > 0 || truncated || filteredOut > 0) && !error ? (
              <div
                testId="file-picker-notice"
                style={{
                  display: 'flex',
                  flexDirection: 'row',
                  alignItems: 'center',
                  gap: 10,
                  height: 24,
                  paddingLeft: M.contentPadding,
                  paddingRight: M.contentPadding,
                  borderTopWidth: 1,
                  borderColor: C.border,
                }}
              >
                {hiddenCount > 0 ? (
                  <text
                    testId="file-picker-show-hidden"
                    role="button"
                    aria-label="显示隐藏项"
                    onClick={() => {
                      const next = !showHidden
                      setShowHidden(next)
                      void browse(current, { showHidden: next })
                    }}
                    style={{ fontSize: 10.5, color: C.link, cursor: 'pointer' }}
                  >
                    {`还有 ${hiddenCount} 个隐藏项 · 显示`}
                  </text>
                ) : null}
                {truncated ? (
                  <text style={{ fontSize: 10.5, color: C.tertiary }}>
                    {`条目太多，还有 ${truncated.omitted} 项没显示`}
                  </text>
                ) : null}
                {filteredOut > 0 ? (
                  <text style={{ fontSize: 10.5, color: C.tertiary }}>
                    {`已按类型过滤掉 ${filteredOut} 项`}
                  </text>
                ) : null}
              </div>
            ) : null}
          </div>
        </div>

        {/* 底部动作 */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'row',
            alignItems: 'center',
            justifyContent: 'space-between',
            height: 44,
            paddingLeft: M.contentPadding,
            paddingRight: M.contentPadding,
            borderTopWidth: 1,
            borderColor: C.border,
          }}
        >
          <text
            testId="file-picker-current"
            style={{ fontSize: 11, color: C.tertiary, flexShrink: 1 }}
          >
            {current || '（还没有选目录）'}
          </text>
          <div style={{ display: 'flex', flexDirection: 'row', alignItems: 'center', gap: 8 }}>
            <div
              testId="file-picker-cancel"
              role="button"
              aria-label="取消"
              onClick={onClose}
              style={{
                display: 'flex',
                alignItems: 'center',
                height: 26,
                paddingLeft: 12,
                paddingRight: 12,
                borderRadius: 7,
                cursor: 'pointer',
                borderWidth: 1,
                borderColor: C.border,
                hover: { backgroundColor: C.chip },
              }}
            >
              <text style={{ fontSize: 11.5, color: C.secondary }}>取消</text>
            </div>
            <div
              testId="file-picker-confirm"
              role="button"
              aria-label={confirmLabel}
              onClick={handleConfirm}
              style={{
                display: 'flex',
                alignItems: 'center',
                height: 26,
                paddingLeft: 12,
                paddingRight: 12,
                borderRadius: 7,
                cursor: 'pointer',
                backgroundColor: C.accent,
                opacity: mode === 'files' && selected.length === 0 ? 0.5 : 1,
                hover: { opacity: 0.85 },
              }}
            >
              <text style={{ fontSize: 11.5, fontWeight: 600, color: C.canvas }}>{confirmLabel}</text>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

/** 路径分隔符：跟着当前目录走（Windows 上是 `\`）。 */
function sep(): string {
  return process.platform === 'win32' ? '\\' : '/'
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`
}
