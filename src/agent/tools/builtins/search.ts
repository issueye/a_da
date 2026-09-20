/**
 * 正则搜索文件工具 (search_files / grep)
 * 参考 @earendil-works/pi-coding-agent/src/core/tools/grep.ts
 */

import { readdir, readFile, stat } from 'node:fs/promises'
import { join, relative } from 'node:path'
import { truncateTail } from '../../core/truncate'
import type { AgentTool, AgentToolResult } from '../../core/types'
import type { SearchToolArgs } from '../types'
import { SKIP_DIRS } from '../workspace'

const MAX_MATCHES = 200
const MAX_FILE_BYTES = 512 * 1024
const MAX_DEPTH = 8

export function createSearchTool(workspace: string): AgentTool<SearchToolArgs> {
  return {
    name: 'search_files',
    label: '搜索文件',
    description: '使用 JavaScript 正则表达式在工作区文件中搜索匹配内容并返回带行号的匹配行。',
    parameters: {
      type: 'object',
      properties: {
        pattern: { type: 'string', description: 'JavaScript 正则表达式。' },
        glob: { type: 'string', description: '可选的文件扩展名过滤（例如 "tsx" 或 "rs"）。' },
      },
      required: ['pattern'],
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      const source = String(args.pattern ?? '').trim()
      if (!source) {
        return { ok: false, output: '缺少 pattern 正则表达式参数。' }
      }

      let regex: RegExp
      try {
        regex = new RegExp(source, 'i')
      } catch (err) {
        return { ok: false, output: `正则表达式无效：${(err as Error).message}` }
      }

      const glob = typeof args.glob === 'string' ? args.glob.replace(/^\./, '') : null
      const matches: string[] = []

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
            const rel = relative(workspace, full).replace(/\\/g, '/')
            const lines = buffer.toString('utf-8').split('\n')

            for (let lineIdx = 0; lineIdx < lines.length; lineIdx++) {
              const line = lines[lineIdx]!
              if (regex.test(line)) {
                matches.push(`${rel}:${lineIdx + 1}: ${line.trim().slice(0, 200)}`)
                if (matches.length >= MAX_MATCHES) break
              }
            }
          } catch {
            continue
          }
        }
      }

      try {
        await searchInDir(workspace, 1)
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
