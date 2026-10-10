import React, { useState, useEffect, useRef } from 'react'
import {
  Folder,
  FileText,
  ChevronRight,
  ArrowUp,
  Plus,
  X,
  HardDrive,
  Home,
  Check,
  Search,
  Code2,
  Image as ImageIcon,
} from 'lucide-react'
import type { FsRoot, FsEntry, FsListing } from '../types'
import { agentClient } from '../client/ws-client'

export interface FilePickerProps {
  isOpen: boolean
  mode: 'directory' | 'files'
  title?: string
  startPath?: string
  filterExts?: string[]
  onPicked: (paths: string[]) => void
  onClose: () => void
}

export const FilePicker: React.FC<FilePickerProps> = ({
  isOpen,
  mode,
  title,
  startPath,
  filterExts,
  onPicked,
  onClose,
}) => {
  const [roots, setRoots] = useState<FsRoot[]>([])
  const [currentPath, setCurrentPath] = useState<string>(startPath || '')
  const [parentPath, setParentPath] = useState<string | null>(null)
  const [dirs, setDirs] = useState<FsEntry[]>([])
  const [files, setFiles] = useState<FsEntry[]>([])
  const [selectedPaths, setSelectedPaths] = useState<string[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [creatingFolder, setCreatingFolder] = useState(false)
  const [newFolderName, setNewFolderName] = useState('')
  const [pathInput, setPathInput] = useState('')

  // 初始化根驱动器与初始路径
  useEffect(() => {
    if (!isOpen) return

    void (async () => {
      try {
        const rootList = await agentClient.fetchRoots()
        if (rootList && rootList.length > 0) {
          setRoots(rootList)
          const target = startPath || rootList[0].path
          loadDir(target)
        }
      } catch (err: any) {
        setError(`读取根目录列表失败: ${err.message}`)
      }
    })()
  }, [isOpen, startPath])

  const loadDir = async (path: string) => {
    if (!path) return
    setLoading(true)
    setError(null)
    setCreatingFolder(false)
    setNewFolderName('')
    try {
      const listing = await agentClient.listDirectory(path, false, undefined, mode === 'directory')
      if (listing) {
        setCurrentPath(listing.path)
        setPathInput(listing.path)
        setParentPath(listing.parent || null)

        // 智能兼容后端 entries 扁平数组或 dirs / files 分离字段，并兼顾 isDir / is_dir
        const rawEntries: FsEntry[] = Array.isArray(listing.entries)
          ? listing.entries
          : [...(listing.dirs || []), ...(listing.files || [])]

        const dirList = (listing.dirs && listing.dirs.length > 0)
          ? listing.dirs
          : rawEntries.filter((e) => Boolean(e.isDir ?? e.is_dir))

        const fileList = (listing.files && listing.files.length > 0)
          ? listing.files
          : rawEntries.filter((e) => !Boolean(e.isDir ?? e.is_dir))

        setDirs(dirList)
        setFiles(fileList)
        setSelectedPaths([])
      }
    } catch (err: any) {
      setError(`无法打开路径 ${path}: ${err.message}`)
    } finally {
      setLoading(false)
    }
  }

  if (!isOpen) return null

  const handleCreateFolder = async () => {
    if (!newFolderName.trim() || !currentPath) return
    const target = `${currentPath.replace(/[\\/]+$/, '')}/${newFolderName.trim()}`
    try {
      await agentClient.makeDirectory(target)
      setCreatingFolder(false)
      setNewFolderName('')
      loadDir(target)
    } catch (err: any) {
      setError(`新建文件夹失败: ${err.message}`)
    }
  }

  const handleConfirm = () => {
    if (mode === 'directory') {
      if (selectedPaths.length > 0) {
        onPicked(selectedPaths)
      } else if (currentPath) {
        onPicked([currentPath])
      }
    } else {
      if (selectedPaths.length > 0) {
        onPicked(selectedPaths)
      }
    }
    onClose()
  }

  const toggleSelect = (p: string) => {
    if (mode === 'directory') {
      setSelectedPaths([p])
    } else {
      setSelectedPaths((prev) =>
        prev.includes(p) ? prev.filter((item) => item !== p) : [...prev, p]
      )
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-xs select-none animate-in fade-in duration-100">
      <div className="w-full max-w-3xl h-[78vh] bg-white dark:bg-[#18181b] border border-zinc-200 dark:border-[#303036] rounded-2xl shadow-2xl flex flex-col overflow-hidden animate-in zoom-in-95 duration-150">
        {/* 顶部标题与关闭 */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416]">
          <div className="flex items-center space-x-2">
            <Folder size={16} className="text-blue-500" />
            <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
              {title || (mode === 'directory' ? '选择工作区目录' : '选择引用文件')}
            </span>
          </div>

          <button
            onClick={onClose}
            className="p-1 rounded-lg text-zinc-400 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200 dark:hover:bg-zinc-800 transition-colors cursor-pointer"
          >
            <X size={15} />
          </button>
        </div>

        {/* 驱动器/根节点快捷工具条 */}
        <div className="flex items-center space-x-1.5 px-3 py-1.5 border-b border-zinc-100 dark:border-[#27272a] bg-white dark:bg-[#1a1a1e] overflow-x-auto text-xs">
          {roots.map((r) => (
            <button
              key={r.path}
              onClick={() => loadDir(r.path)}
              className={`flex items-center space-x-1.5 px-2.5 py-1 rounded-lg font-mono text-[11px] font-medium transition-colors cursor-pointer flex-shrink-0 ${
                currentPath.toLowerCase().startsWith(r.path.toLowerCase())
                  ? 'bg-blue-600 text-white'
                  : 'bg-zinc-100 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700'
              }`}
            >
              {r.kind === 'home' ? (
                <Home size={12} />
              ) : (
                <HardDrive size={12} />
              )}
              <span>{r.label}</span>
            </button>
          ))}
        </div>

        {/* 路径导航条与返回上一级 */}
        <div className="flex items-center space-x-1.5 px-3 py-2 border-b border-zinc-200 dark:border-[#27272a] bg-zinc-50/50 dark:bg-[#151518]">
          <button
            disabled={!parentPath}
            onClick={() => parentPath && loadDir(parentPath)}
            className="p-1.5 rounded-lg bg-white dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700 text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-700 disabled:opacity-40 transition-colors cursor-pointer"
            title="返回上一级目录"
          >
            <ArrowUp size={13} />
          </button>

          <div className="flex-1 flex items-center px-2 py-1 bg-white dark:bg-[#1a1a1e] border border-zinc-200 dark:border-zinc-700 rounded-lg text-xs font-mono">
            <input
              type="text"
              value={pathInput}
              onChange={(e) => setPathInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') loadDir(pathInput)
              }}
              className="w-full bg-transparent outline-none text-zinc-900 dark:text-zinc-100 text-[11.5px]"
            />
          </div>

          <button
            onClick={() => setCreatingFolder(!creatingFolder)}
            className="flex items-center space-x-1 px-2.5 py-1 rounded-lg bg-zinc-100 dark:bg-zinc-800 border border-zinc-200 dark:border-zinc-700 text-zinc-700 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700 text-xs font-medium transition-colors cursor-pointer"
            title="新建文件夹"
          >
            <Plus size={12} />
            <span>新建文件夹</span>
          </button>
        </div>

        {/* 新建文件夹输入行 */}
        {creatingFolder && (
          <div className="flex items-center space-x-2 px-3 py-2 bg-blue-50/50 dark:bg-blue-950/20 border-b border-blue-200 dark:border-blue-900/50">
            <span className="text-xs text-blue-700 dark:text-blue-300 font-medium">新文件夹名称:</span>
            <input
              type="text"
              value={newFolderName}
              onChange={(e) => setNewFolderName(e.target.value)}
              placeholder="输入名称..."
              autoFocus
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleCreateFolder()
                if (e.key === 'Escape') setCreatingFolder(false)
              }}
              className="flex-1 px-2 py-0.5 text-xs bg-white dark:bg-zinc-800 border border-blue-300 dark:border-blue-700 rounded-md outline-none text-zinc-900 dark:text-zinc-100"
            />
            <button
              onClick={handleCreateFolder}
              className="px-2.5 py-0.5 bg-blue-600 hover:bg-blue-500 text-white rounded-md text-xs font-medium cursor-pointer"
            >
              创建
            </button>
            <button
              onClick={() => setCreatingFolder(false)}
              className="px-2 py-0.5 text-zinc-500 hover:text-zinc-700 text-xs cursor-pointer"
            >
              取消
            </button>
          </div>
        )}

        {/* 错误提示 */}
        {error && (
          <div className="px-4 py-2 bg-rose-50 dark:bg-rose-950/30 text-rose-600 dark:text-rose-400 text-xs font-medium border-b border-rose-200 dark:border-rose-900/50">
            {error}
          </div>
        )}

        {/* 目录内容网格列表 */}
        <div className="flex-1 overflow-y-auto p-3">
          {loading ? (
            <div className="py-20 text-center text-xs text-zinc-400">正在读取目录内容...</div>
          ) : dirs.length === 0 && files.length === 0 ? (
            <div className="py-20 text-center text-xs text-zinc-400">当前目录为空</div>
          ) : (
            <div className="space-y-1">
              {/* 文件夹列表 */}
              {dirs.map((dir) => {
                const isSelected = selectedPaths.includes(dir.path)
                return (
                  <div
                    key={dir.path}
                    onClick={() => toggleSelect(dir.path)}
                    onDoubleClick={() => loadDir(dir.path)}
                    className={`flex items-center justify-between px-3 py-1.5 rounded-xl cursor-pointer transition-colors text-xs select-none ${
                      isSelected
                        ? 'bg-blue-600 text-white'
                        : 'text-zinc-800 dark:text-zinc-200 hover:bg-zinc-100 dark:hover:bg-zinc-800/80'
                    }`}
                  >
                    <div className="flex items-center space-x-2.5 min-w-0 flex-1">
                      <Folder
                        size={15}
                        className={isSelected ? 'text-white' : 'text-blue-500 flex-shrink-0'}
                      />
                      <span className="font-mono truncate">{dir.name}</span>
                    </div>

                    <span
                      className={`text-[10px] ${
                        isSelected ? 'text-blue-100' : 'text-zinc-400'
                      }`}
                    >
                      双击进入
                    </span>
                  </div>
                )
              })}

              {/* 文件列表 (文件模式可见) */}
              {mode === 'files' && (() => {
                const isImageFile = (fileName: string) => {
                  const ext = fileName.toLowerCase()
                  return ['.png', '.jpg', '.jpeg', '.gif', '.webp', '.svg', '.bmp', '.ico'].some((e) => ext.endsWith(e))
                }
                const filteredFiles = filterExts && filterExts.length > 0
                  ? files.filter((f) => filterExts.some((ext) => f.name.toLowerCase().endsWith(ext.toLowerCase())))
                  : files

                if (filteredFiles.length === 0 && dirs.length === 0) {
                  return (
                    <div className="py-12 text-center text-xs text-zinc-400">
                      当前目录暂无符合条件的文件
                    </div>
                  )
                }

                return filteredFiles.map((file) => {
                  const isSelected = selectedPaths.includes(file.path)
                  const isImg = isImageFile(file.name)
                  return (
                    <div
                      key={file.path}
                      onClick={() => toggleSelect(file.path)}
                      className={`flex items-center justify-between px-3 py-1.5 rounded-xl cursor-pointer transition-colors text-xs select-none ${
                        isSelected
                          ? 'bg-blue-600 text-white'
                          : 'text-zinc-800 dark:text-zinc-200 hover:bg-zinc-100 dark:hover:bg-zinc-800/80'
                      }`}
                    >
                      <div className="flex items-center space-x-2.5 min-w-0 flex-1">
                        {isImg ? (
                          <ImageIcon
                            size={14}
                            className={isSelected ? 'text-white' : 'text-indigo-500 flex-shrink-0'}
                          />
                        ) : (
                          <FileText
                            size={14}
                            className={isSelected ? 'text-white' : 'text-zinc-400 flex-shrink-0'}
                          />
                        )}
                        <span className="font-mono truncate">{file.name}</span>
                      </div>

                      <span
                        className={`text-[10px] font-mono ${
                          isSelected ? 'text-blue-100' : 'text-zinc-400'
                        }`}
                      >
                        {(file.size / 1024).toFixed(1)} KB
                      </span>
                    </div>
                  )
                })
              })()}
            </div>
          )}
        </div>

        {/* 底部确认栏 */}
        <div className="flex items-center justify-between px-4 py-3 border-t border-zinc-200 dark:border-[#27272a] bg-zinc-50 dark:bg-[#141416]">
          <div className="text-xs text-zinc-500 dark:text-zinc-400 font-mono truncate max-w-md">
            当前选择:{' '}
            <span className="font-semibold text-zinc-800 dark:text-zinc-200">
              {selectedPaths.length > 0
                ? selectedPaths.join(', ')
                : mode === 'directory'
                ? currentPath
                : '未选择文件'}
            </span>
          </div>

          <div className="flex items-center space-x-2">
            <button
              onClick={onClose}
              className="px-3 py-1.5 rounded-xl text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200 dark:hover:bg-zinc-800 text-xs font-medium transition-colors cursor-pointer"
            >
              取消
            </button>
            <button
              onClick={handleConfirm}
              className="flex items-center space-x-1 px-4 py-1.5 rounded-xl bg-blue-600 hover:bg-blue-500 text-white text-xs font-medium transition-colors shadow-xs cursor-pointer"
            >
              <Check size={13} />
              <span>
                {mode === 'directory' ? '选定此工作区' : `确定 (${selectedPaths.length})`}
              </span>
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
