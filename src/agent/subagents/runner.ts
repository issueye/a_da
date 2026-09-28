/**
 * 子智能体隔离执行引擎 (SubagentRunner)
 * 封装独立的上下文环境 (AgentMessage[])、工具集过滤与递归深度防护，
 * 执行子任务并回传最终结构化结果。
 */

import type { ProviderConfig } from '../config'
import { readPluginCapabilities } from '../config'
import { runAgentLoop } from '../core/agent-loop'
import { composePluginHooks } from '../plugins/hook-runtime'
import { resolveSubagentTools, runSubagentGate } from './access'
import type {
  AgentMessage,
  AgentTool,
  AssistantMessage,
  BeforeToolCallContext,
  BeforeToolCallResult,
  ToolResultMessage,
} from '../core/types'
import { defaultToolRegistry, describeTool } from '../tools'
import type { SubagentProfile, SubagentRunResult, SubagentStepUpdate } from './types'

export interface RunSubagentOptions {
  profile: SubagentProfile
  task: string
  additionalContext?: string
  workspace: string
  parentConfig: ProviderConfig
  signal?: AbortSignal
  onUpdate?: (update: SubagentStepUpdate) => void
  beforeToolCall?: (context: BeforeToolCallContext, signal?: AbortSignal) => Promise<BeforeToolCallResult | undefined>
}

export class SubagentRunner {
  /**
   * 启动子智能体隔离执行
   */
  async run(options: RunSubagentOptions): Promise<SubagentRunResult> {
    const {
      profile,
      task,
      additionalContext,
      workspace,
      parentConfig,
      signal,
      onUpdate,
      beforeToolCall,
    } = options

    const startTime = Date.now()
    const maxSteps = profile.maxSteps

    // 1. 工具解析与 store 共用一份实现（subagents/access.ts）：白名单、黑名单、
    // 通配符、只读模式与防递归在这三条入口上必须完全一致，否则"只读模式漏了个工具"
    // 只会出现在其中之一
    let subagentTools = resolveSubagentTools(profile, workspace)

    // 2. 启动门禁：与 store 同一条实现，插在解析之后、执行之前
    const gateOutcome = await runSubagentGate({
      profile,
      task,
      authorizedTools: subagentTools,
      hooks: await composePluginHooks({
        kind: 'subagent',
        workspace,
        subagentId: profile.id,
        capabilities: await readPluginCapabilities(workspace),
      }),
      workspace,
    })
    if (gateOutcome && !gateOutcome.allowed) {
      return {
        ok: false,
        summary: `子智能体 [${profile.name}] 未通过启动门禁：${gateOutcome.reason ?? '判定不通过'}`,
        stepsExecuted: 0,
        durationMs: Date.now() - startTime,
        toolCallsCount: 0,
        errorMessage: '未通过启动门禁',
      }
    }
    if (gateOutcome?.tools && gateOutcome.tools.length > 0) {
      subagentTools = gateOutcome.tools
    }

    // 2. 准备子智能体专属配置与独立消息历史
    const config: ProviderConfig = {
      ...parentConfig,
      ...(profile.modelOverride?.model ? { model: profile.modelOverride.model } : {}),
    }

    let userPrompt = `【委派任务】\n${task}`
    if (additionalContext?.trim()) {
      userPrompt += `\n\n【补充上下文/参考信息】\n${additionalContext.trim()}`
    }
    userPrompt += '\n\n请针对上述任务要求，自主使用工具调研或处理。完成后直接给出结构化、高信息密度的最终总结与建议。'

    const messages: AgentMessage[] = [
      {
        role: 'user',
        content: userPrompt,
        timestamp: Date.now(),
      },
    ]

    // 3. 构建级联的中止信号
    const abortController = new AbortController()
    if (signal) {
      if (signal.aborted) {
        abortController.abort()
      } else {
        signal.addEventListener('abort', () => abortController.abort(), { once: true })
      }
    }

    let stepsExecuted = 0
    let toolCallsCount = 0
    let lastAssistantMessage: AssistantMessage | null = null
    let latestSummary = ''

    onUpdate?.({
      step: 0,
      maxSteps,
      status: 'running',
      currentAction: `子智能体 [${profile.name}] 已启动`,
    })

    try {
      const loop = runAgentLoop(messages, config, {
        systemPrompt: profile.systemPrompt,
        tools: subagentTools,
        maxSteps,
        effort: profile.modelOverride?.effort ?? 'high',
        toolExecution: 'sequential',
        signal: abortController.signal,
        // 这条路径（`invoke_subagent` 的同步兜底）同样派发插件钩子：插件不该因为
        // "这次没挂会话"而被跳过。没有会话，所以 threadId 缺省。
        hooks: await composePluginHooks({
          kind: 'subagent',
          workspace,
          subagentId: profile.id,
          capabilities: await readPluginCapabilities(workspace),
        }),
        beforeToolCall: async (context, toolSignal) => {
          if (profile.mode === 'readonly' && defaultToolRegistry.isWriteTool(context.toolCall.name)) {
            return { block: true, reason: `子智能体 ${profile.name} 运行在只读安全模式下，禁止执行写操作。` }
          }
          if (beforeToolCall) {
            return beforeToolCall(context, toolSignal)
          }
          return undefined
        },
      })

      for await (const event of loop) {
        if (abortController.signal.aborted) {
          break
        }

        switch (event.type) {
          case 'turn_start':
            stepsExecuted += 1
            onUpdate?.({
              step: stepsExecuted,
              maxSteps,
              status: 'running',
              currentAction: maxSteps
                ? `正在思考第 ${stepsExecuted}/${maxSteps} 步...`
                : `正在思考第 ${stepsExecuted} 步...`,
            })
            break

          case 'message_update':
            if (event.delta.text) {
              latestSummary += event.delta.text
            }
            break

          case 'message_end':
            if (event.message.role === 'assistant') {
              lastAssistantMessage = event.message
              if (event.message.content) {
                latestSummary = event.message.content
              }
            }
            break

          case 'tool_execution_start': {
            toolCallsCount += 1
            const desc = describeTool(event.toolName, event.args)
            onUpdate?.({
              step: stepsExecuted,
              maxSteps,
              status: 'running',
              currentAction: `[${profile.name}] 执行工具: ${desc}`,
              toolCallSummary: desc,
            })
            break
          }

          case 'agent_end':
            messages.length = 0
            messages.push(...event.messages)
            break
        }
      }

      const durationMs = Date.now() - startTime
      const isOk = !abortController.signal.aborted && (lastAssistantMessage?.stopReason !== 'error')

      const resultText =
        latestSummary.trim() ||
        (isOk ? `[${profile.name}] 任务执行完成（共 ${stepsExecuted} 步，调用工具 ${toolCallsCount} 次）。` : '任务未完成或异常中断。')

      onUpdate?.({
        step: stepsExecuted,
        maxSteps,
        status: isOk ? 'done' : 'error',
        currentAction: `执行结束（耗时 ${(durationMs / 1000).toFixed(1)}s）`,
      })

      return {
        ok: isOk,
        summary: resultText,
        stepsExecuted,
        durationMs,
        toolCallsCount,
        messages,
      }
    } catch (error) {
      const durationMs = Date.now() - startTime
      const errorMessage = (error as Error).message || String(error)

      onUpdate?.({
        step: stepsExecuted,
        maxSteps,
        status: 'error',
        currentAction: `执行异常: ${errorMessage}`,
      })

      return {
        ok: false,
        summary: `子智能体 [${profile.name}] 执行失败: ${errorMessage}`,
        stepsExecuted,
        durationMs,
        toolCallsCount,
        errorMessage,
        messages,
      }
    }
  }
}

export const defaultSubagentRunner = new SubagentRunner()
