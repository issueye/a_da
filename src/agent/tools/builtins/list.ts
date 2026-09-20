/**
 * 文件列表工具 (list_files / ls)
 * 参考 @earendil-works/pi-coding-agent/src/core/tools/ls.ts
 */

import { readdir, stat } from 'node:fs/promises'
import { join, relative } from 'node:path'
import type { AgentTool, AgentToolResult } from '../../core/types'
import type { ListToolArgs } from '../types'
import { SKIP_DIRS, checkWorkspaceSandbox } from '../workspace'

const MAX_LIST_ENTRIES = 200

interface WalkEntry {
  path: string
  dir: boolean
  size: number
}

function toPosix(path: string): string {
  return path.replace(/\\/g, '/')
}

async function walkDir(
  root: string,
  dir: string,
  depth: number,
  maxDepth: number,
  budget: { left: number }
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
        entries.push(...(await walkDir(root, absolute, depth + 1, maxDepth, budget)))
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

export function createListTool(workspace: string): AgentTool<ListToolArgs> {
  return {
    name: 'list_files',
    label: '列出文件',
    description: '列出工作区内的文件与目录。在读取未知路径前请先使用此工具确认结构。',
    parameters: {
      type: 'object',
      properties: {
        path: { type: 'string', description: '相对于工作区根目录的子目录（可选）。' },
        depth: { type: 'number', description: '遍历深度（默认 3）。' },
      },
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      try {
        const targetDir = args.path ? checkWorkspaceSandbox(workspace, args.path) : workspace
        const depth = typeof args.depth === 'number' ? Math.min(Math.max(args.depth, 1), 6) : 3
        const budget = { left: MAX_LIST_ENTRIES }

        const entries = await walkDir(workspace, targetDir, 1, depth, budget)
        if (!entries.length) {
          return { ok: true, output: '(空目录)' }
        }

        const lines = entries.map((entry) =>
          entry.dir ? `${entry.path}/` : `${entry.path}  ${Math.max(1, Math.round(entry.size / 1024))}k`
        )
        const more = budget.left <= 0 ? '\n… 列表已截断' : ''

        return {
          output: `${lines.join('\n')}${more}`,
          ok: true,
          details: { count: entries.length },
        }
      } catch (err) {
        return {
          output: (err as Error).message,
          ok: false,
        }
      }
    },
  }
}
