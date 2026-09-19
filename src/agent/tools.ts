/**
 * The workspace sandbox.
 *
 * Every path the model asks for is resolved against the workspace root and
 * rejected when it escapes, so the agent can only touch the project the window
 * is pointed at. Only `run_command` reaches outside, and it is the one tool the
 * approval modes always guard.
 */

import { mkdir, readdir, readFile, stat, writeFile } from 'node:fs/promises'
import { dirname, isAbsolute, join, relative, resolve } from 'node:path'
import { unifiedPatch } from './patch'
import type { ToolOutcome } from './types'

const SKIP_DIRS = new Set([
  'node_modules',
  '.git',
  'dist',
  'build',
  'out',
  'target',
  '.next',
  '.cache',
  '.venv',
  '__pycache__',
  '.turbo',
  'coverage',
])

const MAX_FILE_BYTES = 512 * 1024
const MAX_READ_LINES = 400
const MAX_LIST_ENTRIES = 400
/** Entries one walk may gather in total, across every directory it enters. */
const MAX_WALK_ENTRIES = 4000
const MAX_MATCHES = 200
const MAX_OUTPUT = 8000
const COMMAND_TIMEOUT_MS = 120_000

export class WorkspaceError extends Error {}

/**
 * Tool output always uses forward slashes. The model echoes these paths back in
 * its next call, and a mixed `src\\app.ts` on Windows vs `src/app.ts` elsewhere
 * makes the same transcript mean different things.
 */
function toPosix(path: string): string {
  return path.replace(/\\/g, '/')
}

function truncate(text: string, max = MAX_OUTPUT): string {
  if (text.length <= max) return text
  const head = text.slice(0, Math.floor(max * 0.7))
  const tail = text.slice(-Math.floor(max * 0.2))
  return `${head}\n… [已截断 ${text.length - head.length - tail.length} 字符] …\n${tail}`
}

export function resolveInWorkspace(root: string, path: unknown): string {
  if (typeof path !== 'string' || !path.trim()) throw new WorkspaceError('缺少 path 参数')
  const absolute = isAbsolute(path) ? resolve(path) : resolve(root, path)
  const rel = relative(resolve(root), absolute)
  if (rel.startsWith('..') || isAbsolute(rel)) {
    throw new WorkspaceError(`拒绝访问工作区外的路径：${path}`)
  }
  return absolute
}

interface WalkEntry {
  path: string
  dir: boolean
  size: number
}

/**
 * A shared budget across the whole walk.
 *
 * A per-directory limit is not enough: a repo with `zed/` checked out inside it
 * would still gather every file at every level, and the sidebar scan has to
 * stay a fraction of a second. `left` is consumed as entries are produced, and
 * the recursion stops as soon as it runs out.
 */
interface WalkBudget {
  left: number
}

async function walk(
  root: string,
  dir: string,
  depth: number,
  maxDepth: number,
  budget: WalkBudget = { left: MAX_WALK_ENTRIES },
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

async function readTextFile(absolute: string): Promise<string> {
  const info = await stat(absolute)
  if (info.isDirectory()) throw new WorkspaceError('这是目录，不是文件')
  if (info.size > MAX_FILE_BYTES) throw new WorkspaceError(`文件过大（${info.size} 字节），无法读取`)
  const buffer = await readFile(absolute)
  if (buffer.subarray(0, 4096).includes(0)) throw new WorkspaceError('这是二进制文件')
  return buffer.toString('utf8')
}

async function listFiles(root: string, args: Record<string, unknown>): Promise<ToolOutcome> {
  const base = args.path ? resolveInWorkspace(root, args.path) : root
  const depth = typeof args.depth === 'number' ? Math.min(Math.max(args.depth, 1), 6) : 3
  const budget: WalkBudget = { left: MAX_LIST_ENTRIES }
  const entries = await walk(root, base, 1, depth, budget)
  if (!entries.length) return { ok: true, output: '(空目录)' }
  const lines = entries.map((entry) =>
    entry.dir ? `${entry.path}/` : `${entry.path}  ${Math.max(1, Math.round(entry.size / 1024))}k`,
  )
  const more = budget.left <= 0 ? '\n… 列表已截断' : ''
  return { ok: true, output: `${lines.join('\n')}${more}` }
}

async function readFileTool(root: string, args: Record<string, unknown>): Promise<ToolOutcome> {
  const absolute = resolveInWorkspace(root, args.path)
  const text = await readTextFile(absolute)
  const lines = text.split('\n')
  const shown = lines.slice(0, MAX_READ_LINES)
  const more = lines.length > MAX_READ_LINES ? `\n… 余下 ${lines.length - MAX_READ_LINES} 行未显示` : ''
  return { ok: true, output: `${shown.join('\n')}${more}` }
}

async function searchFiles(root: string, args: Record<string, unknown>): Promise<ToolOutcome> {
  const source = String(args.pattern ?? '')
  if (!source) throw new WorkspaceError('缺少 pattern 参数')
  let pattern: RegExp
  try {
    pattern = new RegExp(source, 'i')
  } catch (error) {
    throw new WorkspaceError(`正则表达式无效：${(error as Error).message}`)
  }
  const glob = typeof args.glob === 'string' ? args.glob.replace(/^\./, '') : null
  const entries = await walk(root, root, 1, 8)
  const matches: string[] = []
  for (const entry of entries) {
    if (entry.dir || matches.length >= MAX_MATCHES) continue
    if (glob && !entry.path.endsWith(`.${glob}`)) continue
    let text: string
    try {
      text = await readTextFile(join(root, entry.path))
    } catch {
      continue
    }
    const lines = text.split('\n')
    for (let index = 0; index < lines.length; index++) {
      if (pattern.test(lines[index]!)) {
        matches.push(`${entry.path}:${index + 1}: ${lines[index]!.trim().slice(0, 200)}`)
        if (matches.length >= MAX_MATCHES) break
      }
    }
  }
  if (!matches.length) return { ok: true, output: '没有匹配' }
  return { ok: true, output: matches.join('\n') }
}

async function writeFileTool(root: string, args: Record<string, unknown>): Promise<ToolOutcome> {
  const absolute = resolveInWorkspace(root, args.path)
  const content = String(args.content ?? '')
  let before = ''
  try {
    before = await readTextFile(absolute)
  } catch {
    before = ''
  }
  await mkdir(dirname(absolute), { recursive: true })
  await writeFile(absolute, content, 'utf8')
  const rel = toPosix(relative(root, absolute))
  const patch = unifiedPatch(rel, before, content)
  return {
    ok: true,
    output: before ? `已写入 ${rel}` : `已创建 ${rel}（${content.split('\n').length} 行）`,
    patch,
  }
}

async function editFileTool(root: string, args: Record<string, unknown>): Promise<ToolOutcome> {
  const absolute = resolveInWorkspace(root, args.path)
  const oldString = String(args.old_string ?? '')
  const newString = String(args.new_string ?? '')
  if (!oldString) throw new WorkspaceError('old_string 不能为空')
  const before = await readTextFile(absolute)
  const first = before.indexOf(oldString)
  if (first < 0) return { ok: false, output: 'old_string 在文件中不存在，请先 read_file 确认内容' }
  if (before.indexOf(oldString, first + 1) >= 0) {
    return { ok: false, output: 'old_string 出现了多次，请提供更长的上下文让它唯一' }
  }
  const after = before.slice(0, first) + newString + before.slice(first + oldString.length)
  await writeFile(absolute, after, 'utf8')
  const rel = toPosix(relative(root, absolute))
  return { ok: true, output: `已修改 ${rel}`, patch: unifiedPatch(rel, before, after) }
}

async function runCommand(root: string, args: Record<string, unknown>): Promise<ToolOutcome> {
  const command = String(args.command ?? '').trim()
  if (!command) throw new WorkspaceError('缺少 command 参数')
  const cwd = args.cwd ? resolveInWorkspace(root, args.cwd) : root
  const shell =
    process.platform === 'win32'
      ? ['cmd.exe', '/d', '/s', '/c', command]
      : ['/bin/sh', '-lc', command]
  const proc = Bun.spawn(shell, {
    cwd,
    stdin: 'ignore',
    stdout: 'pipe',
    stderr: 'pipe',
    env: { ...process.env, A_DA_AGENT: '1' },
  })
  let timedOut = false
  const timer = setTimeout(() => {
    timedOut = true
    proc.kill()
  }, COMMAND_TIMEOUT_MS)
  const [stdout, stderr] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
  ])
  const code = await proc.exited
  clearTimeout(timer)
  const parts: string[] = []
  if (stdout.trim()) parts.push(stdout.trim())
  if (stderr.trim()) parts.push(`stderr:\n${stderr.trim()}`)
  if (timedOut) parts.push(`命令超时（${COMMAND_TIMEOUT_MS / 1000}s），已终止`)
  parts.push(`退出码 ${code}`)
  return { ok: code === 0 && !timedOut, output: truncate(parts.join('\n\n')) }
}

export interface ToolRequest {
  name: string
  args: Record<string, unknown>
}

export function isWriteTool(name: string): boolean {
  return name === 'write_file' || name === 'edit_file' || name === 'run_command'
}

export async function runTool(root: string, request: ToolRequest): Promise<ToolOutcome> {
  try {
    switch (request.name) {
      case 'list_files':
        return await listFiles(root, request.args)
      case 'read_file':
        return await readFileTool(root, request.args)
      case 'search_files':
        return await searchFiles(root, request.args)
      case 'write_file':
        return await writeFileTool(root, request.args)
      case 'edit_file':
        return await editFileTool(root, request.args)
      case 'run_command':
        return await runCommand(root, request.args)
      default:
        return { ok: false, output: `未知工具：${request.name}` }
    }
  } catch (error) {
    return { ok: false, output: `${(error as Error).message}` }
  }
}

/** One-line summary for the tool card header. */
export function describeTool(name: string, args: Record<string, unknown>): string {
  switch (name) {
    case 'run_command':
      return String(args.command ?? '')
    case 'search_files':
      return `/${String(args.pattern ?? '')}/`
    case 'list_files':
      return String(args.path || '.')
    case 'read_file':
    case 'write_file':
    case 'edit_file':
      return String(args.path ?? '')
    default:
      return ''
  }
}

/** Counts the workspace so the project row can show what the agent can see. */
export async function scanWorkspace(
  root: string,
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

/**
 * Check a project path the user typed in the sidebar.
 *
 * `addProject` must fail on the spot rather than add a project that turns every
 * later tool call into an error, so the path is resolved and stat-ed first.
 */
export async function resolveProjectPath(
  input: string,
): Promise<{ path: string } | { error: string }> {
  const trimmed = input.trim().replace(/^"|"$/g, '')
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
