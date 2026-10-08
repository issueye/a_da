/**
 * 符号索引工具 (find_symbol)
 *
 * 不引入语言服务器也不引嵌入模型：按语言的定义模式（function / class /
 * struct / def / fn …）扫描工作区，建一份「名字 → 定义位置」的倒排索引。
 * 模型先用它按名字定位符号，再拿 read_file 读上下文——比正则全文搜索准
 * 得多，因为排除了一切非定义行的噪音。
 *
 * 索引按工作区缓存 60 秒；传 refresh=1 强制重建（刚大改完代码的场景）。
 * 支持的语言：TS/JS/JSX/TSX、Python、Rust、Go、Java、C#。其余语言静默跳过。
 */

import { readFile, readdir, stat } from 'node:fs/promises'
import { join, relative } from 'node:path'
import { truncateTail } from '../../core/truncate'
import type { AgentTool, AgentToolResult } from '../../core/types'
import { SKIP_DIRS, isPathInsideWorkspace, resolveRealWorkspace } from '../workspace'

const MAX_FILES = 3000
const MAX_FILE_BYTES = 512 * 1024
const MAX_RESULTS = 80
const INDEX_TTL_MS = 60_000

export interface SymbolEntry {
  name: string
  kind: string
  file: string
  line: number
  signature: string
}

interface LanguagePattern {
  /** 传给 readdir 扩展名过滤；空数组表示靠文件名判断 */
  extensions: string[]
  /** 文件名精确匹配（如 Makefile） */
  filenames?: string[]
  patterns: Array<{ re: RegExp; kind: string; nameGroup: number }>
}

const LANGUAGES: LanguagePattern[] = [
  {
    extensions: ['ts', 'tsx', 'js', 'jsx', 'mjs', 'cjs', 'mts', 'cts'],
    patterns: [
      { re: /^\s*(export\s+)?(default\s+)?(async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)/, kind: 'function', nameGroup: 4 },
      { re: /^\s*(export\s+)?(abstract\s+)?class\s+([A-Za-z_$][\w$]*)/, kind: 'class', nameGroup: 3 },
      { re: /^\s*(export\s+)?interface\s+([A-Za-z_$][\w$]*)/, kind: 'interface', nameGroup: 2 },
      { re: /^\s*(export\s+)?type\s+([A-Za-z_$][\w$]*)\s*[=<]/, kind: 'type', nameGroup: 2 },
      { re: /^\s*(export\s+)?enum\s+([A-Za-z_$][\w$]*)/, kind: 'enum', nameGroup: 2 },
    ],
  },
  {
    extensions: ['py'],
    patterns: [
      { re: /^\s*def\s+([A-Za-z_][\w]*)/, kind: 'function', nameGroup: 1 },
      { re: /^\s*class\s+([A-Za-z_][\w]*)/, kind: 'class', nameGroup: 1 },
    ],
  },
  {
    extensions: ['rs'],
    patterns: [
      { re: /^\s*(pub\s+)?(const\s+)?(unsafe\s+)?(async\s+)?fn\s+([A-Za-z_][\w]*)/, kind: 'function', nameGroup: 5 },
      { re: /^\s*(pub\s+)?struct\s+([A-Za-z_][\w]*)/, kind: 'struct', nameGroup: 2 },
      { re: /^\s*(pub\s+)?enum\s+([A-Za-z_][\w]*)/, kind: 'enum', nameGroup: 2 },
      { re: /^\s*(pub\s+)?trait\s+([A-Za-z_][\w]*)/, kind: 'trait', nameGroup: 2 },
    ],
  },
  {
    extensions: ['go'],
    patterns: [
      { re: /^\s*func\s+([A-Za-z_][\w]*)\s*\(/, kind: 'function', nameGroup: 1 },
      { re: /^\s*func\s*\([^)]*\)\s*([A-Za-z_][\w]*)\s*\(/, kind: 'method', nameGroup: 1 },
      { re: /^\s*type\s+([A-Za-z_][\w]*)\s+(struct|interface)\b/, kind: 'type', nameGroup: 1 },
    ],
  },
  {
    extensions: ['java'],
    patterns: [
      { re: /^\s*(public|private|protected)?\s*(static\s+)?(final\s+)?(abstract\s+)?(class|interface|enum|record)\s+([A-Za-z_][\w]*)/, kind: 'class', nameGroup: 6 },
    ],
  },
  {
    extensions: ['cs'],
    patterns: [
      { re: /^\s*(public|internal|private|protected)?\s*(static\s+)?(abstract\s+|sealed\s+)?(partial\s+)?(class|interface|enum|record|struct)\s+([A-Za-z_][\w]*)/, kind: 'class', nameGroup: 6 },
    ],
  },
]

interface WorkspaceIndex {
  builtAt: number
  byName: Map<string, SymbolEntry[]>
  filesScanned: number
}

const indexCache = new Map<string, WorkspaceIndex>()

function patternsForFile(filename: string): LanguagePattern['patterns'] | null {
  const ext = filename.includes('.') ? filename.slice(filename.lastIndexOf('.') + 1).toLowerCase() : ''
  for (const language of LANGUAGES) {
    if (language.extensions.includes(ext)) return language.patterns
  }
  return null
}

/** 逐文件抽定义行；一条定义都不产出的文件多半不值得再看第二眼。 */
function extractSymbols(content: string, patterns: LanguagePattern['patterns']): Array<{ line: number; name: string; kind: string; signature: string }> {
  const lines = content.split('\n')
  const found: Array<{ line: number; name: string; kind: string; signature: string }> = []
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!
    if (line.length > 300) continue
    for (const pattern of patterns) {
      const match = line.match(pattern.re)
      if (match) {
        found.push({
          line: i + 1,
          name: match[pattern.nameGroup]!,
          kind: pattern.kind,
          signature: line.trim().slice(0, 140),
        })
        break
      }
    }
  }
  return found
}

export async function getWorkspaceSymbolIndex(workspace: string, refresh = false): Promise<WorkspaceIndex> {
  const cached = indexCache.get(workspace)
  if (!refresh && cached && Date.now() - cached.builtAt < INDEX_TTL_MS) return cached

  const rootDir = resolveRealWorkspace(workspace)
  const byName = new Map<string, SymbolEntry[]>()
  let filesScanned = 0

  const walk = async (dir: string, depth: number): Promise<void> => {
    if (depth > 8 || filesScanned >= MAX_FILES) return
    let dirents
    try {
      dirents = await readdir(dir, { withFileTypes: true })
    } catch {
      return
    }
    for (const dirent of dirents) {
      if (filesScanned >= MAX_FILES) return
      if (dirent.name.startsWith('.') && dirent.name !== '.github') continue
      const absolute = join(dir, dirent.name)
      if (dirent.isDirectory() || dirent.isSymbolicLink()) {
        // 与其余工具同一套规矩：指到工作区外的链接整枝跳过
        if (!isPathInsideWorkspace(rootDir, absolute)) continue
      }
      if (dirent.isDirectory()) {
        if (SKIP_DIRS.has(dirent.name)) continue
        await walk(absolute, depth + 1)
        continue
      }

      const patterns = patternsForFile(dirent.name)
      if (!patterns) continue
      try {
        const info = await stat(absolute)
        if (info.size > MAX_FILE_BYTES) continue
        const buffer = await readFile(absolute)
        if (buffer.subarray(0, 2048).includes(0)) continue
        filesScanned += 1

        const rel = relative(rootDir, absolute).replace(/\\/g, '/')
        for (const symbol of extractSymbols(buffer.toString('utf-8'), patterns)) {
          const entry: SymbolEntry = { ...symbol, file: rel }
          const list = byName.get(symbol.name)
          if (list) list.push(entry)
          else byName.set(symbol.name, [entry])
        }
      } catch {
        continue
      }
    }
  }

  await walk(rootDir, 1)
  const fresh: WorkspaceIndex = { builtAt: Date.now(), byName, filesScanned }
  indexCache.set(workspace, fresh)
  return fresh
}

export function createFindSymbolTool(workspace: string): AgentTool<{ query: string; kind?: string; refresh?: boolean; exact?: boolean }> {
  return {
    name: 'find_symbol',
    label: '查找符号',
    description:
      '在工作区里按名字查找函数 / 类 / 结构体 / 接口等定义（支持 TS/JS、Python、Rust、Go、Java、C#），返回文件、行号与签名。找定义用它，比正则全文搜索更准。',
    parameters: {
      type: 'object',
      properties: {
        query: { type: 'string', description: '符号名（或名字的一部分，默认不区分大小写）。' },
        kind: { type: 'string', description: '可选，按种类过滤：function / class / interface / type / enum / struct / trait / method。' },
        exact: { type: 'boolean', description: '可选，精确匹配符号名（默认false，子串匹配）。' },
        refresh: { type: 'boolean', description: '可选，强制重建索引（默认索引缓存 60 秒）。' },
      },
      required: ['query'],
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      const query = String(args.query ?? '').trim()
      if (!query) return { ok: false, output: '缺少 query 参数。' }
      const kind = typeof args.kind === 'string' && args.kind ? args.kind.toLowerCase() : null
      const exact = Boolean(args.exact)

      const index = await getWorkspaceSymbolIndex(workspace, Boolean(args.refresh))
      const lower = query.toLowerCase()
      const hits: SymbolEntry[] = []
      for (const [name, entries] of index.byName) {
        if (exact ? name === query : name.toLowerCase().includes(lower)) {
          for (const entry of entries) {
            if (kind && entry.kind !== kind) continue
            hits.push(entry)
          }
        }
      }

      if (hits.length === 0) {
        return {
          ok: true,
          output: `没有找到符号「${query}」${kind ? `（种类 ${kind}）` : ''}。索引扫描了 ${index.filesScanned} 个文件。`,
          details: { filesScanned: index.filesScanned },
        }
      }

      // 精确命中排前面，其余按文件路径分组，读起来才连贯
      hits.sort((a, b) => {
        const exactA = a.name === query ? 0 : 1
        const exactB = b.name === query ? 0 : 1
        return exactA - exactB || a.file.localeCompare(b.file) || a.line - b.line
      })
      const limited = hits.slice(0, MAX_RESULTS)
      const body = limited
        .map((hit) => `${hit.file}:${hit.line}  [${hit.kind}]  ${hit.signature}`)
        .join('\n')
      const more = hits.length > MAX_RESULTS ? `\n… 还有 ${hits.length - MAX_RESULTS} 条结果未显示` : ''
      const truncated = truncateTail(body, MAX_RESULTS, 25 * 1024)

      return {
        ok: true,
        output: `${truncated.content}${more}`,
        details: { total: hits.length, filesScanned: index.filesScanned },
      }
    },
  }
}
