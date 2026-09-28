/**
 * 核心 Agent 事件循环
 * 参考 @earendil-works/pi-agent-core/src/agent-loop.ts 设计
 * 实现多轮流式生成、工具并行/顺序调度、生命周期事件派发与钩子拦截
 */

import { readFileSync } from 'node:fs'
import { extname, isAbsolute, join } from 'node:path'
import type { ProviderConfig } from '../config'
import type { TokenUsage } from '../ai/types'
import { streamModelChat, type ChatCompletionMessageParam, type ChatCompletionContentPart } from '../ai/stream'
import { NON_NEGOTIABLE_TOOL_TAIL } from './events'
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

/** 下发给模型的一份工具声明（OpenAI 兼容形态）。 */
type ToolSpec = {
  type: 'function'
  function: { name: string; description: string; parameters: Record<string, unknown> }
}

function safeParseArgs(raw: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(raw)
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? (parsed as Record<string, unknown>) : {}
  } catch {
    return {}
  }
}

/** 将本地或远程图片路径转换为模型可接收的 data URL */
export function imageToDataUrl(imagePathOrUrl: string, workspace?: string): string {
  if (
    imagePathOrUrl.startsWith('data:') ||
    imagePathOrUrl.startsWith('http://') ||
    imagePathOrUrl.startsWith('https://')
  ) {
    return imagePathOrUrl
  }
  try {
    const fullPath = workspace && !isAbsolute(imagePathOrUrl) ? join(workspace, imagePathOrUrl) : imagePathOrUrl
    const ext = extname(fullPath).toLowerCase().replace('.', '')
    const mime =
      ext === 'jpg' || ext === 'jpeg'
        ? 'image/jpeg'
        : ext === 'png'
        ? 'image/png'
        : ext === 'webp'
        ? 'image/webp'
        : ext === 'gif'
        ? 'image/gif'
        : ext === 'svg'
        ? 'image/svg+xml'
        : 'image/png'
    const buffer = readFileSync(fullPath)
    return `data:${mime};base64,${buffer.toString('base64')}`
  } catch {
    return imagePathOrUrl
  }
}

/**
 * 将内部 AgentMessage 列表转换为发送给模型的 ChatCompletionMessageParam 格式
 */
export function convertMessagesToLlm(
  systemPrompt: string,
  messages: AgentMessage[],
  options?: { supportsImages?: boolean; workspace?: string },
): ChatCompletionMessageParam[] {
  const result: ChatCompletionMessageParam[] = []

  if (systemPrompt.trim()) {
    result.push({ role: 'system', content: systemPrompt })
  }

  for (const m of messages) {
    if (m.role === 'user') {
      if (m.images && m.images.length > 0) {
        if (options?.supportsImages !== false) {
          const parts: ChatCompletionContentPart[] = []
          if (m.content) {
            parts.push({ type: 'text', text: m.content })
          }
          for (const img of m.images) {
            const url = imageToDataUrl(img, options?.workspace)
            parts.push({ type: 'image_url', image_url: { url } })
          }
          result.push({ role: 'user', content: parts })
        } else {
          // 模型不支持视觉输入时，将图片作为文字提示降级附加
          const note = m.images.map((img) => `[附带图片: ${img}]`).join('\n')
          const text = m.content ? `${m.content}\n\n${note}` : note
          result.push({ role: 'user', content: text })
        }
      } else {
        result.push({ role: 'user', content: m.content })
      }
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
  const maxSteps = options.maxSteps

  // ── 插件钩子的运行状态（`hooks` 缺席时下面每一处调用点都会被整体跳过）──
  const hooks = options.hooks
  const notice = (message: string): void => options.onNotice?.(message)

  const hookBase = {
    kind: options.hookContext?.kind ?? ('main' as const),
    threadId: options.hookContext?.threadId,
    subagentId: options.hookContext?.subagentId,
    workspace: options.workspace,
    // 插件自己的打点与循环的提醒走同一条通道：都会出现在调试面板里
    trace: notice,
  }

  /**
   * 调一个插件钩子，**抛错一律当作"没有意见"**（设计文档 §6.2 改造点 3）。
   *
   * 运行层已经逐插件兜了一层，这里再兜一层是因为"绝不让插件打崩主循环"必须是**机制**
   * 而不是运行层的自觉：钩子是可选增强，任何一层漏了 try/catch，用户看到的就是整个
   * 回合莫名其妙断掉，而错误来源还指向插件之外。
   */
  const callHook = async <T>(
    label: string,
    invoke: () => Promise<T | undefined>
  ): Promise<T | undefined> => {
    try {
      return await invoke()
    } catch (error) {
      notice(`[插件] ${label} 抛错，已忽略：${(error as Error)?.message ?? String(error)}`)
      return undefined
    }
  }
  let systemPrompt = options.systemPrompt ?? ''
  /** 有插件要求"本轮跑完就收尾"（before* 或 after* 都可能提出）。 */
  let terminateAfterTurn = false
  let terminateByHook: string | undefined
  let stepsExecuted = 0

  /**
   * 授权上限：钩子只能在它里面挑工具。
   *
   * **唯一不可配置的强制项**（设计文档 §6.4.3）：工具集是审批闸门的依据
   * （`isWriteTool` 是静态名字白名单），钩子若能塞进新名字就等于绕过审批，也会让
   * `effectiveToolNames` 那份"回执"失真。注意取的是**授权实例**而不是钩子给的实例
   * ——否则插件可以顶着 `read_file` 这个名字塞一份自己的实现进来。
   */
  const authorizedByName = new Map<string, AgentTool>(
    (options.tools ?? []).map((tool) => [tool.name, tool])
  )
  const narrowTools = (requested: AgentTool[]): AgentTool[] => {
    const kept: AgentTool[] = []
    const dropped: string[] = []
    for (const tool of requested) {
      const authorized = authorizedByName.get(tool.name)
      if (authorized) kept.push(authorized)
      else dropped.push(tool.name)
    }
    if (dropped.length > 0) {
      notice(`[插件] 钩子返回了未授权的工具，已剔除：${dropped.join(', ')}`)
    }
    return kept
  }

  /**
   * 本轮**期望**的工具集（`applyTools` 的输入）。
   *
   * 来源依次是 `options.tools` → `beforeAgentStart.tools` → 每轮 `beforeTurn.tools`，
   * 后两者只能收窄。工具表因此是**每轮算一次**而不是循环外算死（M2 的破坏性重构，
   * 行为等价由 `loop-equivalence.test.ts` 钉住）：
   *
   * 以**工具名集合**为签名做缓存——集合没变就复用同一份 `toolSpecs` 引用，模型请求体
   * 逐轮完全相同（对提示缓存友好），没有插件干预时零额外开销。同名工具视为等价：
   * 实例由 `ToolRegistry` 说了算，钩子只改"这一轮用哪些名字"。
   */
  let desiredTools: AgentTool[] = options.tools ?? []

  let toolMap = new Map<string, AgentTool>()
  let toolSpecs: ToolSpec[] = []
  let toolSig = ''

  const applyTools = (next: AgentTool[]): void => {
    // 先按名字收进 map，再**由 map 生成** specs 与签名：工具名是集合语义，钩子递回
    // 重复的名字（或同一个名字给两次）不该让模型收到两条同名声明。
    const byName = new Map(next.map((tool) => [tool.name, tool]))
    const sig = Array.from(byName.keys()).join('\n')
    if (sig === toolSig) return
    toolSig = sig
    toolMap = byName
    toolSpecs = Array.from(byName.values()).map((tool) => ({
      type: 'function' as const,
      function: {
        name: tool.name,
        description: tool.description,
        parameters: tool.parameters,
      },
    }))
  }

  applyTools(desiredTools)

  const workingMessages = [...messages]
  let endReason: AgentEndReason = 'completed'
  const loopStartTime = Date.now()
  const accumulatedTokens = {
    promptTokens: 0,
    completionTokens: 0,
    totalTokens: 0,
    thinkingTokens: 0,
    cachedTokens: 0,
  }

  // 整轮开始钩子：可替换/追加系统提示词、追加初始消息、收窄初始工具集。
  //
  // 放在 agent_start **之前**：它改的是"这一轮怎么开始"，事件流里不该出现"已经开始了"
  // 之后初始条件才变化的错位。钩子抛错/超时由运行层兜住（当作没有意见）。
  if (hooks?.beforeAgentStart) {
    const result = await callHook('beforeAgentStart', () =>
      hooks.beforeAgentStart!({
        messages: [...workingMessages],
        tools: desiredTools,
        systemPrompt,
        ...hookBase,
      }))
    if (result) {
      if (result.systemPrompt !== undefined) {
        // 替换时核心仍附上不可协商的机制段落（§6.2.1）：插件换得掉人设，换不掉
        // "有审批、有检查点、不许声称做过没做的事"这些运行时事实
        systemPrompt = `${result.systemPrompt}\n\n${NON_NEGOTIABLE_TOOL_TAIL}`
      }
      if (result.appendSystemPrompt) {
        systemPrompt = systemPrompt
          ? `${systemPrompt}\n\n${result.appendSystemPrompt}`
          : result.appendSystemPrompt
      }
      if (result.extraMessages?.length) workingMessages.push(...result.extraMessages)
      if (result.tools) {
        desiredTools = narrowTools(result.tools)
        // 收窄后的集合成为新的授权上限：后面每轮只能在此基础上再收窄
        authorizedByName.clear()
        for (const tool of desiredTools) authorizedByName.set(tool.name, tool)
        applyTools(desiredTools)
      }
    }
  }

  yield { type: 'agent_start' }

  /**
   * 轮次结束钩子。
   *
   * **两个 `turn_end` 出口都要调**（纯文本轮与带工具轮）——漏掉任何一个，"成对"就
   * 成了空话，而漏掉的多半正是纯文本轮（§6.2 改造点 2 专门点了这件事）。
   *
   * `after*` **一定执行**，即使本轮已被 `before*` 终止（§6.4.4.1）：用户能配置的是
   * 插件"能做什么"，不是"钩子是否被调用"；少了 after，被短路插件的清理逻辑就没了。
   *
   * 自身耗时**不计入** `llmDurationMs`/`toolsDurationMs`：那两项是给插件测量模型与
   * 工具开销的，把插件自己的开销算进去会污染它要测量的数据（§6.2 第 5 点）。
   */
  const runAfterTurnHooks = async (
    assistant: AssistantMessage,
    toolResults: ToolResultMessage[],
    toolsDurationMs: number,
    currentStep: number
  ): Promise<void> => {
    if (!hooks?.afterTurn) return
    const result = await callHook('afterTurn', () =>
      hooks.afterTurn!({
        step: currentStep,
        message: assistant,
        toolResults,
        // 回执：实际下发的工具名，而不是任何插件的意图
        effectiveToolNames: toolSpecs.map((spec) => spec.function.name),
        llmDurationMs: assistant.durationMs ?? 0,
        toolsDurationMs,
        terminatedByHook: terminateByHook,
        ...hookBase,
      }))
    if (result?.appendNote) {
      // 旁注以用户消息形态进入上下文：AgentMessage 没有 system 角色，而对模型来说它
      // 就是"下一轮多看到一段话"。不写回会话历史（持久化是 store 的事，本层不做）。
      workingMessages.push({
        role: 'user',
        content: `【插件旁注】${result.appendNote}`,
        timestamp: Date.now(),
      })
    }
    if (result?.terminate) {
      terminateAfterTurn = true
      terminateByHook = result.terminateBy ?? terminateByHook
    }
  }

  try {
    for (let step = 0; maxSteps === undefined || step < maxSteps; step++) {
      if (options.signal?.aborted) {
        endReason = 'aborted'
        break
      }

      // 若指定了步数上限，且执行完本步后即达到上限，预设结束原因为 max_steps
      if (maxSteps !== undefined && step + 1 >= maxSteps) {
        endReason = 'max_steps'
      }

      yield { type: 'turn_start' }
      stepsExecuted += 1

      // 轮次开始钩子：可收窄本轮工具表、注入消息、要求"跑完本轮就收尾"。
      // 注入的消息要在构造 llmMessages **之前**落进 workingMessages，本轮请求即生效。
      if (hooks?.beforeTurn) {
        const result = await callHook('beforeTurn', () =>
          hooks.beforeTurn!({
            step,
            messages: [...workingMessages],
            tools: desiredTools,
            ...hookBase,
          }))
        if (result) {
          if (result.extraMessages?.length) workingMessages.push(...result.extraMessages)
          // 只接受数组：契约里 `'casual'` 档位尚未实现，运行层已拦下并记过 trace，
          // 这里再挡一次是为了防第三方 JS 传回意料之外的值
          if (Array.isArray(result.tools)) desiredTools = narrowTools(result.tools)
          if (result.terminate) {
            terminateAfterTurn = true
            terminateByHook = result.terminateBy
          }
        }
      }

      // 本轮的工具表：期望值没变时复用同一份 specs，行为与重构前逐事件等价
      applyTools(desiredTools)

      // 构造当前助手消息容器
      const assistantMessage: AssistantMessage = {
        role: 'assistant',
        content: '',
        thinking: '',
        toolCalls: [],
        timestamp: Date.now(),
      }

      yield { type: 'message_start', message: assistantMessage }

      const llmMessages = convertMessagesToLlm(systemPrompt, workingMessages, {
        supportsImages: config.supportsImages,
        workspace: options.workspace,
      })
      const rawToolCalls: ToolCallBlock[] = []

      // 派发接口请求事件（记录完整请求 Messages、Tools 与参数）
      yield {
        type: 'llm_request',
        model: config.model,
        baseUrl: config.baseUrl,
        messages: llmMessages,
        tools: toolSpecs.length > 0 ? toolSpecs : undefined,
      }

      const stepStartTime = Date.now()
      let stepUsage: TokenUsage | undefined

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
        } else if (chunk.type === 'usage' && chunk.usage) {
          stepUsage = { ...chunk.usage }
          accumulatedTokens.promptTokens += chunk.usage.promptTokens
          accumulatedTokens.completionTokens += chunk.usage.completionTokens
          accumulatedTokens.totalTokens += chunk.usage.totalTokens
          if (chunk.usage.thinkingTokens) {
            accumulatedTokens.thinkingTokens += chunk.usage.thinkingTokens
          }
          if (chunk.usage.cachedTokens) {
            accumulatedTokens.cachedTokens = chunk.usage.cachedTokens
          }

          // 保持单次请求的真实 TokenUsage（含 cachedTokens），避免覆盖为累加值导致统计虚高
          assistantMessage.usage = {
            promptTokens: chunk.usage.promptTokens,
            completionTokens: chunk.usage.completionTokens,
            totalTokens: chunk.usage.totalTokens,
            thinkingTokens: chunk.usage.thinkingTokens,
            cachedTokens: chunk.usage.cachedTokens,
          }
          yield {
            type: 'message_update',
            message: assistantMessage,
            delta: { usage: assistantMessage.usage },
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

      // 记录单次大模型调用的真实耗时与Token数据
      assistantMessage.durationMs = Math.max(1, Date.now() - stepStartTime)
      if (stepUsage) {
        assistantMessage.usage = {
          promptTokens: stepUsage.promptTokens,
          completionTokens: stepUsage.completionTokens,
          totalTokens: stepUsage.totalTokens,
          thinkingTokens: stepUsage.thinkingTokens,
          cachedTokens: stepUsage.cachedTokens,
        }
      } else if (accumulatedTokens.totalTokens > 0) {
        assistantMessage.usage = {
          promptTokens: accumulatedTokens.promptTokens,
          completionTokens: accumulatedTokens.completionTokens,
          totalTokens: accumulatedTokens.totalTokens,
          thinkingTokens: accumulatedTokens.thinkingTokens || undefined,
          cachedTokens: accumulatedTokens.cachedTokens || undefined,
        }
      }

      yield { type: 'message_end', message: assistantMessage }
      yield {
        type: 'llm_response',
        model: config.model,
        message: assistantMessage,
      }
      workingMessages.push(assistantMessage)

      // 如果未产生工具调用，本轮结束
      if (rawToolCalls.length === 0) {
        yield { type: 'turn_end', message: assistantMessage, toolResults: [] }
        await runAfterTurnHooks(assistantMessage, [], 0, step)

        // 插件要求"跑完本轮就收尾"：本轮已经完整跑完（事件都发了），现在才断
        if (terminateAfterTurn) {
          endReason = 'completed'
          break
        }

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

      // 依据执行模式决定串行还是并行。
      //
      // 全局 'sequential' 是保守的默认值：审批一次只该问一件事，命令之间也不该抢
      // 工作目录。但「整批调用都是显式声明 parallel 的安全工具」是另一回事——并发
      // 委派多个只读子智能体正是这种情况，默认值不该把它们压成一条队列。所以这里
      // 只在「全批显式 parallel」时放行重叠，混合批次（写工具 + 子智能体）仍走串行。
      const allExplicitlyParallel =
        rawToolCalls.length > 1 &&
        rawToolCalls.every((c) => toolMap.get(c.name)?.executionMode === 'parallel')
      const isSequential =
        rawToolCalls.some((c) => toolMap.get(c.name)?.executionMode === 'sequential') ||
        (options.toolExecution === 'sequential' && !allExplicitlyParallel)

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
      const toolsStartTime = Date.now()
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

      const toolsDurationMs = Date.now() - toolsStartTime
      yield { type: 'turn_end', message: assistantMessage, toolResults }
      await runAfterTurnHooks(assistantMessage, toolResults, toolsDurationMs, step)

      if (terminateBatch || terminateAfterTurn) {
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
    // 整轮收尾钩子：**放在 finally 里**是为了与 `beforeAgentStart` 严格成对——无论
    // 正常收尾、撞上步数上限、被中止还是中途抛错，事前的初始化都有对应的清理与汇总
    // 时机（§6.0）。`durationMs` 在钩子之前算，不含它自身的开销。
    if (hooks?.afterAgentEnd) {
      const result = await callHook('afterAgentEnd', () =>
        hooks.afterAgentEnd!({
          reason: endReason,
          messages: [...workingMessages],
          stepsExecuted,
          durationMs: Date.now() - loopStartTime,
          ...hookBase,
        }))
      if (result?.appendText) {
        const appended: AssistantMessage = {
          role: 'assistant',
          content: result.appendText,
          thinking: '',
          toolCalls: [],
          timestamp: Date.now(),
        }
        // 最终回复早已流式送达界面，原地改写已渲染内容不可能，所以"追加一段文本"
        // 以**再发一条助手消息**的方式交付——这是本项目唯一诚实的做法（见 events.ts）。
        yield { type: 'message_start', message: appended }
        yield { type: 'message_update', message: appended, delta: { text: result.appendText } }
        yield { type: 'message_end', message: appended }
        workingMessages.push(appended)
      }
    }
    yield { type: 'agent_end', messages: workingMessages, reason: endReason }
  }

  return workingMessages
}
