/**
 * 用户可配置钩子 (hooks)
 *
 * Claude Code 式的 hooks：用户在 `~/.a-da/hooks.json`（全局）或
 * `<项目>/.ada/hooks.json`（工作区）里声明「某事件发生时跑这条 shell 命令」，
 * Agent 在对应点位代为执行。
 *
 *   {
 *     "hooks": [
 *       { "event": "before_tool", "tool": "edit_file", "command": "echo 锁文件检查", "timeout": 10 }
 *     ]
 *   }
 *
 * 事件与语义：
 * - before_tool：工具获准执行后、真正执行前。stdin 收到 JSON（tool/args/thread_id/
 *   workspace），**退出码非零 = 拦截这次调用**，stderr 成为回给模型的理由。
 * - after_tool：工具执行完。payload 含 ok/output；返回值不影响结果（跑 prettier
 *   这类自动格式化用它）。
 * - agent_end：一轮结束。payload 含 reason。
 *
 * 匹配规则：`tool` 省略或 "*" 匹配所有工具；否则按逗号分隔的工具名清单精确匹配。
 * 钩子命令继承 A_DA_AGENT=1 环境变量，另外注入 A_DA_HOOK_EVENT / A_DA_HOOK_TOOL。
 */

import { readFile } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { getAppHome } from './home'

export type HookEvent = 'before_tool' | 'after_tool' | 'agent_end'

export interface HookRule {
  event: HookEvent
  /** 工具名过滤：省略或 "*" = 全部；逗号分隔精确匹配多个 */
  tool?: string
  command: string
  /** 超时秒数，默认 10，上限 60 */
  timeout?: number
}

export interface HooksConfig {
  hooks: HookRule[]
}

export interface HookRunResult {
  /** before_tool：是否拦截 */
  blocked?: boolean
  reason?: string
  /** 执行过的钩子与其退出码（调试日志用） */
  runs: Array<{ rule: HookRule; exitCode: number | null; timedOut: boolean; stderr: string }>
}

const HOOK_EVENTS: Set<string> = new Set(['before_tool', 'after_tool', 'agent_end'])
const DEFAULT_TIMEOUT_S = 10
const MAX_TIMEOUT_S = 60

/** 解析并规范化 hooks 配置：坏条目跳过而不是整个文件作废。 */
export function parseHooksConfig(text: string, source: string): { rules: HookRule[]; warnings: string[] } {
  const warnings: string[] = []
  const rules: HookRule[] = []
  if (!text.trim()) return { rules, warnings }

  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch (err) {
    warnings.push(`${source} 不是合法 JSON：${(err as Error).message}`)
    return { rules, warnings }
  }

  const hooks = (parsed as { hooks?: unknown })?.hooks
  if (!Array.isArray(hooks)) {
    warnings.push(`${source} 缺少 hooks 数组`)
    return { rules, warnings }
  }

  for (let i = 0; i < hooks.length; i++) {
    const raw = hooks[i] as Record<string, unknown>
    const event = typeof raw?.event === 'string' ? raw.event : ''
    const command = typeof raw?.command === 'string' ? raw.command.trim() : ''
    if (!HOOK_EVENTS.has(event)) {
      warnings.push(`${source} 第 ${i + 1} 条：event 必须是 before_tool / after_tool / agent_end`)
      continue
    }
    if (!command) {
      warnings.push(`${source} 第 ${i + 1} 条：command 不能为空`)
      continue
    }
    const tool = typeof raw?.tool === 'string' && raw.tool.trim() ? raw.tool.trim() : undefined
    const timeout = Number(raw?.timeout)
    rules.push({
      event: event as HookEvent,
      tool: tool === '*' ? undefined : tool,
      command,
      timeout: Number.isFinite(timeout) && timeout > 0 ? Math.min(timeout, MAX_TIMEOUT_S) : undefined,
    })
  }
  return { rules, warnings }
}

function ruleMatches(rule: HookRule, event: HookEvent, toolName: string): boolean {
  if (rule.event !== event) return false
  if (!rule.tool) return true
  return rule.tool
    .split(',')
    .map((name) => name.trim())
    .filter(Boolean)
    .includes(toolName)
}

export class HookManager {
  /** 读取全局与工作区两处配置并合并（全局在前）。 */
  async load(workspace: string): Promise<{ rules: HookRule[]; warnings: string[] }> {
    const files = [join(getAppHome(), 'hooks.json'), join(workspace, '.ada', 'hooks.json')]
    const rules: HookRule[] = []
    const warnings: string[] = []
    for (const file of files) {
      if (!existsSync(file)) continue
      try {
        const text = await readFile(file, 'utf-8')
        const parsed = parseHooksConfig(text, file)
        rules.push(...parsed.rules)
        warnings.push(...parsed.warnings)
      } catch (err) {
        warnings.push(`读取 ${file} 失败：${(err as Error).message}`)
      }
    }
    return { rules, warnings }
  }

  /** 执行匹配的钩子命令：stdin 送 JSON payload，stdout/stderr 记入结果。 */
  async run(
    workspace: string,
    event: HookEvent,
    toolName: string | null,
    payload: Record<string, unknown>,
    warnings: string[] = []
  ): Promise<HookRunResult> {
    const { rules } = await this.load(workspace)
    const matched = rules.filter((rule) => ruleMatches(rule, event, toolName ?? ''))
    const runs: HookRunResult['runs'] = []
    const result: HookRunResult = { runs }

    for (const rule of matched) {
      const outcome = await this.execute(rule, event, toolName, payload, workspace, warnings)
      runs.push(outcome)
      if (event === 'before_tool' && (outcome.timedOut || (outcome.exitCode !== null && outcome.exitCode !== 0))) {
        result.blocked = true
        const reason = outcome.stderr.trim()
        result.reason = reason || `钩子命令以退出码 ${outcome.exitCode} 拦截了这次调用：${rule.command}`
        // 拦截语义下立即停：后面同事件的钩子不再跑
        break
      }
    }
    return result
  }

  private execute(
    rule: HookRule,
    event: HookEvent,
    toolName: string | null,
    payload: Record<string, unknown>,
    workspace: string,
    warnings: string[]
  ): Promise<HookRunResult['runs'][number]> {
    const timeoutS = rule.timeout ?? DEFAULT_TIMEOUT_S
    const isWin = process.platform === 'win32'
    const shell = isWin ? process.env.COMSPEC || 'cmd.exe' : '/bin/sh'
    const shellArgs = isWin ? ['/d', '/s', '/c', rule.command] : ['-c', rule.command]

    return new Promise((resolve) => {
      let stderr = ''
      let timedOut = false
      let settled = false

      let child
      try {
        child = spawn(shell, shellArgs, {
          cwd: workspace,
          env: {
            ...process.env,
            A_DA_AGENT: '1',
            A_DA_HOOK_EVENT: event,
            ...(toolName ? { A_DA_HOOK_TOOL: toolName } : {}),
          },
          windowsHide: true,
          detached: isWin ? undefined : true,
        })
      } catch (err) {
        warnings.push(`钩子启动失败（${rule.command}）：${(err as Error).message}`)
        resolve({ rule, exitCode: null, timedOut: false, stderr: (err as Error).message })
        return
      }

      const timer = setTimeout(() => {
        timedOut = true
        try {
          if (child.pid) {
            if (isWin) {
              spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' }).on('error', () => {})
            } else {
              try {
                process.kill(-child.pid, 'SIGKILL')
              } catch {}
            }
          }
        } catch {}
      }, timeoutS * 1000)

      const finish = (exitCode: number | null): void => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        resolve({ rule, exitCode, timedOut, stderr })
      }

      child.stderr?.on('data', (chunk: Buffer) => {
        stderr += chunk.toString('utf-8')
      })
      child.on('error', (err) => {
        warnings.push(`钩子执行失败（${rule.command}）：${err.message}`)
        finish(null)
      })
      child.on('close', (code) => finish(code))

      child.stdin?.write(JSON.stringify({ event, tool: toolName, ...payload }))
      child.stdin?.end()
    })
  }
}

export const defaultHooks = new HookManager()
