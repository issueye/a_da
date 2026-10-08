/**
 * 读文件工具 (read_file / read)
 * 参考 @earendil-works/pi-coding-agent/src/core/tools/read.ts
 */

import { readFile, stat } from 'node:fs/promises'
import { truncateTail } from '../../core/truncate'
import type { AgentTool, AgentToolResult } from '../../core/types'
import type { ReadToolArgs } from '../types'
import { checkWorkspaceSandbox } from '../workspace'

/** 超过这个大小就只允许用 offset/limit 分段读，避免整个文件进内存。 */
const MAX_FILE_BYTES = 512 * 1024
const DEFAULT_LIMIT = 400

/**
 * 读取前的三道门：目录、超大、二进制。
 *
 * 这些必须在 readFile 之前跑完——读完再判断的话，一个几百 MB 的文件已经进来
 * 了，split 成行数组还会再来一份，足以把 UI 线程拖死。
 */
async function readTextFile(absolute: string): Promise<string> {
  const info = await stat(absolute)
  if (info.isDirectory()) throw new Error('这是目录，不是文件')
  if (info.size > MAX_FILE_BYTES) {
    throw new Error(
      `文件过大（${info.size} 字节，上限 ${MAX_FILE_BYTES}），请用 offset/limit 分段读取`
    )
  }
  const buffer = await readFile(absolute)
  if (buffer.subarray(0, 4096).includes(0)) throw new Error('这是二进制文件')
  return buffer.toString('utf8')
}

export function createReadTool(workspace: string): AgentTool<ReadToolArgs> {
  return {
    name: 'read_file',
    label: '读取文件',
    description:
      '读取工作区内的文本文件。支持按行号范围读取 (offset / limit)，超大文件自动截断保护。',
    parameters: {
      type: 'object',
      properties: {
        path: { type: 'string', description: '相对于工作区根目录的文件路径。' },
        offset: { type: 'number', description: '起始行号（1 起始，可选）。' },
        limit: { type: 'number', description: '最多读取行数（可选，默认 400 行）。' },
      },
      required: ['path'],
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      try {
        const full = checkWorkspaceSandbox(workspace, args.path)
        const raw = await readTextFile(full)
        const lines = raw.split(/\r?\n/)

        const offset = Math.max(1, args.offset ?? 1)
        const limit = args.limit !== undefined ? Math.max(1, args.limit) : DEFAULT_LIMIT

        const slice = lines.slice(offset - 1, offset - 1 + limit)
        const numbered = slice.map((line, idx) => `${offset + idx} | ${line}`).join('\n')

        const truncated = truncateTail(numbered, DEFAULT_LIMIT, 30 * 1024)
        return {
          output: truncated.content,
          ok: true,
          details: { lines: slice.length, offset, total: lines.length },
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
