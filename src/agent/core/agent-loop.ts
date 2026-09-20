/**
 * 核心 Agent 事件循环
 * 参考 @earendil-works/pi-agent-core/src/agent-loop.ts 设计
 * 实现多轮流式生成、工具并行/顺序调度、生命周期事件派发与钩子拦截
 */

import type { ProviderConfig } from '../config'
import { streamModelChat, type ChatCompletionMessageParam } from '../ai/stream'
import type {
  AgentEndReason,
  AgentEvent,
  AgentLoopOptions,
  AgentMessage,
  AgentTool,
  AgentToolResult,
  AssistantMessage,
  ToolCallBlock,
  ToolResultMessage,
} from './types'

function safeParseArgs(raw: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(raw)
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? (parsed as Record<string, unknown>) : {}
  } catch {
    return {}
  }
}

/**
 * 将内部 AgentMessage 列表转换为发送给模型的 ChatCompletionMessageParam 格式
 */
export function convertMessagesToLlm(
  systemPrompt: string,
  messages: AgentMessage[]
): ChatCompletionMessageParam[] {
  const result: ChatCompletionMessageParam[] = []

  if (systemPrompt.trim()) {
    result.push({ role: 'system', content: systemPrompt })
  }

  for (const m of messages) {
    if (m.role === 'user') {
      result.push({ role: 'user', content: m.content })
    } else if (m.role === 'assistant') {
      const toolCalls = m.toolCalls?.map((tc) => ({
        id: tc.id,
        type: 'function' as const,
        function: {
          name: tc.name,
          arguments: tc.rawArguments,
        },
      }))
      result.push({
        role: 'assistant',
        content: m.content || null,
        tool_calls: toolCalls && toolCalls.length > 0 ? toolCalls : undefined,
      })
    } else if (m.role === 'toolResult') {
      result.push({
        role: 'tool',
        tool_call_id: m.toolCallId,
        content: m.content,
      })
    }
  }

  return result
}

/** 一次工具调用的返回值：给模型的结果，附带是否请求终止整轮。 */
interface ToolStepResult {
  message: ToolResultMessage
  terminate: boolean
}

/**
 * 并行驱动多个工具，并按事件到达顺序转发。
 *
 * 这里不能是 `Promise.all`：异步生成器只有被 `next()` 驱动时才推进，直接并发
 * `run()` 只会启动外层、内部一次都不执行。所以每个工具各起一个驱动任务把事件
 * 投入队列，外层边收边 yield——命令还在跑的时候 tool_execution_update 就已经
 * 到达界面，而不是等全部结束才补发。
 */
async function* mergeRuns(
  calls: ToolCallBlock[],
  runs: AsyncGenerator<AgentEvent, ToolStepResult, void>[]
): AsyncGenerator<AgentEvent, ToolStepResult[], void> {
  const queue: AgentEvent[] = []
  const results: ToolStepResult[] = new Array(runs.length)
  let pending = runs.length
  let wake: (() => void) | null = null
  const bump = (): void => {
    const waiting = wake
    wake = null
    waiting?.()
  }

  runs.forEach((run, index) => {
    void (async () => {
      const call = calls[index]!
      try {
        for (;;) {
          const step = await run.next()
          if (step.done) {
            results[index] = step.value
            break
          }
          queue.push(step.value)
          bump()
        }
      } catch (error) {
        results[index] = {
          message: {
            role: 'toolResult',
            toolCallId: call.id,
            toolName: call.name,
            content: `工具执行异常：${(error as Error).message}`,
            isError: true,
            timestamp: Date.now(),
          },
          terminate: false,
        }
      } finally {
        pending -= 1
        bump()
      }
    })()
  })

  while (pending > 0 || queue.length > 0) {
    if (queue.length === 0) {
      await new Promise<void>((resolve) => {
        wake = resolve
      })
      continue
    }
    yield queue.shift()!
  }

  return results
}

/**
 * 执行 Agent 事件驱动主循环
 */
export async function* runAgentLoop(
  messages: AgentMessage[],
  config: ProviderConfig,
  options: AgentLoopOptions = {}
): AsyncGenerator<AgentEvent, AgentMessage[], void> {
  const maxSteps = options.maxSteps ?? 24
  const tools = options.tools ?? []
  const toolMap = new Map<string, AgentTool>()
  for (const tool of tools) {
    toolMap.set(tool.name, tool)
  }

  const toolSpecs = tools.map((t) => ({
    type: 'function' as const,
    function: {
      name: t.name,
      description: t.description,
      parameters: t.parameters,
    },
  }))

  const workingMessages = [...messages]
  // 撞满步数上限时循环自然走完，不会被下面任何一支改写。
  let endReason: AgentEndReason = 'max_steps'

  yield { type: 'agent_start' }

  try {
    for (let step = 0; step < maxSteps; step++) {
      if (options.signal?.aborted) {
        endReason = 'aborted'
        break
      }

      yield { type: 'turn_start' }

      // 构造当前助手消息容器
      const assistantMessage: AssistantMessage = {
        role: 'assistant',
        content: '',
        thinking: '',
        toolCalls: [],
        timestamp: Date.now(),
      }

      yield { type: 'message_start', message: assistantMessage }

      const llmMessages = convertMessagesToLlm(options.systemPrompt ?? '', workingMessages)
      const rawToolCalls: ToolCallBlock[] = []

      // 发起流式推理
      for await (const chunk of streamModelChat(config, llmMessages, {
        tools: toolSpecs.length > 0 ? toolSpecs : undefined,
        effort: options.effort,
        signal: options.signal,
      })) {
        if (chunk.type === 'text' && chunk.text) {
          assistantMessage.content += chunk.text
          yield {
            type: 'message_update',
            message: assistantMessage,
            delta: { text: chunk.text },
          }
        } else if (chunk.type === 'thinking' && chunk.thinking) {
          assistantMessage.thinking = (assistantMessage.thinking || '') + chunk.thinking
          yield {
            type: 'message_update',
            message: assistantMessage,
            delta: { thinking: chunk.thinking },
          }
        } else if (chunk.type === 'tool_call' && chunk.call) {
          const block: ToolCallBlock = {
            id: chunk.call.id,
            name: chunk.call.name,
            arguments: safeParseArgs(chunk.call.args),
            rawArguments: chunk.call.args,
          }
          rawToolCalls.push(block)
          assistantMessage.toolCalls = rawToolCalls
          yield {
            type: 'message_update',
            message: assistantMessage,
            delta: { toolCall: block },
          }
        } else if (chunk.type === 'error') {
          assistantMessage.stopReason = 'error'
          assistantMessage.errorMessage = chunk.error
        } else if (chunk.type === 'done') {
          if (chunk.stopReason === 'aborted') {
            assistantMessage.stopReason = 'aborted'
          }
        }
      }

      yield { type: 'message_end', message: assistantMessage }
      workingMessages.push(assistantMessage)

      // 如果未产生工具调用，本轮结束
      if (rawToolCalls.length === 0) {
        yield { type: 'turn_end', message: assistantMessage, toolResults: [] }

        // 检查转向消息 (Steering)
        if (options.getSteeringMessages) {
          const steering = await options.getSteeringMessages()
          if (steering.length > 0) {
            for (const s of steering) workingMessages.push(s)
            continue
          }
        }

        // 检查后续消息 (Follow-up)
        if (options.getFollowUpMessages) {
          const followUps = await options.getFollowUpMessages()
          if (followUps.length > 0) {
            for (const f of followUps) workingMessages.push(f)
            continue
          }
        }

        endReason = options.signal?.aborted ? 'aborted' : 'completed'
        break
      }

      // 依据执行模式：若任一工具为 sequential 或全局指定 sequential 则顺序执行
      const isSequential =
        options.toolExecution === 'sequential' ||
        rawToolCalls.some((c) => toolMap.get(c.name)?.executionMode === 'sequential')

      let terminateBatch = false

      /**
       * 执行单个工具，并把事件实时 yield 出去。
       *
       * `onUpdate` 是在 `await execute()` 挂起期间被调用的，而生成器挂起在 await
       * 上时无法 yield——所以用「执行 promise 与唤醒信号赛跑」的方式轮转，把回调
       * 推来的增量在产生的那一刻交出去，而不是攒到结束再补发。
       */
      const runOneTool = (call: ToolCallBlock): AsyncGenerator<AgentEvent, ToolStepResult, void> =>
        (async function* (): AsyncGenerator<AgentEvent, ToolStepResult, void> {
          const fail = (reason: string): ToolStepResult => ({
            message: {
              role: 'toolResult',
              toolCallId: call.id,
              toolName: call.name,
              content: reason,
              isError: true,
              timestamp: Date.now(),
            },
            terminate: false,
          })

          // beforeToolCall 拦截前置校验（审批闸门挂在这里）
          if (options.beforeToolCall) {
            let before
            try {
              before = await options.beforeToolCall(
                {
                  assistantMessage,
                  toolCall: call,
                  args: call.arguments,
                },
                options.signal
              )
            } catch (error) {
              return fail(`工具调用前置校验失败：${(error as Error).message}`)
            }

            if (before?.block) {
              if (before.terminate) terminateBatch = true
              return fail(before.reason || '用户拒绝了此工具调用。')
            }
          }

          yield {
            type: 'tool_execution_start',
            toolCallId: call.id,
            toolName: call.name,
            args: call.arguments,
          }

          const tool = toolMap.get(call.name)
          if (!tool) {
            const message = `未知的工具名称：${call.name}`
            yield { type: 'tool_execution_end', toolCallId: call.id, result: { output: message, ok: false } }
            return fail(message)
          }

          const updates: AgentToolResult[] = []
          let wake: (() => void) | null = null

          const settledRun = tool
            .execute(call.id, call.arguments, options.signal, (partial) => {
              updates.push(partial)
              const waiting = wake
              wake = null
              waiting?.()
            })
            .then(
              (result) => ({ ok: true as const, result }),
              (error) => ({ ok: false as const, error: error as Error })
            )

          let settled: Awaited<typeof settledRun> | null = null
          for (;;) {
            // 先清空再等：工具可能在任何 await 之前就同步推了一帧，攒着不发
            // 等于把「实时」又还回去了。
            while (updates.length > 0) {
              yield { type: 'tool_execution_update', toolCallId: call.id, partialResult: updates.shift()! }
            }
            if (settled) break
            settled = await Promise.race([
              settledRun,
              new Promise<null>((resolve) => {
                wake = () => resolve(null)
              }),
            ])
            wake = null
          }

          let finalOutput: string
          let isError: boolean
          let patch: string | undefined
          let details: unknown

          if (settled.ok) {
            finalOutput = settled.result.output
            isError = !settled.result.ok
            patch = settled.result.patch
            details = settled.result.details
            if (settled.result.terminate) terminateBatch = true
          } else {
            finalOutput = `工具执行异常：${settled.error.message}`
            isError = true
          }

          if (options.afterToolCall) {
            try {
              const after = await options.afterToolCall(
                {
                  assistantMessage,
                  toolCall: call,
                  result: settled.ok ? settled.result : { output: finalOutput, ok: false },
                  isError,
                },
                options.signal
              )
              if (after) {
                if (after.output !== undefined) finalOutput = after.output
                if (after.isError !== undefined) isError = after.isError
                if (after.details !== undefined) details = after.details
                if (after.terminate) terminateBatch = true
              }
            } catch {
              // 后置钩子异常不覆盖工具本身的结果
            }
          }

          yield {
            type: 'tool_execution_end',
            toolCallId: call.id,
            result: { output: finalOutput, ok: !isError, patch },
          }

          return {
            message: {
              role: 'toolResult',
              toolCallId: call.id,
              toolName: call.name,
              content: finalOutput,
              isError,
              patch,
              details,
              timestamp: Date.now(),
            },
            terminate: false,
          }
        })()

      // 执行工具调用
      const toolResults: ToolResultMessage[] = []
      const calls = rawToolCalls.map(runOneTool)

      if (isSequential) {
        for (const run of calls) {
          const step = yield* run
          toolResults.push(step.message)
          workingMessages.push(step.message)
          yield { type: 'message_start', message: step.message }
          yield { type: 'message_end', message: step.message }
        }
      } else {
        const steps = yield* mergeRuns(rawToolCalls, calls)
        for (const step of steps) {
          toolResults.push(step.message)
          workingMessages.push(step.message)
          yield { type: 'message_start', message: step.message }
          yield { type: 'message_end', message: step.message }
        }
      }

      yield { type: 'turn_end', message: assistantMessage, toolResults }

      if (terminateBatch) {
        endReason = 'completed'
        break
      }

      // 检查 shouldStopAfterTurn
      if (options.shouldStopAfterTurn) {
        const stop = await options.shouldStopAfterTurn(
          { message: assistantMessage, toolResults, messages: workingMessages },
          options.signal
        )
        if (stop) {
          endReason = 'completed'
          break
        }
      }
    }
  } finally {
    yield { type: 'agent_end', messages: workingMessages, reason: endReason }
  }

  return workingMessages
}
