/**
 * 决策插件的钩子：**工具路由**（设计文档 §6.5 的第一行）。
 *
 * 这是钩子机制的第一个真实消费者，同时也是"成对"的示范：
 * - `beforeTurn` 按配置收窄本轮工具表；
 * - `afterTurn` 拿回执（`effectiveToolNames`）核对"我以为的"与"真正生效的"是否一致，
 *   不一致就如实说出来——这正是本层要解决的诚实性问题（插件不该"以为自己生效了"）。
 *
 * **默认不干预**：只有 `toolRouting` 配了名字才动工具表。
 *
 * 刻意**不做**"每轮调一次模型来定用哪些工具"的自动模式：那要让每轮多一次引擎调用
 * （成本与延迟都翻倍），而没有可用引擎时只能靠启发式——按本项目的原则，拿不到真实
 * 判断时不该假装有判断（见 AGENTS.md §9）。真要做，应该先定一个明确的收益场景。
 */

import type { AgentHooks } from '../../../core/events'
import { readToolRouting } from './config'

/** 请求与实际生效的对照表：`threadId:step` → 本插件请求保留的工具名。 */
const requestedTools = new Map<string, string[]>()

/** 对照表上限：afterTurn 缺失时（运行被硬中断）不该无限增长。 */
const MAX_PENDING = 64

function keyOf(threadId: string | undefined, step: number): string {
  return `${threadId ?? '(无会话)'}:${step}`
}

/**
 * 造决策插件的钩子。
 *
 * 返回的是**模块级共享**的钩子对象（内置插件的 hook 不是按工作区实例化的工厂），
 * 所以内部状态一律按 `threadId:step` 分键，避免并发会话互相串味。
 */
export function createDecisionHooks(): AgentHooks {
  return {
    beforeTurn: async (ctx) => {
      const routing = await readToolRouting()
      if (routing.length === 0) return undefined

      // 只能从当前工具表里挑：核心还会再按"授权集合"裁一次（钩子不能扩张工具集）
      const kept = ctx.tools.filter((tool) => routing.includes(tool.name))
      if (kept.length === 0) {
        // 配置里的名字一个都没命中时**不动工具表**：否则一个拼写错误会让模型突然
        // 没有任何工具可用，而那看起来像"模型变笨了"，完全查不到配置头上
        ctx.trace?.(
          `[决策插件] 工具路由未生效：配置里的名字（${routing.join('、')}）都不在当前工具表里，本轮不干预`
        )
        return undefined
      }

      const dropped = ctx.tools.length - kept.length
      if (dropped > 0) {
        ctx.trace?.(
          `[决策插件] 工具路由生效：本轮保留 ${kept.length}/${ctx.tools.length} 个工具（收窄 ${dropped} 个）`
        )
      }

      if (requestedTools.size >= MAX_PENDING) requestedTools.clear()
      requestedTools.set(keyOf(ctx.threadId, ctx.step), kept.map((tool) => tool.name))

      return { tools: kept }
    },

    afterTurn: async (ctx) => {
      const key = keyOf(ctx.threadId, ctx.step)
      const wanted = requestedTools.get(key)
      if (!wanted) return undefined
      requestedTools.delete(key)

      // 回执核对：实际下发的集合可能被核心的收窄规则改写（或被更高优先级的钩子再收窄），
      // 不一致时说出来，而不是让插件继续以为自己的路由生效了
      const actual = [...ctx.effectiveToolNames].sort()
      const expected = [...wanted].sort()
      if (actual.join('\n') !== expected.join('\n')) {
        ctx.trace?.(
          `[决策插件] 工具路由的实际结果与请求不一致：请求 [${expected.join('、')}]，实际 [${actual.join('、')}]`
        )
      }
      return undefined
    },
  }
}
