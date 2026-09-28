/**
 * 钩子运行层：把**多个插件**的成对钩子合成一份给主循环用的 {@link AgentHooks}。
 *
 * 这一层负责四件主循环不该管的事（设计文档 §6.4、§6.4.4）：
 * 1. **谁有权跑**：能力开关（第三方钩子、plan 模式、按插件覆盖）；
 * 2. **顺序与短路**：按插件加载顺序串行，`before*` 任一 block/terminate 即短路后续；
 * 3. **超时与打点**：单个钩子超时 → **放行**并记 trace（超时不该变成隐式拒绝），
 *    耗时逐点位记入 trace；
 * 4. **受限必须可见**：开关关掉导致某个插件的意图被丢弃时，写一条 trace 说明——
 *    "静默失效"是本项目明确要避免的（§6.4.2）。
 *
 * 工具集的**收窄**不在这里做：那需要主循环手里的"授权集合"，见 `agent-loop.ts`。
 */

import type {
  AgentMode,
} from '../types'
import type { AgentTool } from '../core/types'
import type { ResolvedPluginCapabilities, PluginCapabilities } from '../config'
import type {
  AgentHooks,
  AgentHookKind,
  AfterAgentEndResult,
  AfterSubagentEndResult,
  AfterTurnResult,
  BeforeAgentStartResult,
  BeforeApprovalResult,
  BeforeCompactionResult,
  BeforeSkillLoadResult,
  BeforeSystemPromptResult,
  BeforeThreadCreateResult,
  BeforeTodoUpdateResult,
  BeforeTurnResult,
  SubagentGateResult,
} from '../core/events'
import { getLoadedPlugins } from './registry'
import type { LoadedPlugin, PluginScope } from './types'

/**
 * 单个钩子耗时超过这个值就打点提醒。
 *
 * 与 `hookTimeoutMs`（默认也是 500）是两件事：那是"最多等多久"，这是"多久算慢"。
 * `hookTimeoutMs: 0`（不限）的配置下，慢钩子依然会被记出来。
 */
const HOOK_WARN_MS = 500

/** 工具调用钩子的上下文与返回值（抽出来是为了别让泛型套娃糊在签名里）。 */
type ToolCallHookContext = Parameters<NonNullable<AgentHooks['beforeToolCall']>>[0]
type ToolCallHookResult = Awaited<ReturnType<NonNullable<AgentHooks['beforeToolCall']>>>
type AfterToolCallHookResult = Awaited<ReturnType<NonNullable<AgentHooks['afterToolCall']>>>

interface Contributor {
  pluginId: string
  scope: PluginScope
  hooks: AgentHooks
  capabilities: PluginCapabilities
}

export interface HookRuntimeOptions {
  kind: AgentHookKind
  threadId?: string
  subagentId?: string
  workspace?: string
  /** 协作模式：plan 模式下 `allowPlanModeHooks` 关掉的插件不生效 */
  mode?: AgentMode
  capabilities: ResolvedPluginCapabilities
  /** 打点出口（耗时、超时、受限原因都从这里出去） */
  trace?: (message: string) => void
  /** 插件清单来源；默认取已加载插件索引，测试可注入 */
  plugins?: LoadedPlugin[]
}

/**
 * 合成插件钩子。
 *
 * 没有任何插件贡献某个点位时，返回的对象上**不会出现该属性**——主循环据此完全跳过
 * （不进 try/catch、不计时），这是"无插件时零额外开销"的实现方式（§11 风险 7）。
 */
export function composePluginHooks(options: HookRuntimeOptions): AgentHooks {
  const trace = options.trace ?? ((): void => {})
  const contributors: Contributor[] = []

  // 插件来源按**工作区**取：切项目时不会读到上一个项目的插件清单
  for (const plugin of options.plugins ?? getLoadedPlugins(options.workspace)) {
    const hooks = plugin.contributions.hooks
    if (!hooks || Object.keys(hooks).length === 0) continue

    const pluginId = plugin.manifest.id
    const capabilities = options.capabilities.forPlugin(pluginId)

    if (plugin.manifest.scope !== 'builtin' && !capabilities.allowThirdPartyHooks) {
      trace(`[插件] ${pluginId} 的钩子未生效：allowThirdPartyHooks 已关闭（只能注册工具）`)
      continue
    }
    if (options.mode === 'plan' && !capabilities.allowPlanModeHooks) {
      trace(`[插件] ${pluginId} 的钩子未生效：plan 模式且 allowPlanModeHooks 已关闭`)
      continue
    }

    contributors.push({
      pluginId,
      scope: plugin.manifest.scope ?? 'workspace',
      hooks,
      capabilities,
    })
  }

  /** 某个点位有哪些贡献者；返回空数组表示该点位整体不存在。 */
  const contributorsOf = (point: keyof AgentHooks): Contributor[] =>
    contributors.filter((contributor) => typeof contributor.hooks[point] === 'function')

  /**
   * 跑一个钩子：抛错与超时都**当作"没有意见"**（返回 undefined），并把原因记进 trace。
   * 钩子绝不能让主循环崩掉——插件是可选增强，不是必需件。
   */
  const runHook = async <T>(
    contributor: Contributor,
    point: keyof AgentHooks,
    invoke: () => Promise<T | undefined>
  ): Promise<T | undefined> => {
    const timeoutMs = contributor.capabilities.hookTimeoutMs
    const timedOut = Symbol('timeout')
    const startedAt = Date.now()

    const call = invoke().then(
      (value) => value as T | undefined,
      (error: unknown) => {
        trace(
          `[插件] ${contributor.pluginId} 的 ${String(point)} 抛错，本轮按"无意见"处理：${
            (error as Error)?.message ?? String(error)
          }`
        )
        return undefined
      }
    )

    let result: T | undefined | typeof timedOut
    if (timeoutMs > 0) {
      let timer: ReturnType<typeof setTimeout> | undefined
      const timeout = new Promise<typeof timedOut>((resolve) => {
        timer = setTimeout(() => resolve(timedOut), timeoutMs)
      })
      result = await Promise.race([call, timeout])
      if (timer) clearTimeout(timer)
    } else {
      result = await call
    }

    if (result === timedOut) {
      trace(
        `[插件] ${contributor.pluginId} 的 ${String(point)} 超过 ${timeoutMs}ms，已放行（超时不视为拒绝）`
      )
      return undefined
    }

    const elapsed = Date.now() - startedAt
    if (elapsed > HOOK_WARN_MS) {
      trace(`[插件] ${contributor.pluginId} 的 ${String(point)} 耗时 ${elapsed}ms`)
    }
    return result
  }

  const hooks: AgentHooks = {}

  // ── 整轮开始：只能收窄工具集、追加/替换提示词
  const startContributors = contributorsOf('beforeAgentStart')
  if (startContributors.length > 0) {
    hooks.beforeAgentStart = async (ctx) => {
      let merged: BeforeAgentStartResult | undefined
      for (const contributor of startContributors) {
        const result = await runHook(contributor, 'beforeAgentStart', () =>
          contributor.hooks.beforeAgentStart!({
            ...ctx,
            // 折叠：下一个插件看到的是上一个插件收窄后的工具集，链式收窄可叠加
            tools: merged?.tools ?? ctx.tools,
            systemPrompt: merged?.systemPrompt ?? ctx.systemPrompt,
          })
        )
        if (!result) continue
        merged = mergeBeforeAgentStart(merged, result, contributor, trace)
      }
      return merged
    }
  }

  // ── 整轮结束
  const endContributors = contributorsOf('afterAgentEnd')
  if (endContributors.length > 0) {
    hooks.afterAgentEnd = async (ctx) => {
      let merged: AfterAgentEndResult | undefined
      for (const contributor of endContributors) {
        const result = await runHook(contributor, 'afterAgentEnd', () =>
          contributor.hooks.afterAgentEnd!(ctx)
        )
        if (!result?.appendText) continue
        if (!contributor.capabilities.allowTextRewrite) {
          trace(
            `[插件] ${contributor.pluginId} 想追加收尾文本，但 allowTextRewrite 已关闭，已忽略`
          )
          continue
        }
        merged = {
          appendText: [merged?.appendText, result.appendText].filter(Boolean).join('\n\n'),
        }
      }
      return merged
    }
  }

  // ── 轮次开始
  const turnStartContributors = contributorsOf('beforeTurn')
  if (turnStartContributors.length > 0) {
    hooks.beforeTurn = async (ctx) => {
      // 分开攒再合并：`tools` 的契约类型含 `'casual'`（本仓库尚未实现），
      // 而这里只想留下真正生效的数组，混在一个对象里类型会绕。
      let mergedTools: AgentTool[] | undefined
      let extraMessages: NonNullable<BeforeTurnResult['extraMessages']> = []
      let termination: BeforeTurnResult | undefined

      for (const contributor of turnStartContributors) {
        const result = await runHook(contributor, 'beforeTurn', () =>
          contributor.hooks.beforeTurn!({ ...ctx, tools: mergedTools ?? ctx.tools })
        )
        if (!result) continue

        if (result.tools === 'casual') {
          // core 阶段 B 的惰性工具档位在本仓库尚未实现（开发计划里明确不做）。
          // 明确说出来，而不是让插件以为降级成功了。
          trace(`[插件] ${contributor.pluginId} 请求 'casual' 工具档位，该档位尚未实现，已忽略`)
        } else if (Array.isArray(result.tools)) {
          mergedTools = result.tools
        }

        if (result.extraMessages?.length) {
          extraMessages = [...extraMessages, ...result.extraMessages]
        }

        if (result.terminate) {
          termination = {
            terminate: true,
            terminateReason: result.terminateReason,
            terminateBy: contributor.pluginId,
          }
          // before* 短路：后面的插件不再有机会表态（§6.4.4.2）
          break
        }
      }

      const merged: BeforeTurnResult = {}
      if (mergedTools) merged.tools = mergedTools
      if (extraMessages.length > 0) merged.extraMessages = extraMessages
      if (termination) Object.assign(merged, termination)
      return Object.keys(merged).length > 0 ? merged : undefined
    }
  }

  // ── 轮次结束（事前被短路时**照跑**，这是成对原则的机制保障）
  const turnEndContributors = contributorsOf('afterTurn')
  if (turnEndContributors.length > 0) {
    hooks.afterTurn = async (ctx) => {
      let merged: AfterTurnResult | undefined
      for (const contributor of turnEndContributors) {
        const result = await runHook(contributor, 'afterTurn', () =>
          contributor.hooks.afterTurn!(ctx)
        )
        if (!result) continue
        if (result.appendNote) {
          merged = { ...merged, appendNote: [merged?.appendNote, result.appendNote].filter(Boolean).join('\n\n') }
        }
        if (result.terminate && !merged?.terminate) {
          merged = { ...merged, terminate: true, terminateReason: result.terminateReason, terminateBy: contributor.pluginId }
        }
      }
      return merged
    }
  }

  // ── 工具调用：插件只能**收窄**（block），不能放行——放行权在审批闸门那里
  const toolStartContributors = contributorsOf('beforeToolCall')
  if (toolStartContributors.length > 0) {
    hooks.beforeToolCall = async (ctx) => {
      return await runToolHooks(toolStartContributors, ctx, trace)
    }
  }

  const toolEndContributors = contributorsOf('afterToolCall')
  if (toolEndContributors.length > 0) {
    hooks.afterToolCall = async (ctx) => {
      let merged: AfterToolCallHookResult
      for (const contributor of toolEndContributors) {
        const result = await runHook(contributor, 'afterToolCall', () =>
          contributor.hooks.afterToolCall!(ctx)
        )
        if (!result) continue
        merged = { ...merged, ...result }
      }
      return merged
    }
  }

  // ── 子智能体启动门禁：纯判定类点位，第一个"不放行"即短路
  const gateContributors = contributorsOf('beforeSubagentStart')
  if (gateContributors.length > 0) {
    hooks.beforeSubagentStart = async (ctx) => {
      let merged: SubagentGateResult | undefined
      let tools: AgentTool[] | undefined
      for (const contributor of gateContributors) {
        const result = await runHook(contributor, 'beforeSubagentStart', () =>
          contributor.hooks.beforeSubagentStart!(ctx)
        )
        if (!result) continue

        if (result.tools) {
          // 折叠：下一个判定方看到的是上一个收窄后的集合（与 beforeTurn 同一套规则）
          tools = result.tools
        }
        merged = {
          ...merged,
          ...result,
          // 置信度/校准信息取**第一个给出依据**的判定方；
          // 只报 allowed 而不报依据的，等于没判断（见 access.ts 的判定表）
          confidence: merged?.confidence ?? result.confidence,
          calibrated: merged?.calibrated ?? result.calibrated,
        }

        if (!result.allowed) break
      }
      if (merged && tools) merged.tools = tools
      return merged
    }
  }

  // ── 子智能体结束：清理与复核（事前被短路时照跑，与其它 after* 一致）
  const subagentEndContributors = contributorsOf('afterSubagentEnd')
  if (subagentEndContributors.length > 0) {
    hooks.afterSubagentEnd = async (ctx) => {
      let merged: AfterSubagentEndResult | undefined
      for (const contributor of subagentEndContributors) {
        const result = await runHook(contributor, 'afterSubagentEnd', () =>
          contributor.hooks.afterSubagentEnd!(ctx)
        )
        if (!result?.appendParentNote) continue
        merged = {
          appendParentNote: [merged?.appendParentNote, result.appendParentNote]
            .filter(Boolean)
            .join('\n\n'),
        }
      }
      return merged
    }
  }

  // ── 审批闸门：allow / deny 的效力刻意不对称（见 events.ts 的说明）
  const approvalContributors = contributorsOf('beforeApproval')
  if (approvalContributors.length > 0) {
    hooks.beforeApproval = async (ctx) => {
      let allowed: BeforeApprovalResult | undefined
      for (const contributor of approvalContributors) {
        const result = await runHook(contributor, 'beforeApproval', () =>
          contributor.hooks.beforeApproval!(ctx)
        )
        if (!result?.decision) continue
        const tagged: BeforeApprovalResult = { ...result, decidedBy: contributor.pluginId }
        if (result.decision === 'deny') {
          // 否决压倒放行：一个插件不该能推翻另一个插件的拒绝，所以这里不短路、
          // 继续看完，遇到 deny 立刻定稿
          return tagged
        }
        allowed ??= tagged
      }
      return allowed
    }
  }

  const approvalEndContributors = contributorsOf('afterApproval')
  if (approvalEndContributors.length > 0) {
    hooks.afterApproval = async (ctx) => {
      // 纯观察：审批已经发生，返回值没有语义
      for (const contributor of approvalEndContributors) {
        await runHook(contributor, 'afterApproval', () => contributor.hooks.afterApproval!(ctx))
      }
    }
  }

  // ── 上下文压缩：可追加保留消息，也可替换选择方案（受开关约束）
  const compactionContributors = contributorsOf('beforeCompaction')
  if (compactionContributors.length > 0) {
    hooks.beforeCompaction = async (ctx) => {
      let keepMessages: BeforeCompactionResult['keepMessages'] = []
      let selection: BeforeCompactionResult['selection']
      let by: string | undefined

      for (const contributor of compactionContributors) {
        const result = await runHook(contributor, 'beforeCompaction', () =>
          contributor.hooks.beforeCompaction!(ctx)
        )
        if (!result) continue

        if (result.keepMessages?.length) {
          // 追加保留**永远**生效：它只会让压缩少做点，不会让插件超出授权
          keepMessages = [...(keepMessages ?? []), ...result.keepMessages]
        }

        if (result.selection) {
          if (contributor.capabilities.allowCompactionReplace) {
            selection = result.selection
            by = contributor.pluginId
          } else {
            trace(
              `[插件] ${contributor.pluginId} 想替换压缩选择方案，但 allowCompactionReplace 已关闭，已忽略（追加保留仍生效）`
            )
          }
        }
      }

      const merged: BeforeCompactionResult = {}
      if (keepMessages && keepMessages.length > 0) merged.keepMessages = keepMessages
      if (selection) {
        merged.selection = selection
        merged.by = by
      }
      return Object.keys(merged).length > 0 ? merged : undefined
    }
  }

  const compactionEndContributors = contributorsOf('afterCompaction')
  if (compactionEndContributors.length > 0) {
    hooks.afterCompaction = async (ctx) => {
      for (const contributor of compactionEndContributors) {
        await runHook(contributor, 'afterCompaction', () => contributor.hooks.afterCompaction!(ctx))
      }
    }
  }

  // ── 会话创建：可建议标题、可写自己的 pluginData
  const createContributors = contributorsOf('beforeThreadCreate')
  if (createContributors.length > 0) {
    hooks.beforeThreadCreate = async (ctx) => {
      let merged: BeforeThreadCreateResult | undefined
      let by: string | undefined
      for (const contributor of createContributors) {
        const result = await runHook(contributor, 'beforeThreadCreate', () =>
          contributor.hooks.beforeThreadCreate!(ctx)
        )
        if (!result) continue

        const title = result.title?.trim()
        // 空标题被忽略：宁可用默认的"新会话"，也不产生一个没名字的会话
        if (title) merged = { ...merged, title }

        if (result.data !== undefined) {
          by = contributor.pluginId
          merged = {
            ...merged,
            // 每个插件各写各的键，核心只负责搬运
            data: { ...((merged?.data as Record<string, unknown> | undefined) ?? {}), [contributor.pluginId]: result.data },
            by,
          }
        }
      }
      return merged
    }
  }

  const createdContributors = contributorsOf('afterThreadCreate')
  if (createdContributors.length > 0) {
    hooks.afterThreadCreate = async (ctx) => {
      for (const contributor of createdContributors) {
        await runHook(contributor, 'afterThreadCreate', () => contributor.hooks.afterThreadCreate!(ctx))
      }
    }
  }

  // ── 会话删除：可阻止（受开关约束）、可先归档
  const deleteContributors = contributorsOf('beforeThreadDelete')
  if (deleteContributors.length > 0) {
    hooks.beforeThreadDelete = async (ctx) => {
      let archive = false
      for (const contributor of deleteContributors) {
        const result = await runHook(contributor, 'beforeThreadDelete', () =>
          contributor.hooks.beforeThreadDelete!(ctx)
        )
        if (!result) continue

        if (result.archiveBeforeDelete) archive = true

        if (result.block) {
          if (contributor.capabilities.allowThreadDeleteBlock) {
            // 第一个拦下的即定稿：删除是不可逆操作，没必要继续问
            return {
              block: true,
              blockReason: result.blockReason ?? `插件「${contributor.pluginId}」阻止了删除`,
              blockedBy: contributor.pluginId,
              archiveBeforeDelete: archive,
            }
          }
          trace(
            `[插件] ${contributor.pluginId} 想阻止删除会话，但 allowThreadDeleteBlock 已关闭，已忽略（只能归档）`
          )
        }
      }
      return archive ? { archiveBeforeDelete: true } : undefined
    }
  }

  const deletedContributors = contributorsOf('afterThreadDelete')
  if (deletedContributors.length > 0) {
    hooks.afterThreadDelete = async (ctx) => {
      for (const contributor of deletedContributors) {
        await runHook(contributor, 'afterThreadDelete', () => contributor.hooks.afterThreadDelete!(ctx))
      }
    }
  }

  // ── 会话切换：纯通知，不短路、不看返回值
  const switchContributors = contributorsOf('onThreadSwitch')
  if (switchContributors.length > 0) {
    hooks.onThreadSwitch = async (ctx) => {
      for (const contributor of switchContributors) {
        await runHook(contributor, 'onThreadSwitch', () => contributor.hooks.onThreadSwitch!(ctx))
      }
    }
  }

  // ── 模型请求：最后一刻的裁剪/脱敏（替换后的数组整体生效）
  const llmContributors = contributorsOf('beforeLlmRequest')
  if (llmContributors.length > 0) {
    hooks.beforeLlmRequest = async (ctx) => {
      let messages = ctx.messages
      for (const contributor of llmContributors) {
        const result = await runHook(contributor, 'beforeLlmRequest', () =>
          contributor.hooks.beforeLlmRequest!({ ...ctx, messages })
        )
        if (!result?.messages?.length) continue
        messages = result.messages
        trace(`[插件] ${contributor.pluginId} 替换了本次请求的消息数组（${messages.length} 条）`)
      }
      return messages === ctx.messages ? undefined : { messages }
    }
  }

  const llmEndContributors = contributorsOf('afterLlmResponse')
  if (llmEndContributors.length > 0) {
    hooks.afterLlmResponse = async (ctx) => {
      for (const contributor of llmEndContributors) {
        await runHook(contributor, 'afterLlmResponse', () => contributor.hooks.afterLlmResponse!(ctx))
      }
    }
  }

  // ── 系统提示词（单向）
  const promptContributors = contributorsOf('beforeSystemPrompt')
  if (promptContributors.length > 0) {
    hooks.beforeSystemPrompt = async (ctx) => {
      let merged: BeforeSystemPromptResult | undefined
      for (const contributor of promptContributors) {
        const result = await runHook(contributor, 'beforeSystemPrompt', () =>
          contributor.hooks.beforeSystemPrompt!(ctx)
        )
        if (!result) continue

        if (result.append) {
          merged = {
            ...merged,
            append: [merged?.append, result.append].filter(Boolean).join('\n\n'),
          }
        }
        if (result.replace !== undefined) {
          if (contributor.capabilities.allowSystemPromptReplace) {
            merged = { ...merged, replace: result.replace }
          } else {
            trace(
              `[插件] ${contributor.pluginId} 想替换系统提示词，但 allowSystemPromptReplace 已关闭，已忽略（追加仍生效）`
            )
          }
        }
      }
      return merged
    }
  }

  // ── 技能加载
  const skillContributors = contributorsOf('beforeSkillLoad')
  if (skillContributors.length > 0) {
    hooks.beforeSkillLoad = async (ctx) => {
      let content: string | undefined
      for (const contributor of skillContributors) {
        const result = await runHook(contributor, 'beforeSkillLoad', () =>
          contributor.hooks.beforeSkillLoad!(ctx)
        )
        if (!result) continue
        if (result.content !== undefined) {
          content = result.content
          trace(`[插件] ${contributor.pluginId} 替换了技能「${ctx.skillName}」的正文`)
        }
        if (result.block) {
          // 拦下即定稿：技能看不到就是看不到，没必要继续问
          return {
            block: true,
            blockReason: result.blockReason ?? `插件「${contributor.pluginId}」阻止了加载技能「${ctx.skillName}」`,
            content,
          }
        }
      }
      return content === undefined ? undefined : { content }
    }
  }

  const skillEndContributors = contributorsOf('afterSkillLoad')
  if (skillEndContributors.length > 0) {
    hooks.afterSkillLoad = async (ctx) => {
      for (const contributor of skillEndContributors) {
        await runHook(contributor, 'afterSkillLoad', () => contributor.hooks.afterSkillLoad!(ctx))
      }
    }
  }

  // ── 落盘前脱敏（单向）
  const persistContributors = contributorsOf('beforePersist')
  if (persistContributors.length > 0) {
    hooks.beforePersist = async (ctx) => {
      let content: string | undefined
      for (const contributor of persistContributors) {
        const result = await runHook(contributor, 'beforePersist', () =>
          contributor.hooks.beforePersist!(ctx)
        )
        if (!result?.content) continue
        content = result.content
        trace(`[插件] ${contributor.pluginId} 替换了落盘内容（${result.content.length} 字）`)
      }
      return content === undefined ? undefined : { content }
    }
  }

  // ── 检查点已建立（单向）
  const checkpointContributors = contributorsOf('afterCheckpoint')
  if (checkpointContributors.length > 0) {
    hooks.afterCheckpoint = async (ctx) => {
      for (const contributor of checkpointContributors) {
        await runHook(contributor, 'afterCheckpoint', () => contributor.hooks.afterCheckpoint!(ctx))
      }
    }
  }

  // ── 任务清单：可改写清单、可拦下；事后拿回执
  const todoContributors = contributorsOf('beforeTodoUpdate')
  if (todoContributors.length > 0) {
    hooks.beforeTodoUpdate = async (ctx) => {
      let todos = ctx.todos
      let blocked: BeforeTodoUpdateResult | undefined
      for (const contributor of todoContributors) {
        const result = await runHook(contributor, 'beforeTodoUpdate', () =>
          contributor.hooks.beforeTodoUpdate!({ ...ctx, todos })
        )
        if (!result) continue

        if (Array.isArray(result.todos)) {
          todos = result.todos
          trace(`[插件] ${contributor.pluginId} 改写了任务清单（${todos.length} 项）`)
        }
        if (result.block) {
          blocked = {
            block: true,
            blockReason: result.blockReason ?? `插件「${contributor.pluginId}」拦下了这次清单更新`,
            by: contributor.pluginId,
          }
          // 拦下即定稿：清单不该被写入，也没必要继续问别人
          break
        }
      }

      if (blocked) return blocked
      return todos === ctx.todos ? undefined : { todos }
    }
  }

  const todoEndContributors = contributorsOf('afterTodoUpdate')
  if (todoEndContributors.length > 0) {
    hooks.afterTodoUpdate = async (ctx) => {
      let appendNote: string | undefined
      for (const contributor of todoEndContributors) {
        const result = await runHook(contributor, 'afterTodoUpdate', () =>
          contributor.hooks.afterTodoUpdate!(ctx)
        )
        if (!result?.appendNote) continue
        appendNote = [appendNote, result.appendNote].filter(Boolean).join('\n')
      }
      return appendNote === undefined ? undefined : { appendNote }
    }
  }

  return hooks
}

/** 工具调用的 before 链：只认 `block: true`，第一个拦下的即短路。 */
async function runToolHooks(
  contributors: Contributor[],
  context: ToolCallHookContext,
  trace: (message: string) => void
): Promise<ToolCallHookResult> {
  for (const contributor of contributors) {
    const result = await runSingleToolHook(contributor, context, trace)
    if (result?.block) {
      return {
        block: true,
        reason: result.reason ?? `插件「${contributor.pluginId}」阻止了此工具调用。`,
        terminate: result.terminate,
      }
    }
  }
  return undefined
}

async function runSingleToolHook(
  contributor: Contributor,
  context: ToolCallHookContext,
  trace: (message: string) => void
): Promise<ToolCallHookResult> {
  const timeoutMs = contributor.capabilities.hookTimeoutMs
  const timedOut = Symbol('timeout')
  const startedAt = Date.now()
  const call = contributor.hooks.beforeToolCall!(context).then(
    (value) => value,
    (error: unknown) => {
      trace(
        `[插件] ${contributor.pluginId} 的 beforeToolCall 抛错，已忽略：${
          (error as Error)?.message ?? String(error)
        }`
      )
      return undefined
    }
  )

  let result: ToolCallHookResult | typeof timedOut
  if (timeoutMs > 0) {
    let timer: ReturnType<typeof setTimeout> | undefined
    const timeout = new Promise<typeof timedOut>((resolve) => {
      timer = setTimeout(() => resolve(timedOut), timeoutMs)
    })
    result = await Promise.race([call, timeout])
    if (timer) clearTimeout(timer)
  } else {
    result = await call
  }

  if (result === timedOut) {
    trace(
      `[插件] ${contributor.pluginId} 的 beforeToolCall 超过 ${timeoutMs}ms，已放行（超时不视为拒绝）`
    )
    return undefined
  }
  const elapsed = Date.now() - startedAt
  if (elapsed > HOOK_WARN_MS) {
    trace(`[插件] ${contributor.pluginId} 的 beforeToolCall 耗时 ${elapsed}ms`)
  }
  return result
}

/** 合并 `beforeAgentStart` 的结果：追加类累加，收窄类以后者为准，替换类受能力开关约束。 */
function mergeBeforeAgentStart(
  merged: BeforeAgentStartResult | undefined,
  result: BeforeAgentStartResult,
  contributor: Contributor,
  trace: (message: string) => void
): BeforeAgentStartResult {
  const next: BeforeAgentStartResult = { ...merged }

  if (result.appendSystemPrompt) {
    next.appendSystemPrompt = [merged?.appendSystemPrompt, result.appendSystemPrompt]
      .filter(Boolean)
      .join('\n\n')
  }

  if (result.systemPrompt !== undefined) {
    if (contributor.capabilities.allowSystemPromptReplace) {
      next.systemPrompt = result.systemPrompt
    } else {
      trace(
        `[插件] ${contributor.pluginId} 想替换系统提示词，但 allowSystemPromptReplace 已关闭，已忽略（追加仍生效）`
      )
    }
  }

  if (result.extraMessages?.length) {
    next.extraMessages = [...(merged?.extraMessages ?? []), ...result.extraMessages]
  }
  if (result.tools) next.tools = result.tools

  return next
}
