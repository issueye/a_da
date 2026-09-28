/**
 * 工作区沙箱与工具系统统一适配
 * 向后兼容保留现有工具 API，同时底层对接到 @earendil-works/pi-coding-agent 风格的 ToolRegistry
 */

import { readdir, stat } from 'node:fs/promises'
import { join, relative, resolve } from 'node:path'
import type { ToolOutcome } from './types'
import { defaultToolRegistry, ToolRegistry } from './tools/registry'
import { defaultExtensionLoader } from './tools/loader'
import { SKIP_DIRS, isPathInsideWorkspace } from './tools/workspace'

export { defaultToolRegistry, ToolRegistry, defaultExtensionLoader }

const MAX_WALK_ENTRIES = 4000

function toPosix(path: string): string {
  return path.replace(/\\/g, '/')
}

interface WalkEntry {
  path: string
  dir: boolean
  size: number
}

interface WalkBudget {
  left: number
}

async function walk(
  root: string,
  dir: string,
  depth: number,
  maxDepth: number,
  budget: WalkBudget = { left: MAX_WALK_ENTRIES }
): Promise<WalkEntry[]> {
  const entries: WalkEntry[] = []
  if (budget.left <= 0) return entries
  let dirents
  try {
    dirents = await readdir(dir, { withFileTypes: true })
  } catch {
    return entries
  }
  dirents.sort((a, b) => Number(b.isDirectory()) - Number(a.isDirectory()) || a.name.localeCompare(b.name))
  for (const dirent of dirents) {
    if (budget.left <= 0) return entries
    if (dirent.name.startsWith('.') && dirent.name !== '.github') continue
    const absolute = join(dir, dirent.name)
    if (dirent.isDirectory() || dirent.isSymbolicLink()) {
      // 指到工作区外的 symlink/junction 整枝跳过，不让索引和附件清单泄出内容
      if (!isPathInsideWorkspace(root, absolute)) continue
    }
    if (dirent.isDirectory()) {
      if (SKIP_DIRS.has(dirent.name)) continue
      budget.left -= 1
      entries.push({ path: toPosix(relative(root, absolute)), dir: true, size: 0 })
      if (depth < maxDepth) {
        entries.push(...(await walk(root, absolute, depth + 1, maxDepth, budget)))
      }
    } else {
      let size = 0
      try {
        size = (await stat(absolute)).size
      } catch {
        size = 0
      }
      budget.left -= 1
      entries.push({ path: toPosix(relative(root, absolute)), dir: false, size })
    }
  }
  return entries
}

export interface ToolRequest {
  name: string
  args: Record<string, unknown>
}

/**
 * 这次调用是否会改动工作区（决定「只读」模式下要不要审批）。
 * 只读白名单在 ToolRegistry 里，名单外的工具——包括扩展注册的——都算写操作。
 */
export function isWriteTool(name: string): boolean {
  return defaultToolRegistry.isWriteTool(name)
}

/**
 * 统一执行工具（桥接至 ToolRegistry）
 */
export async function runTool(root: string, request: ToolRequest): Promise<ToolOutcome> {
  const tools = defaultToolRegistry.getToolsForWorkspace(root)
  const target = tools.find((t) => t.name === request.name)

  if (!target) {
    return { ok: false, output: `未知工具：${request.name}` }
  }

  try {
    const result = await target.execute(`call_${Date.now()}`, request.args)
    return {
      ok: result.ok,
      output: result.output,
      patch: result.patch,
    }
  } catch (error) {
    return { ok: false, output: (error as Error).message }
  }
}

/** 工具摘要格式化（卡片头部展示） */
export function describeTool(name: string, args: Record<string, unknown>): string {
  switch (name) {
    case 'run_command':
      return String(args.command ?? '').replace(/\r?\n+/g, ' ').trim()
    case 'run_background':
      return String(args.command ?? '').replace(/\r?\n+/g, ' ').trim()
    case 'check_task': {
      const taskId = String(args.task_id ?? '').trim()
      return taskId || '全部任务'
    }
    case 'kill_task':
      return String(args.task_id ?? '').replace(/\r?\n+/g, ' ').trim()
    case 'search_files':
      return `/${String(args.pattern ?? '').replace(/\r?\n+/g, ' ').trim()}/`
    case 'find_symbol': {
      const query = String(args.query ?? '').replace(/\r?\n+/g, ' ').trim()
      const kind = typeof args.kind === 'string' && args.kind ? `:${args.kind}` : ''
      return `@${query}${kind}`
    }
    case 'list_files':
      return String(args.path || '.').replace(/\r?\n+/g, ' ').trim()
    case 'read_file':
    case 'write_file':
    case 'edit_file':
      return String(args.path ?? '').replace(/\r?\n+/g, ' ').trim()
    case 'read_files': {
      const paths = Array.isArray(args.paths) ? (args.paths as unknown[]) : []
      const files = Array.isArray(args.files) ? (args.files as { path?: unknown }[]) : []
      const count = paths.length + files.length
      const first = String(paths[0] ?? files[0]?.path ?? '').replace(/\r?\n+/g, ' ').trim()
      return count > 0 ? `${first}${count > 1 ? ` 等 ${count} 个文件` : ''}` : ''
    }
    case 'edit_files': {
      const files = Array.isArray(args.files) ? (args.files as { path?: unknown }[]) : []
      const first = String(files[0]?.path ?? '').replace(/\r?\n+/g, ' ').trim()
      return files.length > 0 ? `${first}${files.length > 1 ? ` 等 ${files.length} 个文件` : ''}` : ''
    }
    case 'todo': {
      const todos = Array.isArray(args.todos) ? (args.todos as { title?: string; status?: string }[]) : []
      const active = todos.find((t) => t.status === 'in_progress') ?? todos.find((t) => t.status !== 'completed')
      return (active?.title ?? (todos.length ? `${todos.filter((t) => t.status === 'completed').length}/${todos.length}` : '')).replace(/\r?\n+/g, ' ').trim()
    }
    case 'invoke_subagent': {
      const subagentId = String(args.subagent_id ?? '').trim()
      const task = String(args.task ?? '').split('\n')[0]!.trim()
      return `${subagentId}: ${task}`
    }
    case 'check_subagent': {
      return String(args.subagent_thread_id ?? args.subagent_id ?? '查询子智能体').replace(/\r?\n+/g, ' ').trim()
    }
    case 'await_subagents': {
      const ids = Array.isArray(args.subagent_thread_ids) ? (args.subagent_thread_ids as unknown[]) : []
      if (ids.length === 0) return '全部运行中的子智能体'
      const first = String(ids[0] ?? '').replace(/\r?\n+/g, ' ').trim()
      return ids.length > 1 ? `${first} 等 ${ids.length} 个` : first
    }
    case 'notify_parent':
      return String(args.summary ?? '').replace(/\r?\n+/g, ' ').trim().slice(0, 60)
    default:
      return ''
  }
}

/** 扫描工作区统计文件信息 */
export async function scanWorkspace(
  root: string
): Promise<{ files: number; dirs: number; entries: string[] }> {
  const entries = await walk(root, root, 1, 8)
  const topLevel = entries
    .filter((entry) => !/[/\\]/.test(entry.path))
    .map((entry) => (entry.dir ? `${entry.path}/` : entry.path))
  return {
    files: entries.filter((entry) => !entry.dir).length,
    dirs: entries.filter((entry) => entry.dir).length,
    entries: topLevel.slice(0, 40),
  }
}

/** 校验用户在侧边栏输入的项目路径 */
export async function resolveProjectPath(
  input: string
): Promise<{ path: string } | { error: string }> {
  let trimmed = input.trim().replace(/^['"]|['"]$/g, '')
  if (trimmed.startsWith('file://')) {
    try {
      const url = new URL(trimmed)
      trimmed = decodeURIComponent(url.pathname)
      if (process.platform === 'win32' && /^\/[a-zA-Z]:/.test(trimmed)) {
        trimmed = trimmed.slice(1)
      }
    } catch {
      // 容错保留原始串由 resolve 校验
    }
  }
  if (!trimmed) return { error: '请输入目录路径' }
  const absolute = resolve(trimmed)
  let info
  try {
    info = await stat(absolute)
  } catch {
    return { error: `路径不存在：${absolute}` }
  }
  if (!info.isDirectory()) return { error: `不是目录：${absolute}` }
  return { path: absolute }
}
