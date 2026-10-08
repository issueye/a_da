/**
 * 跨平台启动前置初始化模块
 * 必须作为应用的首个静态导入引入，在加载任何原生 GUI 渲染器之前就绪。
 */

import { appendFileSync, mkdirSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { ensureValidStdHandles, hasConsoleWindow } from './win32'

// 1. 在 Windows 环境下，确保底层 stdio 句柄指向合法管道（CreatePipe），
// 杜绝 NUL 字符设备导致 Bun/libuv uv_pipe_open 报 invalid_fd 触发 panic 闪退拉起 powershell，
// 并避免 Rust 原生层向 stderr 输出日志时因 NULL/INVALID_HANDLE 引发 panic (os error 6)。
try {
  ensureValidStdHandles()
} catch {}

// 2. 劫持并安全化全局 console 输出：
// 在 Windows 纯 GUI 模式（PE Subsystem 2 无控制台黑框）下，
// Bun 的内置 console 方法会尝试调用 Bun.stdout.writer() / PipeWriter。
// 通过在此重定向 console，将所有输出安全落地到 ~/.a-da/app_debug.log，彻底杜绝闪退。
const appHome = process.env.A_DA_HOME || join(homedir(), '.a-da')
try {
  mkdirSync(appHome, { recursive: true })
} catch {}
const logFile = join(appHome, 'app_debug.log')

function writeLog(level: string, ...args: any[]) {
  try {
    const line = args
      .map((arg) => {
        if (typeof arg === 'string') return arg
        if (arg instanceof Error) return arg.stack || `${arg.name}: ${arg.message}`
        try {
          return JSON.stringify(arg)
        } catch {
          return String(arg)
        }
      })
      .join(' ')
    appendFileSync(logFile, `[${new Date().toISOString()}] [${level}] ${line}\n`)
  } catch {}
}

const isAttachedConsole = hasConsoleWindow()
const origConsole = { ...globalThis.console }

globalThis.console.log = (...args: any[]) => {
  writeLog('INFO', ...args)
  if (isAttachedConsole) {
    try { origConsole.log(...args) } catch {}
  }
}
globalThis.console.info = (...args: any[]) => {
  writeLog('INFO', ...args)
  if (isAttachedConsole) {
    try { origConsole.info(...args) } catch {}
  }
}
globalThis.console.warn = (...args: any[]) => {
  writeLog('WARN', ...args)
  if (isAttachedConsole) {
    try { origConsole.warn(...args) } catch {}
  }
}
globalThis.console.error = (...args: any[]) => {
  writeLog('ERROR', ...args)
  if (isAttachedConsole) {
    try { origConsole.error(...args) } catch {}
  }
}
globalThis.console.debug = (...args: any[]) => {
  writeLog('DEBUG', ...args)
  if (isAttachedConsole) {
    try { origConsole.debug(...args) } catch {}
  }
}
globalThis.console.trace = (...args: any[]) => {
  writeLog('TRACE', ...args, new Error().stack)
  if (isAttachedConsole) {
    try { origConsole.trace(...args) } catch {}
  }
}
globalThis.console.dir = (...args: any[]) => {
  writeLog('DIR', ...args)
  if (isAttachedConsole) {
    try { origConsole.dir(...args) } catch {}
  }
}
globalThis.console.table = (data: any, columns?: string[]) => {
  writeLog('TABLE', data, columns)
  if (isAttachedConsole) {
    try { origConsole.table(data, columns) } catch {}
  }
}

// 针对 process.stdout / process.stderr 的兜底保护
if (!isAttachedConsole && process.platform === 'win32') {
  try {
    const safeWrite = (chunk: any) => {
      try {
        appendFileSync(logFile, typeof chunk === 'string' ? chunk : chunk.toString())
      } catch {}
      return true
    }
    if (process.stdout) {
      process.stdout.write = safeWrite as any
    }
    if (process.stderr) {
      process.stderr.write = safeWrite as any
    }
  } catch {}
}
