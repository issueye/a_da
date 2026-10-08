/**
 * 内置辅助 Coding 插件：批量读写提效 (batch-ops)
 *
 * 存在的理由只有一个：**步数**。一次 read_file 只够读一个文件，一次 edit_file 只够
 * 改一处，于是「看 8 个文件再改 3 个」要来回 11 轮模型请求——每一轮都要把整个上下文
 * 重新发一遍，慢且贵。这里把「多文件读」与「多文件改」各压成一次调用，模型一轮就能
 * 拿到/落下一整批改动。
 *
 * 读是纯只读的，没有副作用。写则必须接进既有的安全网：写之前要把这批文件全部快照、
 * 返回的 patch 也要按文件拆开，否则「改动审阅」面板和逐文件回滚就会看不见这批改动。
 * 所以这里的输出契约是：patch 用 `--- a/<path>` 分段拼接（`<diff>` 认这个格式），
 * 每个文件的路径与改动量放进 details.files，由 store 侧的接线读取。
 */

import { readFile, stat, writeFile } from 'node:fs/promises'
import { isAbsolute, join } from 'node:path'
import { unifiedPatch } from '../../patch'
import { checkWorkspaceSandbox } from '../workspace'
import type { PluginDescriptor } from './types'

/** 与 read_file 一致的单文件上限：批量不能让单个大文件把整批拖垮。 */
const MAX_FILE_BYTES = 512 * 1024
const DEFAULT_LIMIT = 400
/** 一次批量读取的总行数上限：够看一圈实现，又不至于灌爆上下文。 */
const MAX_TOTAL_LINES = 2400

interface ReadTarget {
  path: string
  offset?: number
  limit?: number
}

interface EditPair {
  old_string?: string
  new_string?: string
  oldText?: string
  newText?: string
  old?: string
  new?: string
}

interface EditTargetFile {
  path: string
  edits?: EditPair[]
  old_string?: string
  new_string?: string
}

function normalizePair(pair: EditPair): { oldStr: string; newStr: string } | null {
  const oldStr = pair.old_string ?? pair.oldText ?? pair.old ?? ''
  const newStr = pair.new_string ?? pair.newText ?? pair.new ?? ''
  return oldStr ? { oldStr, newStr } : null
}

export const batchOpsPlugin: PluginDescriptor = {
  id: 'batch-ops',
  name: '批量读写提效 (batch-ops)',
  description:
    '把「多文件读取」与「多文件修改」各自压成一次工具调用，显著减少模型往返步数；改动依旧进入检查点与改动审阅，可逐文件回滚。',
  tools: [
    (workspace: string) => ({
      name: 'read_files',
      executionMode: 'parallel' as const,
      description:
        '一次性读取多个文件（最多 12 个），每个文件可单独指定行号范围。需要同时了解多个文件时优先用它替代多次 read_file，可省下数轮模型请求。',
      parameters: {
        type: 'object',
        properties: {
          paths: {
            type: 'array',
            description: '要读取的文件路径列表（相对工作区），最多 12 个。',
            items: { type: 'string' },
          },
          files: {
            type: 'array',
            description: '可选，需要逐文件指定行号范围时改用这个：每个元素形如 { path, offset, limit }。',
            items: {
              type: 'object',
              properties: {
                path: { type: 'string' },
                offset: { type: 'number', description: '起始行号（1 起始，可选）。' },
                limit: { type: 'number', description: '最多读取行数（可选，默认 400）。' },
              },
            },
          },
          limit: { type: 'number', description: '可选，对未单独指定 limit 的文件统一生效的读取行数上限。' },
        },
      },
      async execute(_callId, args: {
        paths?: string[]
        files?: ReadTarget[]
        limit?: number
      }) {
        const targets: ReadTarget[] = []
        for (const p of args?.paths ?? []) {
          const trimmed = String(p ?? '').trim()
          if (trimmed) targets.push({ path: trimmed })
        }
        for (const f of args?.files ?? []) {
          const trimmed = String(f?.path ?? '').trim()
          if (trimmed) targets.push({ path: trimmed, offset: f.offset, limit: f.limit })
        }

        if (targets.length === 0) {
          return { output: '请通过 paths（或 files）提供至少一个待读取的文件路径。', ok: false }
        }
        if (targets.length > 12) {
          return { output: `一次最多读取 12 个文件，当前请求了 ${targets.length} 个。请拆成两批。`, ok: false }
        }

        const sections: string[] = []
        const missing: string[] = []
        let usedLines = 0
        let truncatedNote = ''

        for (const target of targets) {
          let full: string
          try {
            full = checkWorkspaceSandbox(workspace, target.path)
          } catch (err) {
            sections.push(`## ${target.path}\n\n> 拒绝访问：${(err as Error).message}`)
            missing.push(target.path)
            continue
          }

          try {
            const info = await stat(full)
            if (info.isDirectory()) {
              sections.push(`## ${target.path}\n\n> 这是目录，不是文件。请用 list_files。`)
              missing.push(target.path)
              continue
            }
            if (info.size > MAX_FILE_BYTES) {
              sections.push(
                `## ${target.path}\n\n> 文件过大（${Math.round(info.size / 1024)} KB），请用 read_file 配合 offset/limit 分段读取。`
              )
              missing.push(target.path)
              continue
            }

            const buffer = await readFile(full)
            if (buffer.subarray(0, 4096).includes(0)) {
              sections.push(`## ${target.path}\n\n> 这是二进制文件，已跳过。`)
              missing.push(target.path)
              continue
            }

            const lines = buffer.toString('utf8').split(/\r?\n/)
            const offset = Math.max(1, target.offset ?? 1)
            const limit = Math.max(1, target.limit ?? args.limit ?? DEFAULT_LIMIT)
            const remaining = Math.max(0, MAX_TOTAL_LINES - usedLines)
            // 整批行数预算用完就直接说明，而不是静默少给内容
            if (remaining === 0) {
              sections.push(`## ${target.path}\n\n> 本批已达总行数上限（${MAX_TOTAL_LINES} 行），该文件未展开。请单独读取。`)
              continue
            }
            const effectiveLimit = Math.min(limit, remaining)
            const slice = lines.slice(offset - 1, offset - 1 + effectiveLimit)
            usedLines += slice.length

            const numbered = slice.map((line, idx) => `${offset + idx} | ${line}`).join('\n')
            const header =
              slice.length < lines.length
                ? `## ${target.path}  (第 ${offset}-${offset + slice.length - 1} 行 / 共 ${lines.length} 行)`
                : `## ${target.path}  (共 ${lines.length} 行)`
            sections.push(`${header}\n\n\`\`\`\n${numbered}\n\`\`\``)

            if (slice.length < lines.length && effectiveLimit < limit) {
              truncatedNote = `\n\n> 注意：本批总行数已达 ${MAX_TOTAL_LINES} 行上限，后续文件可能未完整展开。`
            }
          } catch (err) {
            sections.push(`## ${target.path}\n\n> 读取失败：${(err as Error).message}`)
            missing.push(target.path)
          }
        }

        const okCount = targets.length - missing.length
        const summary =
          `已批量读取 ${okCount}/${targets.length} 个文件` +
          (missing.length > 0 ? `，未能读取：${missing.join('、')}` : '')

        return {
          output: `${summary}\n\n${sections.join('\n\n')}${truncatedNote}`,
          ok: okCount > 0,
          details: {
            requested: targets.length,
            read: okCount,
            failed: missing,
            totalLines: usedLines,
          },
        }
      },
    }),

    (workspace: string) => ({
      name: 'edit_files',
      executionMode: 'sequential' as const,
      description:
        '一次性对多个文件应用精准替换（最多 10 个文件）。每个文件的 old_string 必须在其中唯一出现。需要改动多个文件时优先用它替代多次 edit_file，可省下数轮模型请求；执行前会为所有涉及文件建立检查点，改动可在改动审阅里逐文件回滚。',
      parameters: {
        type: 'object',
        properties: {
          files: {
            type: 'array',
            description: '待修改的文件列表，每个元素形如 { path, edits: [{ old_string, new_string }] }。',
            items: {
              type: 'object',
              properties: {
                path: { type: 'string', description: '文件路径（相对工作区）。' },
                edits: {
                  type: 'array',
                  description: '该文件内的一处或多处精准替换。',
                  items: {
                    type: 'object',
                    properties: {
                      old_string: { type: 'string' },
                      new_string: { type: 'string' },
                    },
                  },
                },
                old_string: { type: 'string', description: '只改一处时的简写形式。' },
                new_string: { type: 'string', description: '只改一处时的简写形式。' },
              },
            },
          },
        },
        required: ['files'],
      },
      async execute(_callId, args: { files?: EditTargetFile[] }) {
        const targets = (args?.files ?? []).filter((f) => String(f?.path ?? '').trim())
        if (targets.length === 0) {
          return { output: '请通过 files 提供至少一个待修改的文件。', ok: false }
        }
        if (targets.length > 10) {
          return { output: `一次最多修改 10 个文件，当前请求了 ${targets.length} 个。请拆成两批。`, ok: false }
        }

        const applied: Array<{ path: string; patch: string; additions: number; deletions: number }> = []
        const failures: string[] = []
        const patches: string[] = []

        for (const target of targets) {
          const rel = String(target.path).trim()
          const pairs: Array<{ oldStr: string; newStr: string }> = []
          for (const raw of target.edits ?? []) {
            const pair = normalizePair(raw)
            if (pair) pairs.push(pair)
          }
          if (pairs.length === 0) {
            const single = normalizePair(target)
            if (single) pairs.push(single)
          }

          if (pairs.length === 0) {
            failures.push(`${rel}：未提供任何有效的 old_string/new_string 替换对`)
            continue
          }

          let full: string
          try {
            full = checkWorkspaceSandbox(workspace, rel)
          } catch (err) {
            failures.push(`${rel}：${(err as Error).message}`)
            continue
          }

          let before: string
          try {
            before = await readFile(full, 'utf-8')
          } catch {
            failures.push(`${rel}：文件不存在`)
            continue
          }

          let after = before
          let pairError: string | null = null
          for (const pair of pairs) {
            const firstIndex = after.indexOf(pair.oldStr)
            if (firstIndex === -1) {
              pairError = '原文本在文件中不存在，请确认代码上下文'
              break
            }
            const secondIndex = after.indexOf(pair.oldStr, firstIndex + pair.oldStr.length)
            if (secondIndex !== -1) {
              pairError = '原文本在文件中出现了多次，请扩大上下文使其唯一'
              break
            }
            after = after.slice(0, firstIndex) + pair.newStr + after.slice(firstIndex + pair.oldStr.length)
          }

          if (pairError) {
            failures.push(`${rel}：未能替换（${pairError}）`)
            continue
          }

          // 整体判断：一批里任一文件失败就不写这个文件，避免只落一半的中间态
          try {
            await writeFile(full, after, 'utf-8')
          } catch (err) {
            failures.push(`${rel}：写入失败（${(err as Error).message}）`)
            continue
          }

          const patch = unifiedPatch(rel, before, after)
          if (patch) {
            patches.push(patch)
            const added = patch.split('\n').filter((l) => l.startsWith('+') && !l.startsWith('+++')).length
            const removed = patch.split('\n').filter((l) => l.startsWith('-') && !l.startsWith('---')).length
            applied.push({ path: rel, patch, additions: added, deletions: removed })
          }
        }

        const combinedPatch = patches.join('')
        const lines = [
          `## 批量修改结果：成功 ${applied.length} / ${targets.length} 个文件`,
        ]
        for (const file of applied) {
          lines.push(`- \`${file.path}\`  +${file.additions} −${file.deletions}`)
        }
        if (failures.length > 0) {
          lines.push('', '### 未能完成的文件：')
          for (const failure of failures) lines.push(`- ${failure}`)
        }

        return {
          output: lines.join('\n'),
          ok: failures.length === 0 && applied.length > 0,
          patch: combinedPatch || undefined,
          details: {
            files: applied.map((f) => ({
              path: f.path,
              patch: f.patch,
              additions: f.additions,
              deletions: f.deletions,
            })),
            failed: failures,
          },
        }
      },
    }),
  ],
  skills: [
    {
      name: 'batch-efficiency',
      description: '多文件读写批量化，减少模型往返步数的最佳实践。',
      content: `---
name: batch-efficiency
description: 多文件读写批量化，减少模型往返步数的最佳实践。
whenToUse: 当需要同时查看或修改多个文件时使用。
---

# 批量读写提效法则

每一轮工具调用都会把整个上下文重新发一遍，所以「步数」是最贵的东西。两个批量工具
就是为了把步数压下来：

1. **读多个文件用 read_files**：一次最多 12 个文件，可逐文件给行号范围。不要为了看
   三个文件而发三次 read_file；
2. **改多个文件用 edit_files**：一次最多 10 个文件，每处替换仍要保证 old_string 在
   文件内唯一。不要为了改三处而发三次 edit_file；
3. **先批量侦察再批量下手**：用一次 read_files 把相关文件看全，规划好所有替换点，再用
   一次 edit_files 全部落下；
4. **单个文件的一处小改动仍可用 edit_file**：批量不是目的，省步数才是——数量为 1 时
   用哪个都一样。
`,
    },
  ],
  prompts: [
    {
      name: 'batch-refactor',
      description: '先批量读取相关文件，再用一次批量编辑完成多点改造',
      argumentHint: '[目标模块或路径]',
      content: `请针对 \${1:目标模块} 做一次批量改造，按以下步骤执行：
1. 先用 read_files 一次读全所有相关文件，理清调用关系；
2. 规划出所有需要修改的位置，确保每处 old_string 在各自文件内唯一；
3. 用一次 edit_files 把所有替换落下；
4. 汇总本次改动涉及的文件与每个文件的改动量。`,
    },
  ],
}
