/**
 * 写文件工具 (write_file / write)
 * 参考 @earendil-works/pi-coding-agent/src/core/tools/write.ts
 */

import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname } from 'node:path'
import { unifiedPatch } from '../../patch'
import type { AgentTool, AgentToolResult } from '../../core/types'
import type { WriteToolArgs } from '../types'
import { checkWorkspaceSandbox } from '../workspace'

export function createWriteTool(workspace: string): AgentTool<WriteToolArgs> {
  return {
    name: 'write_file',
    label: '写入文件',
    description: '创建或覆写整个文件。若只需修改部分代码，优先使用 edit_file。',
    parameters: {
      type: 'object',
      properties: {
        path: { type: 'string', description: '文件路径（相对于工作区）。' },
        content: { type: 'string', description: '文件的完整新内容。' },
      },
      required: ['path', 'content'],
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      try {
        const full = checkWorkspaceSandbox(workspace, args.path)
        let before = ''
        try {
          before = await readFile(full, 'utf-8')
        } catch {
          before = ''
        }

        await mkdir(dirname(full), { recursive: true })
        await writeFile(full, args.content, 'utf-8')

        const patch = unifiedPatch(args.path, before, args.content)
        const lineCount = args.content.split('\n').length
        return {
          output: `已写入 ${args.path}（${lineCount} 行）`,
          ok: true,
          patch,
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
