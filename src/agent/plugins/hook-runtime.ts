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
  AfterTurnResult,
  BeforeAgentStartResult,
  BeforeTurnResult,
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

  for (const plugin of options.plugins ?? getLoadedPlugins()) {
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
