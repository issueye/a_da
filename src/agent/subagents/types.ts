/**
 * 子智能体 (Subagent) 类型定义
 */

import type { ProviderConfig } from '../config'
import type { AgentMessage } from '../core/types'

export type SubagentScope = 'builtin' | 'global' | 'workspace'
export type SubagentMode = 'readonly' | 'readwrite'
export type SubagentColor = 'blue' | 'cyan' | 'purple' | 'green' | 'yellow' | 'orange' | 'red'

export const SUBAGENT_HEX_COLORS: Record<SubagentColor, string> = {
  blue: '#3b82f6',
  cyan: '#06b6d4',
  purple: '#a855f7',
  green: '#10b981',
  yellow: '#eab308',
  orange: '#f97316',
  red: '#ef4444',
}

export function getSubagentColor(subagentId?: string): string {
  if (!subagentId) return '#3b82f6'
  const id = subagentId.toLowerCase()
  if (id.includes('researcher') || id.includes('explore')) return '#06b6d4'
  if (id.includes('reviewer') || id.includes('security')) return '#a855f7'
  if (id.includes('tester') || id.includes('test')) return '#10b981'
  if (id.includes('general')) return '#3b82f6'
  return '#3b82f6'
}

/**
 * 子智能体配置规范契约
 */
export interface SubagentProfile {
  /** 唯一标识符，例如 general_purpose, researcher, code_reviewer, tester */
  id: string
  /** 展示名称，例如 "全能执行专员", "代码调研专员" */
  name: string
  /** 功能与适用场景描述（供主模型决策何时委派调用） */
  description: string
  /** 专属 System Prompt，注入专有规范、策略与思维链约束 */
  systemPrompt: string
  /** 授权该子智能体使用的工具名称白名单（支持 '*' 通配全部工具） */
  allowedTools: string[]
  /** 显式禁用的工具名称黑名单（优先级高于白名单与通配符） */
  disallowedTools?: string[]
  /** 运行模式：只读（严格禁止写工具与破坏性命令）或读写 */
  mode: SubagentMode
  /** 视觉色彩标识 */
  color?: SubagentColor
  /** 默认是否建议后台异步运行 */
  background?: boolean
  /** 单次委派执行的最大步数限制（未配置则无限制，直到自然结束） */
  maxSteps?: number
  /** 模型覆盖选项（可选：切换至速度更快的模型或配置特定推理强度） */
  modelOverride?: {
    model?: string
    effort?: 'max' | 'high' | 'medium' | 'low'
  }
  /** 是否启用 */
  enabled: boolean
  /** 作用域：内置预设、全局用户自定义、当前工作区自定义 */
  scope: SubagentScope
  /** 图标名称 */
  icon?: string
  /** 最后更新时间戳 */
  updatedAt?: number
}

/**
 * 子智能体单步运行状态快照
 */
export interface SubagentStepUpdate {
  step: number
  maxSteps?: number
  status: 'running' | 'done' | 'error'
  currentAction?: string
  toolCallSummary?: string
}

/**
 * 子智能体执行结果
 */
export interface SubagentRunResult {
  ok: boolean
  summary: string
  stepsExecuted: number
  durationMs: number
  toolCallsCount: number
  outputFile?: string
  errorMessage?: string
  messages?: AgentMessage[]
}

/**
 * 主 Agent 委派工具调用的入参
 */
export interface SubagentToolArgs {
  subagent_id: string
  task: string
  additional_context?: string
  async?: boolean
}

/**
 * 主 Agent 向子智能体通信/转向调用的入参
 */
export interface SendSubagentMessageArgs {
  subagent_thread_id: string
  message: string
  summary?: string
}
