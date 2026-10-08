/**
 * 终端命令执行工具 (run_command / bash)
 * 参考 @earendil-works/pi-coding-agent/src/core/tools/bash.ts
 */

import { spawn } from 'node:child_process'
import { truncateTail } from '../../core/truncate'
import type { AgentTool, AgentToolResult } from '../../core/types'
import type { BashToolArgs } from '../types'
import { killProcessTree } from '../proc'
import { checkWorkspaceSandbox } from '../workspace'

/** 默认两分钟：构建、测试、安装依赖都不该被一刀切掉。 */
const DEFAULT_TIMEOUT_S = 120
/** 模型可以把超时调长，但只能到这个上限。 */
const MAX_TIMEOUT_S = 600

export function createBashTool(workspace: string): AgentTool<BashToolArgs> {
  return {
    name: 'run_command',
    label: '执行命令',
    description: '在工作区内执行 Shell 命令。输出受截断保护。',
    executionMode: 'sequential',
    parameters: {
      type: 'object',
      properties: {
        command: { type: 'string', description: '要执行的 Shell 命令。' },
        cwd: { type: 'string', description: '工作区内的相对子目录（可选）。' },
        timeout: {
          type: 'number',
          description: `超时时间（秒，可选，默认 ${DEFAULT_TIMEOUT_S}s，上限 ${MAX_TIMEOUT_S}s）。`,
        },
      },
      required: ['command'],
    },
    async execute(_callId, args, signal, onUpdate): Promise<AgentToolResult> {
      try {
        let runCwd = workspace
        if (args.cwd) {
          runCwd = checkWorkspaceSandbox(workspace, args.cwd)
        }

        const isWin = process.platform === 'win32'
        const shell = isWin ? process.env.COMSPEC || 'cmd.exe' : '/bin/sh'
        // /d 关掉 AutoRun 注册表项，/s 固定引号处理规则——两条都是让命令按字面执行。
        const shellArgs = isWin ? ['/d', '/s', '/c', args.command] : ['-c', args.command]
        // POSIX 上让 shell 成为进程组长，超时/中止时才能连子进程一起清掉。
        const posixDetached = !isWin

        const requested = args.timeout ?? DEFAULT_TIMEOUT_S
        const timeoutS = Math.min(Math.max(requested, 1), MAX_TIMEOUT_S)
        const timeoutMs = timeoutS * 1000

        return await new Promise<AgentToolResult>((resolveResult) => {
          let stdoutText = ''
          let stderrText = ''
          let killed = false

          const child = spawn(shell, shellArgs, {
            cwd: runCwd,
            // A_DA_AGENT 让工作区里的脚本能识别「这次是被 Agent 调起的」。
            env: { ...process.env, A_DA_AGENT: '1' },
            windowsHide: true,
            detached: posixDetached || undefined,
          })

          const finish = (result: AgentToolResult): void => {
            clearTimeout(timer)
            signal?.removeEventListener('abort', onAbort)
            resolveResult(result)
          }

          /** 只杀 shell 会留下孤儿（构建脚本、dev server 照样活着），必须整棵清。 */
          const killTree = (): void => {
            killed = true
            killProcessTree({ pid: child.pid })
          }

          const timer = setTimeout(() => {
            killTree()
            finish({ output: `命令执行超时（超过 ${timeoutS} 秒）`, ok: false })
          }, timeoutMs)

          const onAbort = (): void => {
            killTree()
            finish({ output: '用户中止了命令执行。', ok: false })
          }

          signal?.addEventListener('abort', onAbort)

          // 内存保护：单任务流式输出最大累积 1MB，防止子进程海量日志撑爆堆内存
          const MAX_BUFFER_CHARS = 1024 * 1024

          const appendWithLimit = (current: string, chunkStr: string): string => {
            const next = current + chunkStr
            if (next.length <= MAX_BUFFER_CHARS) return next
            const head = next.slice(0, 64 * 1024)
            const tail = next.slice(-(MAX_BUFFER_CHARS - 64 * 1024 - 100))
            return `${head}\n...[输出过长，已截断中间日志]...\n${tail}`
          }

          child.stdout?.on('data', (chunk: Buffer) => {
            stdoutText = appendWithLimit(stdoutText, chunk.toString('utf-8'))
            if (onUpdate) {
              const truncated = truncateTail(stdoutText, 300, 20 * 1024)
              onUpdate({ output: truncated.content, ok: true })
            }
          })

          child.stderr?.on('data', (chunk: Buffer) => {
            stderrText = appendWithLimit(stderrText, chunk.toString('utf-8'))
          })

          child.on('error', (err) => {
            if (!killed) {
              finish({ output: `启动命令失败：${err.message}`, ok: false })
            }
          })

          child.on('close', (code) => {
            if (killed) return

            const parts: string[] = []
            if (stdoutText.trim()) parts.push(stdoutText.trim())
            if (stderrText.trim()) parts.push(`stderr:\n${stderrText.trim()}`)
            parts.push(`退出码 ${code ?? 0}`)

            const combined = parts.join('\n\n')
            const truncated = truncateTail(combined, 500, 25 * 1024)

            finish({
              output: truncated.content,
              ok: code === 0,
              details: { exitCode: code },
            })
          })
        })
      } catch (err) {
        return {
          output: (err as Error).message,
          ok: false,
        }
      }
    },
  }
}
