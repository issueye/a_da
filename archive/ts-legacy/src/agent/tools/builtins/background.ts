/**
 * 后台任务工具 (run_background / check_task / kill_task)
 *
 * dev server、watcher、长监听进程这类「跑起来就不用等的命令」交给后台任务：
 * run_background 立即返回任务 id，输出沉淀在内存环形缓冲里，模型用 check_task
 * 轮询、kill_task 终止（整棵进程树，见 proc.ts）。会话结束应用退出时任务随
 * 进程一起消失——它们本来就是短命的。
 */

import { spawn, type ChildProcess } from 'node:child_process'
import { truncateTail } from '../../core/truncate'
import type { AgentTool, AgentToolResult } from '../../core/types'
import { killProcessTree } from '../proc'
import { checkWorkspaceSandbox } from '../workspace'

/** 单任务输出缓冲上限：超出的旧内容被丢掉，保住最近的部分。 */
const MAX_OUTPUT_BYTES = 200 * 1024
/** 最多同时登记的任务数：防止模型无限开后台。 */
const MAX_TASKS = 20

export type BackgroundTaskStatus = 'running' | 'completed' | 'failed' | 'killed'

export interface BackgroundTask {
  id: string
  command: string
  cwd: string
  startedAt: number
  endedAt?: number
  status: BackgroundTaskStatus
  exitCode: number | null
  output: string
  truncated: boolean
  pid: number | undefined
  proc: ChildProcess
}

let counter = 0
const nextTaskId = () => `bg_${Date.now().toString(36)}_${++counter}`

function statusLabel(status: BackgroundTaskStatus): string {
  switch (status) {
    case 'running':
      return '运行中'
    case 'completed':
      return '已正常退出'
    case 'failed':
      return '已退出（非零）'
    case 'killed':
      return '已被终止'
  }
}

export class BackgroundTaskManager {
  private tasks = new Map<string, BackgroundTask>()

  list(): BackgroundTask[] {
    return [...this.tasks.values()]
  }

  get(id: string): BackgroundTask | undefined {
    return this.tasks.get(id)
  }

  /** 启动一个后台任务并立即返回。 */
  start(workspace: string, command: string, cwd?: string): BackgroundTask {
    // 清理已结束的旧任务，给新任务腾位置
    const finished = this.list().filter((task) => task.status !== 'running')
    while (this.tasks.size >= MAX_TASKS && finished.length > 0) {
      const oldest = finished.shift()!
      this.tasks.delete(oldest.id)
    }

    const runCwd = cwd ? checkWorkspaceSandbox(workspace, cwd) : workspace
    const isWin = process.platform === 'win32'
    const shell = isWin ? process.env.COMSPEC || 'cmd.exe' : '/bin/sh'
    const shellArgs = isWin ? ['/d', '/s', '/c', command] : ['-c', command]

    const task: BackgroundTask = {
      id: nextTaskId(),
      command,
      cwd: runCwd,
      startedAt: Date.now(),
      status: 'running',
      exitCode: null,
      output: '',
      truncated: false,
      pid: undefined,
      proc: null as unknown as ChildProcess,
    }

    const child = spawn(shell, shellArgs, {
      cwd: runCwd,
      env: { ...process.env, A_DA_AGENT: '1' },
      windowsHide: true,
      // POSIX 上让 shell 成为进程组长，终止任务时才能整树清掉
      detached: isWin ? undefined : true,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    task.pid = child.pid
    task.proc = child

    const append = (chunk: Buffer): void => {
      task.output += chunk.toString('utf-8')
      if (task.output.length > MAX_OUTPUT_BYTES) {
        task.output = task.output.slice(task.output.length - MAX_OUTPUT_BYTES)
        task.truncated = true
      }
    }
    child.stdout?.on('data', append)
    child.stderr?.on('data', append)
    child.on('error', (err) => {
      if (task.status === 'running') {
        task.output += `\n启动后台任务失败：${err.message}\n`
        task.status = 'failed'
        task.endedAt = Date.now()
      }
    })
    child.on('close', (code) => {
      // killed / error 已经定过终态，这里只接管自然退出
      if (task.status !== 'running') return
      task.exitCode = code ?? 0
      task.endedAt = Date.now()
      task.status = code === 0 ? 'completed' : 'failed'
    })

    this.tasks.set(task.id, task)
    return task
  }

  /** 终止任务（整棵进程树）。返回是否找到了运行中的任务。 */
  kill(id: string): boolean {
    const task = this.tasks.get(id)
    if (!task) return false
    if (task.status === 'running') {
      killProcessTree({ pid: task.pid })
      task.status = 'killed'
      task.endedAt = Date.now()
    }
    return true
  }

  /** 终止并清空全部任务（应用退出前调用）。 */
  dispose(): void {
    for (const task of this.tasks.values()) {
      if (task.status === 'running') killProcessTree({ pid: task.pid })
    }
    this.tasks.clear()
  }
}

export const defaultBackgroundTasks = new BackgroundTaskManager()

function formatOutput(task: BackgroundTask, lines: number): string {
  const allLines = task.output.split('\n')
  const tail = allLines.slice(Math.max(0, allLines.length - lines)).join('\n')
  const truncated = truncateTail(tail, lines, 30 * 1024)
  const head: string[] = []
  head.push(`任务 ${task.id} ${statusLabel(task.status)}（命令：${task.command}）`)
  const elapsed = Math.round(((task.endedAt ?? Date.now()) - task.startedAt) / 1000)
  head.push(`已持续 ${elapsed}s${task.exitCode !== null ? `，退出码 ${task.exitCode}` : ''}`)
  if (task.truncated) head.push('（输出过长，早先的内容已被丢弃）')
  if (!truncated.content.trim()) head.push('（暂无输出）')
  return `${head.join('\n')}\n\n${truncated.content}`
}

export function createRunBackgroundTool(workspace: string): AgentTool<{ command: string; cwd?: string }> {
  return {
    name: 'run_background',
    label: '后台命令',
    description:
      '在后台启动长运行命令（dev server、watcher、监听进程等），立即返回任务 id，不阻塞对话。用 check_task 查看输出与状态。',
    executionMode: 'sequential',
    parameters: {
      type: 'object',
      properties: {
        command: { type: 'string', description: '要后台执行的 Shell 命令。' },
        cwd: { type: 'string', description: '工作区内的相对子目录（可选）。' },
      },
      required: ['command'],
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      const command = String(args.command ?? '').trim()
      if (!command) return { ok: false, output: '缺少 command 参数。' }
      try {
        const task = defaultBackgroundTasks.start(workspace, command, args.cwd)
        return {
          ok: true,
          output: `后台任务已启动：${task.id}\n命令：${command}\n用 check_task 查看输出，用 kill_task 终止。`,
          details: { taskId: task.id },
        }
      } catch (err) {
        return { ok: false, output: `启动失败：${(err as Error).message}` }
      }
    },
  }
}

export function createCheckTaskTool(): AgentTool<{ task_id?: string; lines?: number }> {
  return {
    name: 'check_task',
    label: '查看后台任务',
    description: '查看后台任务的状态与输出（带 task_id 查单个，不带则列出全部任务）。用于判断 dev server 是否就绪、测试是否跑完。',
    parameters: {
      type: 'object',
      properties: {
        task_id: { type: 'string', description: '后台任务 id（可选，缺省列出全部）。' },
        lines: { type: 'string', description: '返回末尾多少行输出（可选，默认 60）。' },
      },
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      const lines = Math.min(Math.max(Number(args.lines ?? 60) || 60, 5), 400)
      const taskId = typeof args.task_id === 'string' ? args.task_id.trim() : ''

      if (!taskId) {
        const tasks = defaultBackgroundTasks.list()
        if (tasks.length === 0) return { ok: true, output: '当前没有后台任务。' }
        const summary = tasks
          .map((task) => {
            const elapsed = Math.round(((task.endedAt ?? Date.now()) - task.startedAt) / 1000)
            return `${task.id}  ${statusLabel(task.status)}  ${elapsed}s  ${task.command.slice(0, 80)}`
          })
          .join('\n')
        return { ok: true, output: `共 ${tasks.length} 个后台任务：\n${summary}\n\n用带 task_id 的 check_task 查看单个任务的输出。` }
      }

      const task = defaultBackgroundTasks.get(taskId)
      if (!task) return { ok: false, output: `后台任务不存在：${taskId}` }
      return { ok: true, output: formatOutput(task, lines) }
    },
  }
}

export function createKillTaskTool(): AgentTool<{ task_id: string }> {
  return {
    name: 'kill_task',
    label: '停止后台任务',
    description: '终止一个后台任务（连同它的子进程一起）。',
    parameters: {
      type: 'object',
      properties: {
        task_id: { type: 'string', description: '后台任务 id。' },
      },
      required: ['task_id'],
    },
    async execute(_callId, args): Promise<AgentToolResult> {
      const taskId = String(args.task_id ?? '').trim()
      if (!taskId) return { ok: false, output: '缺少 task_id 参数。' }
      const found = defaultBackgroundTasks.get(taskId)
      if (!found) return { ok: false, output: `后台任务不存在：${taskId}` }
      const wasRunning = found.status === 'running'
      defaultBackgroundTasks.kill(taskId)
      return {
        ok: true,
        output: wasRunning ? `已终止后台任务 ${taskId}。` : `任务 ${taskId} 已经结束（${statusLabel(found.status)}），无需终止。`,
      }
    },
  }
}
