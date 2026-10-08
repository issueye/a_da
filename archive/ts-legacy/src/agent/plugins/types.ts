/**
 * 插件系统的契约（单一事实来源）
 *
 * 这里定义「插件是什么」——元信息、能贡献的能力、加载后的统一形状。
 * 两套加载路径（内置插件的声明数组、工作区/全局目录的文件扫描）都产出 {@link LoadedPlugin}，
 * 注册表 / 插件管理页 / 诊断 / 能力开关只认这一种形状，不再为来源分叉。
 *
 * 设计依据：docs/plugin-system-design.md §4.1、§4.3.2、§5.2
 */

import type { AgentTool } from '../core/types'
import type { AgentHooks } from '../core/events'

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
 * 钩子（hooks）自 M2 起可用；子智能体 profile（subagents）尚未声明——届时会新增
 * 字段并同步加载器，也就是说契约还会再变一次，别以为它已经冻结。
 * 加载器对未知字段一律忽略，所以插件多写字段不会报错，但也不会生效。
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
  /**
   * 可干预的决策点（设计文档 §6.1）。返回值能改变控制流，因此受能力开关约束：
   * 第三方插件受 `allowThirdPartyHooks`，plan 模式下受 `allowPlanModeHooks`，
   * 钩子里返回的工具**只能收窄**（唯一不可配置的强制项，§6.4.3）。
   *
   * 只读观察请用 `ExtensionContext.onEvent`——两者刻意分开声明：用户与审查者
   * 一眼就能看出哪些插件是"会动手"的。
   */
  hooks?: AgentHooks
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

/**
 * 第三方扩展的**声明式导出**形态（首选写法）：
 *
 * ```ts
 * export default {
 *   name: '我的插件',
 *   description: '做什么用的',
 *   tools: [ myTool ],
 * } satisfies PluginDescriptorExport
 * ```
 *
 * 与内置插件的 `PluginDescriptor` 是同一套字段，区别只在 `id` / `scope` 允许省略
 * ——由加载器按「目录作用域 + 文件名」填写，插件自己不必知道装在哪。`tools` 也可以
 * 省，只贡献技能或提示词的插件是合法的。
 *
 * 需要运行时上下文（订阅事件、按工作区动态建工具）时改用函数形态：
 * `export default (ctx) => { ctx.registerTool(...); ctx.onEvent(...) }`。
 * 两种形态产出的 {@link LoadedPlugin} 完全一样。
 */
export type PluginDescriptorExport = Partial<PluginDescriptor>

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
  /** 声明式来源（内置）为 true；从文件加载的工作区/全局扩展为 false */
  declarative: boolean
  status: PluginStatus
  diagnostics: PluginDiagnostic[]
  /**
   * 因能力开关被**挡下、未注册**的工具名（目前只有一种来源：`allowBuiltinShadow`
   * 关闭时，占用了核心内置工具名字的插件工具）。
   *
   * 它们仍然留在 {@link PluginContributions.tools} 里可见——界面要能告诉用户
   * "你写的这个工具没生效、为什么"，而不是让它在列表里凭空消失。加载层据此跳过注册，
   * 状态同时标为 `conflict`。为空/未出现表示没有被挡下的工具。
   */
  blockedTools?: string[]
}
