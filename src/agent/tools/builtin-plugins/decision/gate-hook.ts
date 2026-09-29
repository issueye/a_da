/**
 * 决策插件的**子智能体启动门禁**（设计文档 §6.5 的第三行）。
 *
 * 核心提供点位与失败方向，判定由这里做——核心不该内置"怎么判断"。判定直接用本插件
 * 的引擎（jev → local → heuristic 回退），并且守住两条诚实性契约：
 *
 * 1. **拿不到真实判断就不给依据**：只报 `allowed`、不报 `confidence`/`calibrated`，
 *    核心据此认定"门禁未生效"，并按 profile 的 `failOpen` 决定放行还是拦截；
 * 2. **概率标注校准性**：只有 Jev 引擎的 `calibrated` 才是 true，本地自评是投票占比
 *    恒为 false（见 engine.ts）。
 */

import type { AgentHooks } from '../../../core/events'
import { readDecisionConfig, DEFAULT_GATE_THRESHOLD } from './config'
import { resolveEngine } from './engine'
import { runGate } from './gate'
import type { GateSource } from './gate'

/**
 * 造子智能体门禁钩子。
 *
 * `source` 固定为任务文本：委派时还没有产出物可看，要判的是"这个任务描述是否满足
 * 放行条件"（例如"任务里必须写明验收标准"）。
 */
export function createSubagentGateHook(): AgentHooks['beforeSubagentStart'] {
  return async (ctx) => {
    const config = await readDecisionConfig()
    const resolved = await resolveEngine(config, {})
    if (resolved.engine.id === 'heuristic') {
      // 没有可用引擎：**不给依据**，把失败方向交给核心的 failOpen 规则
      // （不编造 confidence，也不假装判定成功——docs/agent-conventions.md §9）
      ctx.trace?.(
        `[决策插件] 子智能体「${ctx.profileName}」的门禁没有可用引擎（${resolved.note ?? '仅启发式'}），判定不可用`
      )
      return { allowed: true, reason: '没有可用的决策引擎，未做判定' }
    }

    const source: GateSource = 'text'
    try {
      const outcome = await runGate({
        criteria: ctx.criteria,
        source,
        text: ctx.task,
        threshold: ctx.threshold ?? DEFAULT_GATE_THRESHOLD,
        // 交给我们自己判"能不能判"：这里传 true 只是为了让 gate 不因缺少引擎而直接
        // 返回 fail-close——真正的失败方向由核心按 failOpen 决定（上面已排除无引擎）
        failOpen: true,
        workspace: ctx.workspace ?? process.cwd(),
        config,
      })

      ctx.trace?.(
        `[决策插件] 子智能体「${ctx.profileName}」门禁判定：${
          outcome.passed ? '通过' : '未通过'
        }（引擎 ${outcome.engine}，校准 ${outcome.calibrated ? '是' : '否'}，概率 ${outcome.probability.toFixed(2)}）`
      )

      return {
        allowed: outcome.passed,
        confidence: outcome.probability,
        calibrated: outcome.calibrated,
        reason: outcome.note ?? `按标准「${ctx.criteria}」判定${outcome.passed ? '通过' : '未通过'}`,
      }
    } catch (error) {
      // 判定过程抛错同样不给依据，交给核心的 failOpen 规则
      ctx.trace?.(
        `[决策插件] 子智能体「${ctx.profileName}」门禁判定失败：${(error as Error).message}`
      )
      return { allowed: true, reason: `判定过程出错：${(error as Error).message}` }
    }
  }
}

/**
 * 子智能体结束时的复核（与门禁成对）。
 *
 * 只在**门禁没生效**时出声：那意味着"用户配了验收标准，但这次委派其实没人把关"，
 * 而这正是最该被知道的事。判定正常通过/拦截时不再重复一遍——噪音不是信息。
 */
export function createSubagentReviewHook(): AgentHooks['afterSubagentEnd'] {
  return async (ctx) => {
    if (!ctx.gate || ctx.gate.judged) return undefined
    return {
      appendParentNote: `本次委派配了验收标准但**门禁未生效**（${ctx.gate.reason ?? '无判定能力'}），产出未经把关，请自行复核。`,
    }
  }
}
