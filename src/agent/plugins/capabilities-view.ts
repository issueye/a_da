/**
 * 能力开关的**展示层与受限说明**（设计文档 §6.4.2、§7.1）。
 *
 * 抽成独立模块的理由：这张表既是界面数据（开关名、说明、关掉的后果），也是"某个插件
 * 被哪些开关限制"的判据——后半句是设计文档的硬要求：**不允许静默失效**。把关掉开关
 * 的后果写在这里、把"受限原因"算成纯函数，就能在没有窗口的情况下测它。
 *
 * 注意措辞：我们只能从插件**注册了哪些点位**判断，无法知道它是否真的会用到那个能力
 * （比如它是否真的想替换系统提示词）。所以每条限制都写成条件句——"若使用…会被忽略"，
 * 而不是断言它一定被影响。
 */

import type { PluginCapabilities } from '../config'
import type { LoadedPlugin } from './types'

export interface CapabilitySwitchMeta {
  key: keyof Omit<PluginCapabilities, 'hookTimeoutMs'>
  label: string
  /** 这个开关管什么 */
  description: string
  /** 关掉之后会发生什么——用户必须能看见后果 */
  effect: string
}

export const CAPABILITY_SWITCHES: CapabilitySwitchMeta[] = [
  {
    key: 'allowThirdPartyHooks',
    label: '第三方扩展可注册钩子',
    description: '工作区与全局目录里的扩展能否用自己的判断影响控制流。',
    effect: '第三方只剩「注册工具」的能力，它们的钩子一律不生效（会写进调试日志）。',
  },
  {
    key: 'allowPlanModeHooks',
    label: '钩子在 plan 模式生效',
    description: '规划模式下是否允许插件按轮次干预工具表、注入消息。',
    effect: 'plan 模式下所有插件的钩子不生效，与未装插件时相同。',
  },
  {
    key: 'allowSystemPromptReplace',
    label: '可替换系统提示词',
    description: '会话开始时能否整体换掉系统提示词（而不是只能在末尾追加）。',
    effect: '插件只能追加；它想替换的部分被忽略，核心会在日志里说明。',
  },
  {
    key: 'allowTextRewrite',
    label: '可追加收尾文本',
    description: '会话结束时能否再追加一段助手消息。',
    effect: '插件想追加的收尾文本被忽略。',
  },
  {
    key: 'allowBuiltinShadow',
    label: '可覆盖同名内置工具',
    description: '插件工具与核心内置工具重名时，是否允许插件那份生效。',
    effect: '重名时保留内置工具，插件的同名工具不注册，并在插件卡上标为冲突。',
  },
  {
    key: 'allowCompactionReplace',
    label: '可替换压缩方案',
    description: '压缩上下文时，能否整体替换「总结哪些、保留哪些」的选择方案。',
    effect: '插件只能追加必须保留的消息，替换意图被忽略。',
  },
  {
    key: 'allowThreadDeleteBlock',
    label: '可阻止删除会话',
    description: '删除会话时，插件能否把它拦下来（钩子本身属会话生命周期，见 M3-7）。',
    effect: '插件只能归档，不能阻止删除。',
  },
]

/**
 * 列出某个插件**当前被哪些开关限制**。
 *
 * 返回空数组表示不受限。每条都写成"它注册了什么 + 因此哪一步会被忽略"，让人能自己
 * 判断这条限制对他有没有影响。
 */
export function describePluginRestrictions(
  plugin: LoadedPlugin,
  capabilities: PluginCapabilities
): string[] {
  const hooks = plugin.contributions.hooks
  if (!hooks || Object.keys(hooks).length === 0) return []

  const reasons: string[] = []
  const isBuiltin = plugin.manifest.scope === 'builtin'

  if (!isBuiltin && !capabilities.allowThirdPartyHooks) {
    // 顶格限制：说清楚是"全部不生效"，不再逐条列
    return ['钩子全部不生效：第三方扩展的钩子已被关闭（allowThirdPartyHooks）']
  }

  if (!capabilities.allowPlanModeHooks) {
    reasons.push('plan 模式下不生效（allowPlanModeHooks 已关闭）')
  }
  if (hooks.beforeAgentStart && !capabilities.allowSystemPromptReplace) {
    reasons.push('若想整体替换系统提示词会被忽略，只能追加（allowSystemPromptReplace 已关闭）')
  }
  if (hooks.afterAgentEnd && !capabilities.allowTextRewrite) {
    reasons.push('若想追加收尾文本会被忽略（allowTextRewrite 已关闭）')
  }
  if (hooks.beforeCompaction && !capabilities.allowCompactionReplace) {
    reasons.push('若想替换压缩方案会被忽略，只能追加保留消息（allowCompactionReplace 已关闭）')
  }
  return reasons
}

/**
 * 校验用户手输的超时值。
 *
 * 0 表示不限——那是**合法**配置，不是"填错了"，所以不能按"必须大于 0"来校验。
 */
export function parseHookTimeout(raw: string): { ok: true; value: number } | { ok: false; reason: string } {
  const text = raw.trim()
  if (text === '') return { ok: false, reason: '不能为空（0 表示不限）' }
  if (!/^\d+$/.test(text)) return { ok: false, reason: '只能填非负整数（0 表示不限）' }
  const value = Number(text)
  if (!Number.isFinite(value)) return { ok: false, reason: '数值超出范围' }
  return { ok: true, value }
}
