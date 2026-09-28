/**
 * 插件系统的契约（单一事实来源）
 *
 * 这里定义「插件是什么」——元信息、能贡献的能力、加载后的统一形状。
 * 两套加载路径（内置插件的声明数组、工作区的 jiti 扫描）都产出 {@link LoadedPlugin}，
 * 注册表 / 插件管理页 / 诊断 / 能力开关只认这一种形状，不再为来源分叉。
 *
 * 设计依据：docs/plugin-system-design.md §4.1、§4.3.2、§5.2
 */

import type { AgentTool } from '../core/types'

/** 插件工具：实例，或按 workspace 初始化的工厂 */
export type PluginToolFactory = AgentTool | ((workspace: string) => AgentTool)

/** 插件来源，决定"能否在不执行代码的前提下展示"与加载顺序，**不决定权限** */
export type PluginScope = 'builtin' | 'workspace' | 'global'

/** 插件随附的技能规范（SKILL.md 文本） */
export interface PluginSkill {
  name: string
  description: string
  content: string
}

/** 插件随附的提示词模板 */
export interface PluginPrompt {
  name: string
  description: string
  argumentHint?: string
  /** 为真时注入系统提示词，且不再出现在斜杠菜单里 */
  isSystem?: boolean
  content: string
}

/** 插件的元信息 */
export interface PluginManifest {
  /** 唯一英文标识，例如 git-tools */
  id: string
  /** 中文友好名称 */
  name: string
  /** 用途说明 */
  description: string
  /** 语义版本，用于展示与兼容检查 */
  version?: string
  author?: string
  /** 插件来源；由加载器填写，插件自身不必声明 */
  scope?: PluginScope
  /** 声明兼容的应用版本范围，不匹配时标记 incompatible（软失败，仍加载） */
  engines?: { a_da?: string }
}

/** 插件声明的、需要用户提供的配置项 */
export interface PluginConfigProperty {
  type: 'string' | 'number' | 'boolean' | 'secret'
  title: string
  description?: string
  default?: unknown
  /** 必填项缺失时插件标记 not-ready，且**不注册其工具** */
  required?: boolean
}

export interface PluginConfigSchema {
  properties: Record<string, PluginConfigProperty>
}

/**
 * 插件提供的全部能力，每一项都可选。
 *
 * 注意：钩子（hooks）与子智能体（subagents）在后续里程碑接入，
 * 此处先预留字段，避免契约再次变更。
 */
export interface PluginContributions {
  /** 工具集（支持实例或按 workspace 初始化的工厂函数） */
  tools?: PluginToolFactory[]
  skills?: PluginSkill[]
  prompts?: PluginPrompt[]
  /** 声明需要用户配置的项 */
  configSchema?: PluginConfigSchema
  /** 依赖的其他插件 id；缺失时标记 broken 且不注册工具 */
  dependsOn?: string[]
}

/**
 * 一个插件的完整描述。
 *
 * 这是**编写期**的形态：内置插件就是这样的纯数据对象，第三方扩展可以直接
 * 导出它（首选写法），也可以导出函数由核心调用后收集。
 *
 * `tools` 是**必填**的：一个不提供任何工具的"插件"没有意义，而且必填能让
 * 加载器与测试免去无谓的可空判断。
 */
export interface PluginDescriptor extends PluginManifest, PluginContributions {
  tools: PluginToolFactory[]
}

/** 加载诊断级别 */
export type PluginDiagnosticLevel = 'info' | 'warn' | 'error'

export interface PluginDiagnostic {
  pluginId: string
  level: PluginDiagnosticLevel
  message: string
  /** 可操作的建议，例如"在设置里补上 api_key" */
  hint?: string
}

/** 插件加载后的状态，插件管理页直接据此渲染 */
export type PluginStatus =
  | 'ready'
  /** 缺必填配置 */
  | 'not-ready'
  /** 应用版本不匹配 */
  | 'incompatible'
  /** 依赖缺失或加载抛错 */
  | 'broken'
  /** 与其它插件/内置工具存在工具名冲突 */
  | 'conflict'

/**
 * 加载后的插件——**无论来源，都是这个形状**。
 *
 * `declarative` 只用来决定"能否在不执行代码的前提下展示"（内置可以），
 * 不用于权限；权限只看用户配置的能力开关。
 */
export interface LoadedPlugin {
  manifest: PluginManifest
  contributions: PluginContributions
  /** 声明式来源（内置）为 true；jiti 执行的工作区/全局扩展为 false */
  declarative: boolean
  status: PluginStatus
  diagnostics: PluginDiagnostic[]
}
