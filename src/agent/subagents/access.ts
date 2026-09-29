/**
 * 子智能体的**入口判定**：工具集解析与启动门禁（设计文档 §6.3）。
 *
 * 抽成独立模块的理由很直接：同一套白名单/黑名单/只读/防递归过滤原先在
 * `store.startSubagentThread`、`store.resumeSubagentThread`、`subagents/runner.ts`
 * 三处各写了一遍。三份实现意味着"只读模式漏了一个工具"这类问题会只在其中一条路径
 * 上出现——而这条路径可能是用户最不常走的那条，于是很久都不会被发现。
 *
 * 门禁（gate）也在这一层：它必须插在 **enabled 检查之后、建会话之前**，且三条入口
 * 都要走同一份实现。
 */

import type { AgentTool } from '../core/types'
import type { AgentHooks, SubagentGateResult } from '../core/events'
import { defaultToolRegistry } from '../tools'
import type { SubagentProfile } from './types'

/** 子智能体永远不该再拿到这些工具：套娃、递归死锁、以及主智能体专属的仪表盘。 */
const NEVER_FOR_SUBAGENT = [
  'invoke_subagent',
  'check_subagent',
  'send_subagent_message',
  'resume_subagent',
  // 子智能体不该再去等别的子智能体（防套娃）
  'await_subagents',
]

/**
 * 解析某个子智能体可用的工具集。
 *
 * 过滤顺序：黑名单 → 白名单/通配符 → 只读模式。只读那一步依赖 `isWriteTool` 的静态
 * 白名单（**失败安全**：名单外的一律算写操作），所以插件注册的只读工具必须登记进
 * `ToolRegistry.READ_ONLY`，否则只读子智能体拿不到它。
 *
 * `notify_parent` **不在这里注入**，由调用方补上：它需要子会话 id，而且它的实现住在
 * `tools/builtins/subagent` 里——从这里 import 会绕成
 * `access → tools/builtins/subagent → subagents/runner → access` 的环。
 * 调用方要记得补（它有子会话 id，本来也得自己决定要不要补）。
 */
export function resolveSubagentTools(profile: SubagentProfile, workspace: string): AgentTool[] {
  const allTools = defaultToolRegistry.getToolsForWorkspace(workspace)
  const allowedSet = new Set(profile.allowedTools)
  const disallowedSet = new Set([...(profile.disallowedTools ?? []), ...NEVER_FOR_SUBAGENT])

  return allTools.filter((tool) => {
    if (disallowedSet.has(tool.name)) return false
    if (!allowedSet.has('*') && !allowedSet.has(tool.name)) return false
    if (profile.mode === 'readonly' && defaultToolRegistry.isWriteTool(tool.name)) return false
    return true
  })
}

export interface SubagentGateOutcome {
  allowed: boolean
  /** 是否真的做了判定。false 表示"门禁未生效"（没有判定能力），此时 allowed 由 failOpen 决定 */
  judged: boolean
  reason?: string
  calibrated?: boolean
  confidence?: number
  /** 收窄后的工具集（只在门禁给了 tools 时出现，且已按授权集合裁过） */
  tools?: AgentTool[]
}

export interface RunSubagentGateOptions {
  profile: SubagentProfile
  task: string
  /** 授权集合：门禁返回的 tools 只能从它里面挑（§6.4.3） */
  authorizedTools: AgentTool[]
  hooks?: AgentHooks
  workspace?: string
  /** 提醒出口（"门禁未生效"之类必须说出来） */
  notice?: (message: string) => void
}

/**
 * 跑一次子智能体启动门禁。
 *
 * 返回 `undefined` 表示**这个 profile 没有配门禁**（没写 criteria）——那是正常情况，
 * 不是失败。
 *
 * 失败方向（设计文档 §6.4.4.5，与早期版本的"默认 fail-close"相反）：
 *
 * | 有判定能力 | `failOpen` | 结果 |
 * |---|---|---|
 * | 否 | 未配置 | **放行** + 提示"门禁未生效" |
 * | 否 | `false` | 拦截 |
 * | 是，但拿不出校准信息 | 未配置 | 放行 + 提示"门禁未生效" |
 * | 是，但拿不出校准信息 | `false` | **拦截**（判定方自说自话不能绕过用户的显式要求） |
 * | 是，有校准信息 | 任意 | 用判定给的 `allowed` |
 *
 * "用户没表态"不该被核心解读成"要求安全"；但用户明确要求了，就必须做到。
 */
export async function runSubagentGate(
  options: RunSubagentGateOptions
): Promise<SubagentGateOutcome | undefined> {
  const gate = options.profile.gate
  if (!gate?.criteria?.trim()) return undefined

  const notice = options.notice ?? ((): void => {})
  const failOpen = gate.failOpen
  const profile = options.profile

  const verdict: SubagentGateResult | undefined = options.hooks?.beforeSubagentStart
    ? await options.hooks
        .beforeSubagentStart({
          kind: 'subagent',
          workspace: options.workspace,
          profileId: profile.id,
          profileName: profile.name,
          task: options.task,
          criteria: gate.criteria,
          threshold: gate.threshold,
          failOpen,
        })
        .catch((error: unknown) => {
          notice(
            `[子智能体门禁] 判定钩子抛错，按"门禁未生效"处理：${
              (error as Error)?.message ?? String(error)
            }`
          )
          return undefined
        })
    : undefined

  // "有没有真的判断"的判据是**拿不拿得出校准信息**：判定方只报 allowed 而不报
  // confidence/calibrated，就等于没给依据（docs/agent-conventions.md §9 的诚实性契约）
  const judged = verdict?.confidence !== undefined || verdict?.calibrated !== undefined
  const authorized = new Map(options.authorizedTools.map((tool) => [tool.name, tool]))

  if (!judged) {
    const reason = verdict?.reason ?? '没有可用的判定能力（无引擎或判定未给出依据）'
    if (failOpen === false) {
      notice(`[子智能体门禁] 判定不可用（${reason}），但用户要求 failOpen: false → 拦截`)
      return { allowed: false, judged: false, reason: `门禁未生效且要求复核：${reason}` }
    }
    // 提示必须出现：用户配了 criteria 却没配 failOpen，核心放行但他得知道门禁没生效
    notice(`[子智能体门禁] 门禁未生效（${reason}）→ 按放行处理；需要拦截请显式配置 failOpen: false`)
    return { allowed: true, judged: false, reason }
  }

  const narrowed = verdict?.tools ? narrowGateTools(verdict.tools, authorized, notice) : undefined
  return {
    allowed: verdict!.allowed,
    judged: true,
    reason: verdict!.reason,
    calibrated: verdict!.calibrated,
    confidence: verdict!.confidence,
    tools: narrowed,
  }
}

/** 门禁只能收窄工具集：取授权实例，未授权的一律剔除并说明。 */
function narrowGateTools(
  requested: AgentTool[],
  authorized: Map<string, AgentTool>,
  notice: (message: string) => void
): AgentTool[] {
  const kept: AgentTool[] = []
  const dropped: string[] = []
  for (const tool of requested) {
    const original = authorized.get(tool.name)
    if (original) kept.push(original)
    else dropped.push(tool.name)
  }
  if (dropped.length > 0) {
    notice(`[子智能体门禁] 返回了未授权的工具，已剔除：${dropped.join(', ')}`)
  }
  return kept
}
