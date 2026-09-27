/**
 * 正则搜索文件工具 (search_files / grep)
 * 参考 @earendil-works/pi-coding-agent/src/core/tools/grep.ts
 */

import { readdir, readFile, stat } from 'node:fs/promises'
import { join, relative } from 'node:path'
import { truncateTail } from '../../core/truncate'
import type { AgentTool, AgentToolResult } from '../../core/types'
import type { SearchToolArgs } from '../types'
import { SKIP_DIRS, checkWorkspaceSandbox, isPathInsideWorkspace, resolveRealWorkspace } from '../workspace'

const MAX_MATCHES = 200
const MAX_FILE_BYTES = 512 * 1024
const MAX_DEPTH = 8
const MAX_CONTEXT = 3

export function createSearchTool(workspace: string): AgentTool<SearchToolArgs> {
  return {
    name: 'search_files',
    label: '搜索文件',
    description:
      '在工作区文件内容中搜索并返回带行号的匹配行。pattern 接受 JavaScript 正则；literal=true 时按纯文本搜索；支持 glob 扩展名过滤、path 子目录限定与 context 上下文行。找符号定义请优先用 find_symbol。',
    parameters: {
      type: 'object',
      properties: {
        pattern: { type: 'string', description: 'JavaScript 正则表达式（literal=true 时为纯文本）。' },
        glob: { type: 'string', description: '可选的文件扩展名过滤（例如 "tsx" 或 "rs"）。' },
        path: { type: 'string', description: '可选，限定在某个子目录内搜索（相对工作区）。' },
        literal: { type: 'boolean', description: '可选，把 pattern 当纯文本而不是正则。' },
        case_sensitive: { type: 'boolean', description: '可选，区分大小写（默认不区分）。' },
        context: { type: 'number', description: `可选，每个匹配附带的上下文行数（0-${MAX_CONTEXT}，默认 0）。` },
      },
      required: ['pattern'],
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      const source = String(args.pattern ?? '').trim()
      if (!source) {
        return { ok: false, output: '缺少 pattern 正则表达式参数。' }
      }

      const patternBody = args.literal ? source.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') : source
      let regex: RegExp
      try {
        regex = new RegExp(patternBody, args.case_sensitive ? '' : 'i')
      } catch (err) {
        return { ok: false, output: `正则表达式无效：${(err as Error).message}` }
      }

      const glob = typeof args.glob === 'string' ? args.glob.replace(/^\./, '') : null
      const contextLines = Math.min(Math.max(Math.trunc(Number(args.context ?? 0) || 0), 0), MAX_CONTEXT)
      const matches: string[] = []
      const seen = new Set<string>()
      // 根目录与子目录限定共用真实落点：workspace 根自己是 symlink 时也不会算歪
      const rootDir = resolveRealWorkspace(workspace)
      let startDir = rootDir
      if (typeof args.path === 'string' && args.path.trim()) {
        // 限定子目录时同样过沙箱：相对路径按 workspace 解析，越界直接报错
        try {
          startDir = checkWorkspaceSandbox(workspace, args.path.trim())
        } catch (err) {
          return { ok: false, output: (err as Error).message }
        }
      }

      const pushMatch = (rel: string, lineIdx: number, lines: string[]): void => {
        const line = lines[lineIdx]!
        const key = `${rel}:${lineIdx}`
        if (seen.has(key)) return
        seen.add(key)
        matches.push(`${rel}:${lineIdx + 1}: ${line.trim().slice(0, 200)}`)
        if (contextLines > 0) {
          for (let offset = 1; offset <= contextLines; offset++) {
            const before = lines[lineIdx - offset]
            const after = lines[lineIdx + offset]
            if (before !== undefined && !seen.has(`${rel}:${lineIdx - offset}`)) {
              seen.add(`${rel}:${lineIdx - offset}`)
              matches.push(`${rel}:${lineIdx + 1 - offset}: ${before.trim().slice(0, 200)}`)
            }
            if (after !== undefined && !seen.has(`${rel}:${lineIdx + offset}`)) {
              seen.add(`${rel}:${lineIdx + offset}`)
              matches.push(`${rel}:${lineIdx + offset + 1}: ${after.trim().slice(0, 200)}`)
            }
          }
        }
      }

      const searchInDir = async (dir: string, currentDepth: number): Promise<void> => {
        if (currentDepth > MAX_DEPTH || matches.length >= MAX_MATCHES) return

        let dirents
        try {
          dirents = await readdir(dir, { withFileTypes: true })
        } catch {
          return
        }

        for (const dirent of dirents) {
          if (matches.length >= MAX_MATCHES) break
          if (dirent.name.startsWith('.') && dirent.name !== '.github') continue

          const full = join(dir, dirent.name)
          if (dirent.isDirectory() || dirent.isSymbolicLink()) {
            // 指到工作区外的链接（目录或文件）一律跳过，内容不外泄
            if (!isPathInsideWorkspace(rootDir, full)) continue
          }
          if (dirent.isDirectory()) {
            if (SKIP_DIRS.has(dirent.name)) continue
            await searchInDir(full, currentDepth + 1)
            continue
          }

          if (glob && !dirent.name.endsWith(`.${glob}`)) continue
          try {
            const fileStat = await stat(full)
            if (fileStat.size > MAX_FILE_BYTES) continue

            const buffer = await readFile(full)
            if (buffer.subarray(0, 2048).includes(0)) continue // 二进制跳过

            // 相对路径交给 path.relative，别用字符串切片：工作区末尾的分隔符和
            // Windows 上的大小写都会让它算出奇怪的东西。
            const rel = relative(rootDir, full).replace(/\\/g, '/')
            const lines = buffer.toString('utf-8').split('\n')

            for (let lineIdx = 0; lineIdx < lines.length; lineIdx++) {
              if (regex.test(lines[lineIdx]!)) {
                pushMatch(rel, lineIdx, lines)
                if (matches.length >= MAX_MATCHES) break
              }
            }
          } catch {
            continue
          }
        }
      }

      try {
        await searchInDir(startDir, 1)
        if (!matches.length) {
          return { ok: true, output: '没有匹配' }
        }

        const combined = matches.join('\n')
        const truncated = truncateTail(combined, MAX_MATCHES, 25 * 1024)
        return {
          output: truncated.content,
          ok: true,
          details: { matchesCount: matches.length },
        }
      } catch (err) {
        return { ok: false, output: (err as Error).message }
      }
    },
  }
}
