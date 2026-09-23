/**
 * 内置辅助 Coding 插件：测试精准执行与失败归因 (test-runner)
 */

import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { exec } from 'node:child_process'
import { promisify } from 'node:util'
import type { BuiltinPluginPackage } from './types'

const execAsync = promisify(exec)

/** 智能精炼提取测试输出中的失败用例与断言 */
function extractFailures(output: string): string {
  const lines = output.split('\n')
  const failureLines: string[] = []
  let inFailureBlock = false

  for (const line of lines) {
    const isFailHeader =
      line.includes('(fail)') ||
      line.includes('FAIL') ||
      line.includes('FAILED') ||
      line.includes('error:') ||
      line.includes('AssertionError') ||
      line.includes('Expect:') ||
      line.includes('Expected:') ||
      line.includes('Received:') ||
      line.includes('panicked at')

    if (isFailHeader) {
      inFailureBlock = true
    }

    if (inFailureBlock) {
      failureLines.push(line)
      // 如果连续多行普通缩进结束或者遇到下一个通过项，稍后重置
      if (line.includes('(pass)') || line.includes('PASS')) {
        inFailureBlock = false
      }
    }
  }

  if (failureLines.length > 0) {
    return failureLines.slice(0, 150).join('\n')
  }

  // 兜底返回尾部 60 行
  return lines.slice(-60).join('\n')
}

export const testRunnerPlugin: BuiltinPluginPackage = {
  id: 'test-runner',
  name: '测试执行与失败精准归因 (test-runner)',
  description: '自动探测并执行项目单元测试，智能过滤杂质日志，精准抓取失败用例断言与错误堆栈。',
  tools: [
    (workspace: string) => ({
      name: 'run_test_focused',
      description: '运行项目自动化测试并精准提取失败结果。剥离大量通过日志，只提取失败的测试用例、断言差异与错误堆栈。',
      parameters: {
        type: 'object',
        properties: {
          pattern: {
            type: 'string',
            description: '可选，指定过滤运行的测试文件或用例名称（例如 template.test.ts）。',
          },
          command: {
            type: 'string',
            description: '可选，自定义测试命令。如果不传，将自动识别 bun test 或 cargo test。',
          },
        },
      },
      async execute(_callId, args: { pattern?: string; command?: string }) {
        const root = workspace || process.cwd()

        let cmd = args?.command?.trim()
        if (!cmd) {
          if (existsSync(join(root, 'package.json'))) {
            cmd = args?.pattern ? `bun test ${args.pattern.trim()}` : `bun test`
          } else if (existsSync(join(root, 'Cargo.toml'))) {
            cmd = args?.pattern ? `cargo test ${args.pattern.trim()}` : `cargo test`
          } else {
            cmd = args?.pattern ? `npm test -- ${args.pattern.trim()}` : `npm test`
          }
        }

        try {
          const { stdout, stderr } = await execAsync(cmd, {
            cwd: root,
            maxBuffer: 4 * 1024 * 1024,
            windowsHide: true,
          })

          const fullOutput = (stdout + '\n' + stderr).trim()
          return {
            output: `## 测试执行成功 (Exit 0)\n命令: \`${cmd}\`\n\n\`\`\`text\n${fullOutput.slice(-1500)}\n\`\`\``,
            ok: true,
          }
        } catch (err: any) {
          const stdout = err.stdout || ''
          const stderr = err.stderr || ''
          const combined = (stdout + '\n' + stderr).trim() || err.message || ''
          const refinedFailures = extractFailures(combined)

          return {
            output: `## ⚠️ 测试运行失败 (Exit Code ${err.code ?? 1})\n执行命令: \`${cmd}\`\n\n### 提取的失败详情与断言堆栈：\n\`\`\`text\n${refinedFailures}\n\`\`\`\n\n请针对以上失败的测试用例与断言差异进行针对性修复。`,
            ok: false,
          }
        }
      },
    }),
  ],
  skills: [
    {
      name: 'tdd-workflow',
      description: '测试驱动开发与快速红绿重构最佳实践。',
      content: `---
name: tdd-workflow
description: 测试驱动开发与快速红绿重构最佳实践。
whenToUse: 当编写新功能、修复复现 bug 或进行重构时使用。
---

# 敏捷测试驱动开发法则

1. **红灯复现 (Red)**：先写出能稳定复现问题的测试用例，运行确认失败；
2. **极速切入 (Green)**：编写最精简的核心实现代码，调用 \`run_test_focused\` 验证通过；
3. **安全重构 (Refactor)**：在绿灯保护下清理坏味道，优化性能与架构。
`,
    },
  ],
  prompts: [
    {
      name: 'fix-test',
      description: '执行测试并根据失败信息快速修复代码缺陷',
      argumentHint: '[pattern]',
      content: `请调用 run_test_focused 工具运行\${1:+ $1 }测试，提取失败用例与报错堆栈，并按以下步骤执行修复：
1. 深入分析导致断言失败的根本原因；
2. 给出定位到具体源码行号的修复方案；
3. 执行代码修复并重新运行测试，确保 100% 绿灯。`,
    },
  ],
}
