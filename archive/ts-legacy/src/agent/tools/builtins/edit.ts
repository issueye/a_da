/**
 * 精确编辑替换工具 (edit_file / edit)
 * 参考 @earendil-works/pi-coding-agent/src/core/tools/edit.ts
 */

import { readFile, writeFile } from 'node:fs/promises'
import { unifiedPatch } from '../../patch'
import type { AgentTool, AgentToolResult } from '../../core/types'
import type { EditToolArgs } from '../types'
import { checkWorkspaceSandbox } from '../workspace'

export function createEditTool(workspace: string): AgentTool<EditToolArgs> {
  return {
    name: 'edit_file',
    label: '精准编辑',
    description: '在文件中用 new_string 精确替换 old_string。old_string 必须在文件中仅出现一次。',
    parameters: {
      type: 'object',
      properties: {
        path: { type: 'string', description: '文件路径（相对于工作区）。' },
        old_string: { type: 'string', description: '待替换的原始文本（必须在文件中唯一）。' },
        new_string: { type: 'string', description: '替换后的新文本。' },
        edits: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              old_string: { type: 'string' },
              new_string: { type: 'string' },
            },
          },
          description: '可选的多处精准替换列表。',
        },
      },
      required: ['path'],
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      try {
        const full = checkWorkspaceSandbox(workspace, args.path)
        let before: string
        try {
          before = await readFile(full, 'utf-8')
        } catch {
          return {
            output: `文件不存在：${args.path}`,
            ok: false,
          }
        }

        const editPairs: Array<{ oldStr: string; newStr: string }> = []
        if (args.edits && args.edits.length > 0) {
          for (const e of args.edits) {
            const o = e.old_string ?? e.oldText ?? ''
            const n = e.new_string ?? e.newText ?? ''
            if (o) editPairs.push({ oldStr: o, newStr: n })
          }
        } else if (args.old_string !== undefined && args.new_string !== undefined) {
          editPairs.push({ oldStr: args.old_string, newStr: args.new_string })
        } else {
          return {
            output: '必须提供 old_string 与 new_string，或提供 edits 替换列表。',
            ok: false,
          }
        }

        let after = before
        for (const pair of editPairs) {
          const firstIndex = after.indexOf(pair.oldStr)
          if (firstIndex === -1) {
            return {
              output: `未能替换：原文本在 ${args.path} 中不存在。请确认代码上下文。`,
              ok: false,
            }
          }
          const secondIndex = after.indexOf(pair.oldStr, firstIndex + pair.oldStr.length)
          if (secondIndex !== -1) {
            return {
              output: `未能替换：原文本在 ${args.path} 中出现了多次，请扩大上下文使其唯一。`,
              ok: false,
            }
          }
          after = after.slice(0, firstIndex) + pair.newStr + after.slice(firstIndex + pair.oldStr.length)
        }

        await writeFile(full, after, 'utf-8')
        const patch = unifiedPatch(args.path, before, after)

        return {
          output: `已成功修改 ${args.path}`,
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
