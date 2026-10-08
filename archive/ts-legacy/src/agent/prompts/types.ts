/**
 * 提示词系统数据模型定义
 * 支持内置预装、工作区项目级和用户全局三层提示词管理。
 */

export type PromptScope = 'builtin' | 'workspace' | 'global' | 'plugin'

export interface PromptItem {
  /** 唯一标识 */
  id: string
  /** 提示词名称，例如：代码审查、中文编码规范 */
  name: string
  /** 简要用途说明 */
  description: string
  /** 参数提示占位符（例如：<PR-URL> 或 [instructions]），对齐 pi Agent 规范 */
  argumentHint?: string
  /** 提示词具体正文（Markdown 文本格式，支持 $1, $@, ${N:-default} 等动态占位符） */
  content: string
  /** 作用域范围：内置、当前工作区、全局通用、插件提供 */
  scope: PromptScope
  /** 是否启用该提示词 */
  enabled: boolean
  /** 是否作为系统级提示词（System Prompt）自动注入到模型对话循环中 */
  isSystem: boolean
  /** 磁盘文件路径（内置提示词无此项） */
  filePath?: string
  /** 若由插件引入，则记录插件名 */
  pluginName?: string
  /** 若由插件引入，记录插件唯一 ID（如 workspace:my-plugin 或 global:my-plugin） */
  pluginId?: string
  /** 最后更新时间戳 */
  updatedAt: number
}

export interface CreatePromptOptions {
  /** 提示词名称 */
  name: string
  /** 描述信息 */
  description?: string
  /** 参数输入提示 */
  argumentHint?: string
  /** 提示词正文内容 */
  content: string
  /** 目标作用域（工作区或全局） */
  scope: 'workspace' | 'global'
  /** 是否作为系统提示词 */
  isSystem?: boolean
  /** 是否立即启用（默认为 true） */
  enabled?: boolean
}
