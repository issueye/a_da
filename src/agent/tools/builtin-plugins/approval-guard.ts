/**
 * 内置插件：审批策略（approval-guard）
 *
 * 把"哪些工具调用需要经用户确认"这件事**从核心搬到插件层**（设计文档 §6.6.4）。
 *
 * ## 分工
 *
 * 核心保留**执行**：弹卡片、等点击、中止、超时、把结果写回历史。这些是"用户点了什么"
 * 的解释权，不能交给插件。
 *
 * 插件负责**策略**：要不要问、能不能免问、什么时候该二次确认。这正是审批的本质——
 * 一个判断，而不是一次交互。
 *
 * ## 三层策略（自上而下，先命中先返回）
 *
 * 1. **高危需二次确认**（`approvalGuard.confirmCommands`）：命令类工具命中危险模式时
 *    用 `askUser` 发起一次带理由的询问——**即使用户开了自动批准**也要问。排在最前，
 *    否则白名单会把最需要确认的调用一起放过。
 * 2. **只读档位的硬约束**：`approvalMode === 'readonly'` 时写操作一律照常问用户。
 *    这里**不返回 allow**——核心也会忽略它，但插件不该发出一个注定被忽略的意图，
 *    那会让"为什么没生效"变成谜。
 * 3. **免问白名单**（`approvalGuard.autoApprove`）：按工具名放行。默认空，
 *    因为"默认自动批准"是不可接受的默认值。
 *
 * 两个列表都从 `readPluginConfig` 读：数组或**逗号分隔的字符串**都认（后者是配置表单
 * 存进来的形态）。空串按未配置处理，沿用默认值。
 *
 * 用户明确关掉整个插件（插件管理里的启停）时，退回核心的默认行为。
 */

import type { AgentHooks, BeforeApprovalContext, BeforeApprovalResult } from '../../core/events'
import { readPluginConfig } from '../../config'
import type { PluginDescriptor } from './types'

export const APPROVAL_GUARD_PLUGIN_ID = 'approval-guard'

interface ApprovalGuardConfig {
  /** 免问的工具名（精确匹配）。默认空——"默认自动批准"不是可接受的默认值 */
  autoApprove: string[]
  /** 命令类工具里，命中这些模式的调用需要二次确认（即便开了自动批准） */
  confirmCommands: string[]
  /** 命令类工具名（这些工具的参数里带命令文本） */
  commandTools: string[]
}

const DEFAULTS: ApprovalGuardConfig = {
  autoApprove: [],
  commandTools: ['run_command', 'run_background'],
  confirmCommands: [
    'rm ',
    'rmdir',
    'del ',
    'format',
    'git push',
    'git reset',
    'git clean',
    'npm publish',
    'bun publish',
    'shutdown',
    'taskkill',
  ],
}

/**
 * 从 `readPluginConfig` 里取列表字段；非法值回落默认。
 *
 * 两种形态都要认，因为**存进来的形态由写入方决定**：`config.json` / 测试里写的是数组，
 * 而插件配置表单对 `type: 'string'` 的字段原样存字符串（用户填"逗号分隔"）。
 * 早先只认数组，于是界面上配的免问白名单会被静默忽略、照旧每次都问——正是本项目
 * 明确要避免的那类"看起来配上了、其实没生效"。
 *
 * 空字符串视为**未配置**（沿用默认）：表单保存时未填的字段就是空串，不能因为用户
 * 没动它就把高危模式清单清空。
 */
function asStringArray(raw: unknown, fallback: string[]): string[] {
  const list = Array.isArray(raw)
    ? raw
    : typeof raw === 'string' && raw.trim() !== ''
      ? // 中英文逗号与换行都当分隔符：用户手写配置时的习惯各不相同
        raw.split(/[,，\n]/)
      : null
  if (!list) return fallback
  return list
    .filter((item): item is string => typeof item === 'string')
    .map((item) => item.trim())
    .filter((item) => item.length > 0)
}

async function readGuardConfig(): Promise<ApprovalGuardConfig> {
  const raw = await readPluginConfig<Record<string, unknown>>(APPROVAL_GUARD_PLUGIN_ID)
  return {
    autoApprove: asStringArray(raw.autoApprove, DEFAULTS.autoApprove),
    commandTools: asStringArray(raw.commandTools, DEFAULTS.commandTools),
    confirmCommands: asStringArray(raw.confirmCommands, DEFAULTS.confirmCommands),
  }
}

/** 取出这次调用里的命令文本（命令类工具才有）。 */
function commandTextOf(ctx: BeforeApprovalContext): string {
  const args = ctx.toolCall.arguments as Record<string, unknown> | undefined
  const command = args?.command
  return typeof command === 'string' ? command : ''
}

/** 命中了哪个危险模式（给用户看的理由要具体，不能只说"危险"）。
 *
 * 匹配方式是**字面子串**（大小写不敏感），不是通配或正则：`git push` 能命中
 * `git push --force`，但 `docker prune` **匹配不到** `docker system prune -a`
 * （中间隔着 `system`）。刻意选这个语义是因为它可预期、不会有正则回溯问题；
 * 代价是跨词的模式写不出来——配的时候要用真实出现的连续片段。
 */
function matchedPattern(command: string, patterns: string[]): string | null {
  const lowered = command.toLowerCase()
  return patterns.find((pattern) => lowered.includes(pattern.toLowerCase())) ?? null
}

/**
 * 造审批策略钩子。
 *
 * 返回值刻意区分三种情况，因为它们的后果完全不同：
 * - `{ decision: 'allow' }` —— 免问放行（只读档位下会被核心忽略）
 * - `{ decision: 'deny' }` —— 直接拒绝（理由回给模型）
 * - `undefined` —— 照常问用户
 *
 * **绝不返回 `allow` 来"替用户决定"**：那正是本插件要防止的事。免问只发生在
 * 用户显式配置的白名单上。
 */
export function createApprovalGuardHook(): AgentHooks['beforeApproval'] {
  return async (ctx): Promise<BeforeApprovalResult | undefined> => {
    const config = await readGuardConfig()
    const command = commandTextOf(ctx)
    const isCommandTool = config.commandTools.includes(ctx.toolCall.name)

    // 第 3 层：高危命令二次确认。排在免问之前——"即使用户开了自动批准也要问"，
    // 否则白名单会把最需要确认的那些调用一起放过。
    if (isCommandTool && command) {
      const pattern = matchedPattern(command, config.confirmCommands)
      if (pattern) {
        // 有 askUser 才提问；没有（子智能体循环/无界面）就照常交给核心问
        if (!ctx.askUser) {
          ctx.trace?.(
            `[审批策略] ${ctx.toolCall.name} 命中高危模式「${pattern}」但当前没有可用的用户界面，交由核心处理`
          )
          return undefined
        }

        ctx.trace?.(`[审批策略] ${ctx.toolCall.name} 命中高危模式「${pattern}」，向用户二次确认`)
        const answer = await ctx.askUser({
          reason: `这条命令命中了高危模式「${pattern}」，请确认是否执行：\n${command}`,
        })

        // 用户的回答原样采纳：批准就放行，其余一切（拒绝/中止）都当拒绝。
        // 不静默——把"谁答的"记下来，中止与明确拒绝在日志里可区分。
        return answer.approved
          ? { decision: 'allow', reason: `用户二次确认通过（命中「${pattern}」）` }
          : {
              decision: 'deny',
              reason:
                answer.answeredBy === 'aborted'
                  ? `会话已被中止，命令未执行（命中「${pattern}」）`
                  : `用户拒绝执行这条高危命令（命中「${pattern}」）`,
            }
      }
    }

    // 第 1 层：只读档位的硬约束。**不返回 allow**——核心会忽略，插件也不该
    // 发出注定被忽略的意图（那会让"为什么没生效"变成谜）。
    if (ctx.approvalMode === 'readonly' && ctx.isWrite) {
      return undefined
    }

    // 第 2 层：免问白名单。用户显式列出来的工具才免问。
    if (config.autoApprove.includes(ctx.toolCall.name)) {
      ctx.trace?.(`[审批策略] ${ctx.toolCall.name} 在免问白名单里，直接放行`)
      return { decision: 'allow', reason: '命中免问白名单' }
    }

    return undefined
  }
}

/**
 * 插件包定义。
 *
 * `tools: []` 是刻意的：本插件不提供任何工具，它只订审批点位。`PluginDescriptor.tools`
 * 是必填字段（契约要求"插件至少要提供工具"能让加载器免去无谓的可空判断），所以这里
 * 显式给空数组而不是省略。
 */
export const approvalGuardPlugin: PluginDescriptor = {
  id: APPROVAL_GUARD_PLUGIN_ID,
  name: '审批策略 (approval-guard)',
  description:
    '把"哪些工具调用需要经用户确认"做成可配置的策略：免问白名单、高危命令二次确认。执行（弹卡片、等待、中止）仍由核心负责，本插件只决定要不要问。',
  tools: [],
  hooks: {
    beforeApproval: createApprovalGuardHook(),
  },
  configSchema: {
    properties: {
      autoApprove: {
        type: 'string',
        title: '免问白名单',
        description:
          '逗号分隔的工具名，列在这里的工具不再询问用户。默认空（"默认自动批准"不是可接受的默认值）。留空表示不改动。',
      },
      confirmCommands: {
        type: 'string',
        title: '高危命令模式',
        description:
          '逗号分隔的片段（字面子串匹配，不是通配/正则）。命令类工具的参数命中时，即使用户开了自动批准也要二次确认。留空表示沿用内置默认清单。',
      },
    },
  },
  skills: [
    {
      name: 'approval-discipline',
      description: '审批策略的配置方式，以及为什么"默认免问"是不可接受的默认值。',
      content: `---
name: approval-discipline
description: 审批策略的配置方式，以及为什么"默认免问"是不可接受的默认值。
whenToUse: 当用户抱怨"每个命令都要点确认"或想收紧审批时使用。
---

# 审批策略的配置

审批由内置插件 \`approval-guard\` 决定"要不要问"，执行（弹卡片、等待点击、
中止、把结果写回历史）始终由核心负责。

## 三条规则，按优先级

1. **高危命令二次确认**（最高）——命中 \`confirmCommands\` 的命令**即使用户开了
   自动批准也会问**。这是刻意的：白名单不该把最需要确认的调用一起放过。
2. **只读档位的硬约束**——用户选了只读审批档位时，写操作一律照常问，插件不能
   替他取消。
3. **免问白名单**——只有用户显式列进 \`autoApprove\` 的工具才免问。

## 为什么默认是空的白名单

"默认自动批准"不是可接受的默认值。免问必须来自用户的明确选择，而不是插件作者的
判断。加白名单时也请一次只加一个工具，并说清为什么它安全。

## 配置位置

\`~/.a-da/config.json\` 的 \`pluginConfig['approval-guard']\`，或在设置界面的
插件卡片里填。两个字段都是逗号分隔的字符串。
`,
    },
  ],
}
