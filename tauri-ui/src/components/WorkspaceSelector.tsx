import React, { useState, useRef, useEffect } from 'react'
import { FolderGit2, FolderOpen, ChevronDown, Check, Plus, Trash2 } from 'lucide-react'

interface WorkspaceSelectorProps {
  currentWorkspace: string
  allWorkspaces: string[]
  onSelectWorkspace: (workspace: string) => void
  onRemoveWorkspace?: (workspace: string) => void
  onOpenPicker: () => void
}

/**
 * 新建会话界面的工作区选择与展示控件
 */
export const WorkspaceSelector: React.FC<WorkspaceSelectorProps> = ({
  currentWorkspace,
  allWorkspaces,
  onSelectWorkspace,
  onRemoveWorkspace,
  onOpenPicker,
}) => {
  const [dropdownOpen, setDropdownOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)

  // 点击外部关闭下拉菜单
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setDropdownOpen(false)
      }
    }
    if (dropdownOpen) {
      document.addEventListener('mousedown', handleClickOutside)
    }
    return () => {
      document.removeEventListener('mousedown', handleClickOutside)
    }
  }, [dropdownOpen])

  // 提取目录名
  const dirName = currentWorkspace
    ? currentWorkspace.split(/[\\/]/).filter(Boolean).pop() || currentWorkspace
    : '未选择工作区'

  return (
    <div className="relative inline-flex items-center" ref={menuRef}>
      <div className="flex items-center space-x-1.5 bg-zinc-100/90 dark:bg-zinc-800/70 hover:bg-zinc-200/80 dark:hover:bg-zinc-800 px-3 py-1.5 rounded-full border border-zinc-200/80 dark:border-zinc-700/60 shadow-xs transition-colors">
        {/* 图标与当前工作区名称（支持点击展开历史工作区下拉） */}
        <button
          type="button"
          onClick={() => setDropdownOpen(!dropdownOpen)}
          className="flex items-center space-x-1.5 text-xs text-zinc-700 dark:text-zinc-200 cursor-pointer outline-none"
          title={`当前工作区: ${currentWorkspace || '无'}。点击切换已记录的工作区`}
        >
          <FolderGit2 size={14} className="text-blue-500 flex-shrink-0" />
          <span className="font-semibold text-zinc-800 dark:text-zinc-100 max-w-[180px] truncate font-mono">
            {dirName}
          </span>
          <ChevronDown
            size={12}
            className={`text-zinc-400 dark:text-zinc-500 transition-transform duration-200 ${
              dropdownOpen ? 'rotate-180' : ''
            }`}
          />
        </button>

        <span className="text-zinc-300 dark:text-zinc-600 select-none">|</span>

        {/* 选择工作区按钮（点击唤起系统/内置文件浏览器） */}
        <button
          type="button"
          onClick={onOpenPicker}
          className="flex items-center space-x-1 text-xs text-blue-600 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 font-medium px-1.5 py-0.5 rounded-md hover:bg-blue-500/10 transition-colors cursor-pointer"
          title="使用全平台文件浏览器选择其它项目目录"
        >
          <FolderOpen size={13} />
          <span>选择工作区</span>
        </button>
      </div>

      {/* 工作区下拉切换浮层 */}
      {dropdownOpen && (
        <div className="absolute top-full mt-1.5 left-0 min-w-[280px] max-w-[360px] bg-white dark:bg-[#1f1f23] border border-zinc-200 dark:border-[#333338] rounded-xl shadow-2xl p-1.5 z-50 text-xs backdrop-blur-md animate-in fade-in zoom-in-95 duration-100">
          <div className="px-2 py-1 text-[10px] font-semibold text-zinc-400 dark:text-zinc-500 uppercase tracking-wider">
            切换项目工程
          </div>

          <div className="max-h-[220px] overflow-y-auto space-y-0.5 py-0.5">
            {allWorkspaces.map((ws) => {
              const name = ws.split(/[\\/]/).filter(Boolean).pop() || ws
              const isSelected = ws === currentWorkspace

              return (
                <div
                  key={ws}
                  onClick={() => {
                    onSelectWorkspace(ws)
                    setDropdownOpen(false)
                  }}
                  className={`group w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-left transition-colors cursor-pointer ${
                    isSelected
                      ? 'bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium'
                      : 'text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800'
                  }`}
                >
                  <div className="flex-1 min-w-0 pr-2">
                    <div className="truncate font-semibold">{name}</div>
                    <div className="text-[10px] text-zinc-400 dark:text-zinc-500 truncate font-mono">
                      {ws}
                    </div>
                  </div>
                  <div className="flex items-center space-x-1 flex-shrink-0">
                    {onRemoveWorkspace && allWorkspaces.length > 1 && (
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation()
                          onRemoveWorkspace(ws)
                        }}
                        className="opacity-0 group-hover:opacity-100 p-1 text-zinc-400 hover:text-rose-500 hover:bg-zinc-200/80 dark:hover:bg-zinc-700/80 rounded transition-all cursor-pointer"
                        title={`从列表中移除工作区「${name}」`}
                      >
                        <Trash2 size={12} />
                      </button>
                    )}
                    {isSelected && <Check size={14} className="text-blue-500 flex-shrink-0" />}
                  </div>
                </div>
              )
            })}
          </div>

          <div className="h-[1px] bg-zinc-200 dark:bg-zinc-800 my-1" />

          {/* 新增/打开工作区入口 */}
          <button
            type="button"
            onClick={() => {
              setDropdownOpen(false)
              onOpenPicker()
            }}
            className="w-full flex items-center space-x-2 px-2.5 py-1.5 rounded-lg text-left text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors cursor-pointer"
          >
            <Plus size={13} className="text-blue-500" />
            <span className="font-medium">打开其它文件夹...</span>
          </button>
        </div>
      )}
    </div>
  )
}
