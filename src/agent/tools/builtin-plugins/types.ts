/**
 * 内置插件包数据契约规范
 * 支持“工具 (Tools) + 技能 (Skills) + 提示词 (Prompts)”三位一体规范
 */

import type { AgentTool } from '../../core/types'

export type BuiltinToolFactory = AgentTool | ((workspace: string) => AgentTool)

export interface BuiltinPluginPackage {
  /** 插件唯一英文标识，例如 git-tools */
  id: string
  /** 插件中文友好名称，例如 Git 变更与协作工具 */
  name: string
  /** 插件用途说明 */
  description: string
  /** 插件内建的 Agent 工具集（支持实例或按 workspace 初始化的工厂函数） */
  tools: BuiltinToolFactory[]
  /** 插件随附的技能规范（SKILL.md 模板） */
  skills?: Array<{
    name: string
    description: string
    content: string
  }>
  /** 插件随附的提示词模板 */
  prompts?: Array<{
    name: string
    description: string
    argumentHint?: string
    isSystem?: boolean
    content: string
  }>
}
