/**
 * 内置辅助 Coding 插件：Git 深度协作与变更洞察 (git-tools)
 */

import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import type { BuiltinPluginPackage } from './types'

const execFileAsync = promisify(execFile)

/** 安全执行 git 命令辅助函数 */
async function runGit(args: string[], cwd: string): Promise<{ stdout: string; stderr: string; ok: boolean }> {
  try {
    const { stdout, stderr } = await execFileAsync('git', args, {
      cwd,
      maxBuffer: 4 * 1024 * 1024,
      windowsHide: true,
    })
    return { stdout: stdout.trim(), stderr: stderr.trim(), ok: true }
  } catch (err: any) {
    return {
      stdout: err.stdout?.trim() || '',
      stderr: err.stderr?.trim() || err.message || '',
      ok: false,
    }
  }
}

export const gitToolsPlugin: BuiltinPluginPackage = {
  id: 'git-tools',
  name: 'Git 变更与协作工具 (git-tools)',
  description: '提供结构化 Git 状态、安全限长 Diff 提取与近期提交历史检索能力，辅助精准掌握版本改动。',
  tools: [
    (workspace: string) => ({
      name: 'git_status',
      description: '获取当前 Git 工作区的状态信息，包含当前分支、暂存修改（Staged）、未暂存修改（Modified）与未跟踪文件（Untracked）。',
      parameters: {
        type: 'object',
        properties: {},
      },
      async execute() {
        const cwd = workspace || process.cwd()
        const res = await runGit(['status', '--porcelain=v1', '-b'], cwd)
        if (!res.ok) {
          if (res.stderr.includes('not a git repository')) {
            return { output: '当前目录不是一个有效的 Git 仓库。', ok: false }
          }
          return { output: `执行 git status 失败: ${res.stderr}`, ok: false }
        }

        const lines = res.stdout.split('\n')
        const branchLine = lines[0] || ''
        const fileLines = lines.slice(1)

        const staged: string[] = []
        const unstaged: string[] = []
        const untracked: string[] = []

        for (const line of fileLines) {
          if (!line.trim()) continue
          const indexStatus = line[0]
          const worktreeStatus = line[1]
          const filePath = line.slice(3).trim()

          if (indexStatus === '?' && worktreeStatus === '?') {
            untracked.push(filePath)
          } else {
            if (indexStatus && indexStatus !== ' ' && indexStatus !== '?') {
              staged.push(`${indexStatus} ${filePath}`)
            }
            if (worktreeStatus && worktreeStatus !== ' ' && worktreeStatus !== '?') {
              unstaged.push(`${worktreeStatus} ${filePath}`)
            }
          }
        }

        const sections = [
          `## Git 状态概要\n- **分支信息**: ${branchLine.replace(/^##\s*/, '')}`,
          `- **暂存区改动 (Staged)**: ${staged.length} 个文件`,
          staged.length > 0 ? staged.map((f) => `  - ${f}`).join('\n') : '  (无)',
          `- **工作区改动 (Unstaged)**: ${unstaged.length} 个文件`,
          unstaged.length > 0 ? unstaged.map((f) => `  - ${f}`).join('\n') : '  (无)',
          `- **未追踪文件 (Untracked)**: ${untracked.length} 个文件`,
          untracked.length > 0 ? untracked.slice(0, 30).map((f) => `  - ${f}`).join('\n') : '  (无)',
        ]

        if (untracked.length > 30) {
          sections.push(`  ... 另有 ${untracked.length - 30} 个未追踪文件省略`)
        }

        return { output: sections.join('\n'), ok: true }
      },
    }),
    (workspace: string) => ({
      name: 'git_diff',
      description: '提取工作区或暂存区的 Git Diff 差异对比，支持按单个文件查看，自动防御输出超长。',
      parameters: {
        type: 'object',
        properties: {
          file: {
            type: 'string',
            description: '可选，限定查看差异的具体相对文件路径（如 src/main.ts）。为空时查看所有文件改动。',
          },
          staged: {
            type: 'boolean',
            description: '是否查看已暂存（git add）的差异。默认为 false（即查看未暂存的工作区改动）。',
          },
          maxLines: {
            type: 'number',
            description: '最大输出行数，避免大量差异挤爆上下文。默认 250 行。',
          },
        },
      },
      async execute(_callId, args: { file?: string; staged?: boolean; maxLines?: number }) {
        const cwd = workspace || process.cwd()
        const gitArgs = ['diff']
        if (args.staged) gitArgs.push('--staged')
        if (args.file) {
          gitArgs.push('--', args.file.trim())
        }

        const res = await runGit(gitArgs, cwd)
        if (!res.ok) {
          return { output: `提取 git diff 失败: ${res.stderr}`, ok: false }
        }

        if (!res.stdout) {
          return {
            output: `未检测到${args.staged ? '暂存区' : '工作区'}${args.file ? `针对 [${args.file}] ` : ''}的任何代码差异。`,
            ok: true,
          }
        }

        const maxLines = Math.max(20, args.maxLines || 250)
        const lines = res.stdout.split('\n')
        if (lines.length > maxLines) {
          const truncated = lines.slice(0, maxLines).join('\n')
          return {
            output: `${truncated}\n\n[提示: Diff 输出共 ${lines.length} 行，已截断显示前 ${maxLines} 行。可传入 "file" 参数缩小对比范围]`,
            ok: true,
          }
        }

        return { output: res.stdout, ok: true }
      },
    }),
    (workspace: string) => ({
      name: 'git_log',
      description: '获取当前分支最近的提交历史（Commit 简要列表），辅助了解代码变更脉络。',
      parameters: {
        type: 'object',
        properties: {
          count: {
            type: 'number',
            description: '获取的提交数量，默认 10 条。',
          },
        },
      },
      async execute(_callId, args: { count?: number }) {
        const cwd = workspace || process.cwd()
        const count = Math.min(50, Math.max(1, args.count || 10))
        const res = await runGit(['log', `-n`, String(count), `--pretty=format:%h | %an | %ar | %s`], cwd)

        if (!res.ok) {
          return { output: `获取 git log 失败: ${res.stderr}`, ok: false }
        }

        if (!res.stdout) {
          return { output: '当前分支暂无任何提交记录。', ok: true }
        }

        return {
          output: `## 最近 ${count} 次 Git 提交历史\n` + res.stdout,
          ok: true,
        }
      },
    }),
  ],
  skills: [
    {
      name: 'git-workflow',
      description: '遵循规范的 Git 敏捷分支管理、原子提交与审查工作流规范。',
      content: `---
name: git-workflow
description: 遵循规范的 Git 敏捷分支管理、原子提交与审查工作流规范。
whenToUse: 当进行版本提交、分支切换、冲突处理或改动审查时使用。
---

# 敏捷 Git 工作流最佳实践

1. **原子提交**：每次提交只做一件事，保持改动的独立与完整；
2. **提交前走查**：使用 \`git_status\` 和 \`git_diff\` 检查暂存区，确认无误后再提交；
3. **编写规范提交说明**：遵循 Conventional Commits 规范，清晰阐明修改动机。
`,
    },
  ],
  prompts: [
    {
      name: 'git-diff-summary',
      description: '提取并分析当前工作区的所有未提交改动，生成结构化摘要',
      argumentHint: '[file]',
      content: `请调用 git_status 和 git_diff 工具查看当前工作区\${1:+中的 $1}代码改动，并从以下维度进行总结：
1. **改动概述**：核心修改了哪些模块，实现了什么功能或修复了什么问题；
2. **主要文件列表**：列出每个受影响的文件及改动要点；
3. **潜在风险排查**：是否引入了未使用的变量、未处理的边界或破坏性变更。`,
    },
  ],
}
