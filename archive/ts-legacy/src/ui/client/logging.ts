/**
 * UI 侧的应用日志：把"日志写到哪"这件事收在客户端层。
 *
 * 为什么不在 `ui/main.tsx` 里直接算：那需要 `agent/home` 的 `getAppHome`，
 * 而**界面组件不许 import agent 侧的实现符号**（`src/ui/protocol-boundary.test.ts` 盯着这条线）。
 * 客户端层是允许的——它本来就是"UI 与主机/环境之间"的那一层。
 *
 * 注意 `platform/init` 也会往同一个文件写（它劫持 `console.log`）；两者共用一条约定：
 * `A_DA_HOME || ~/.a-da` 下的 `app_debug.log`。
 */

import { appendFileSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { getAppHome } from '../../agent/home'

const logDir = getAppHome()
try {
  mkdirSync(logDir, { recursive: true })
} catch {
  // 建不出来就退化成"只写内存"，不能让日志拖住启动
}

const logFile = join(logDir, 'app_debug.log')

/** 追加一行应用日志；永远不抛（日志本身不该拖住启动）。 */
export function log(message: string): void {
  try {
    appendFileSync(logFile, `[${new Date().toISOString()}] ${message}\n`)
  } catch {
    // 写不进去就算了
  }
}

/** 日志文件路径（诊断与测试用）。 */
export function appLogPath(): string {
  return logFile
}
