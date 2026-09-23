/**
 * 子智能体隔离执行引擎 (SubagentRunner)
 * 封装独立的上下文环境 (AgentMessage[])、工具集过滤与递归深度防护，
 * 执行子任务并回传最终结构化结果。
 */

import type { ProviderConfig } from '../config'
import { runAgentLoop } from '../core/agent-loop'
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

    // 1. 获取工作区全量工具并实施白名单、黑名单与安全模式过滤
    const allTools = defaultToolRegistry.getToolsForWorkspace(workspace)
    const allowedSet = new Set(profile.allowedTools)
    const disallowedSet = new Set(profile.disallowedTools ?? [])
    // 绝对防御：杜绝子智能体再次调用子智能体与通信工具 (防套娃防递归死锁)
    disallowedSet.add('invoke_subagent')
    disallowedSet.add('check_subagent')
    disallowedSet.add('send_subagent_message')
    disallowedSet.add('resume_subagent')

    const subagentTools: AgentTool[] = allTools.filter((t) => {
      // 黑名单排除优先
      if (disallowedSet.has(t.name)) return false
      // 白名单与通配符过滤
      if (!allowedSet.has('*') && !allowedSet.has(t.name)) return false
      // 只读模式过滤：若为只读智能体，则严格禁止一切产生写副作用的工具
      if (profile.mode === 'readonly' && defaultToolRegistry.isWriteTool(t.name)) return false
      return true
    })

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
