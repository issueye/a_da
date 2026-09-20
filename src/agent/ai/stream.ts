/**
 * 统一流式 AI 请求引擎
 * 兼容 OpenAI 兼容端点、DeepSeek (含 reasoning_content 思考链) 及主流大模型供应商
 * 参考 @earendil-works/pi-ai 的 streamSimple 与事件聚合设计
 */

import { createParser, type EventSourceMessage } from 'eventsource-parser'
import type { ProviderConfig } from '../config'
import type { ModelChatOptions, StreamDelta, TokenUsage } from './types'

export interface ChatCompletionMessageParam {
  role: 'system' | 'user' | 'assistant' | 'tool'
  content?: string | null
  tool_calls?: Array<{
    id: string
    type: 'function'
    function: { name: string; arguments: string }
  }>
  tool_call_id?: string
}

/**
 * 统一发起流式会话
 */
export async function* streamModelChat(
  config: ProviderConfig,
  messages: ChatCompletionMessageParam[],
  options: ModelChatOptions = {}
): AsyncGenerator<StreamDelta, void, unknown> {
  const url = `${config.baseUrl.replace(/\/+$/, '')}/chat/completions`

  const body: Record<string, unknown> = {
    model: config.model,
    messages,
    stream: true,
  }

  if (options.tools && options.tools.length > 0) {
    body.tools = options.tools
  }

  if (options.effort) {
    body.reasoning_effort = options.effort
  }

  if (options.temperature !== undefined) {
    body.temperature = options.temperature
  }

  let response: Response
  try {
    response = await fetch(url, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        authorization: `Bearer ${config.apiKey}`,
      },
      body: JSON.stringify(body),
      signal: options.signal,
    })
  } catch (error) {
    if ((error as Error).name === 'AbortError') {
      yield { type: 'done', stopReason: 'aborted' }
      return
    }
    yield { type: 'error', error: `网络请求失败：${(error as Error).message}` }
    return
  }

  if (!response.ok) {
    const errorText = await response.text().catch(() => '')
    let message = `API 错误（HTTP ${response.status}）`
    try {
      const parsed = JSON.parse(errorText)
      if (parsed.error?.message) {
        message += `: ${parsed.error.message}`
      } else if (errorText) {
        message += `: ${errorText.slice(0, 300)}`
      }
    } catch {
      if (errorText) message += `: ${errorText.slice(0, 300)}`
    }
    yield { type: 'error', error: message }
    return
  }

  if (!response.body) {
    yield { type: 'error', error: '模型响应为空正文' }
    return
  }

  // 工具调用块的增量聚合缓存
  const activeToolCalls = new Map<number, { id: string; name: string; args: string }>()
  let finalStopReason: 'stop' | 'tool_calls' | 'length' | 'error' | 'aborted' = 'stop'

  const queue: StreamDelta[] = []
  let streamError: string | null = null

  const parser = createParser({
    onEvent(event: EventSourceMessage) {
      if (!event.data || event.data === '[DONE]') return

      try {
        const chunk = JSON.parse(event.data)

        // 提取 usage 统计（若提供）
        if (chunk.usage) {
          const usage: TokenUsage = {
            promptTokens: chunk.usage.prompt_tokens ?? 0,
            completionTokens: chunk.usage.completion_tokens ?? 0,
            totalTokens: chunk.usage.total_tokens ?? 0,
            thinkingTokens: chunk.usage.completion_tokens_details?.reasoning_tokens,
          }
          queue.push({ type: 'usage', usage })
        }

        const choice = chunk.choices?.[0]
        if (!choice) return

        if (choice.finish_reason) {
          if (choice.finish_reason === 'tool_calls' || choice.finish_reason === 'function_call') {
            finalStopReason = 'tool_calls'
          } else if (choice.finish_reason === 'length') {
            finalStopReason = 'length'
          } else {
            finalStopReason = 'stop'
          }
        }

        const delta = choice.delta
        if (!delta) return

        // 1. 处理思考链 / Reasoning (DeepSeek-R1 / OpenAI reasoning_content)
        const thinking = delta.reasoning_content || delta.reasoning
        if (thinking) {
          queue.push({ type: 'thinking', thinking })
        }

        // 2. 处理常规正文增量
        if (delta.content) {
          queue.push({ type: 'text', text: delta.content })
        }

        // 3. 处理工具调用增量 (tool_calls)
        if (delta.tool_calls && Array.isArray(delta.tool_calls)) {
          for (const tc of delta.tool_calls) {
            const index = tc.index ?? 0
            let record = activeToolCalls.get(index)
            if (!record) {
              record = { id: tc.id || `call_${Date.now()}_${index}`, name: '', args: '' }
              activeToolCalls.set(index, record)
            }
            if (tc.id) record.id = tc.id
            if (tc.function?.name) record.name = tc.function.name
            if (tc.function?.arguments) record.args += tc.function.arguments
          }
        }
      } catch (err) {
        // 忽略非 JSON 行
      }
    },
    onError(err) {
      streamError = `SSE 解析错误: ${err}`
    },
  })

  const reader = response.body.getReader()
  const decoder = new TextDecoder()

  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break

      parser.feed(decoder.decode(value, { stream: true }))

      while (queue.length > 0) {
        yield queue.shift()!
      }
      if (streamError) {
        yield { type: 'error', error: streamError }
        return
      }
    }
  } catch (error) {
    if ((error as Error).name === 'AbortError') {
      yield { type: 'done', stopReason: 'aborted' }
      return
    }
    yield { type: 'error', error: `流读取异常：${(error as Error).message}` }
    return
  }

  // 结算所有聚合后的工具调用
  for (const call of activeToolCalls.values()) {
    if (call.name) {
      yield {
        type: 'tool_call',
        call: {
          id: call.id,
          name: call.name,
          args: call.args,
        },
      }
    }
  }

  yield { type: 'done', stopReason: activeToolCalls.size > 0 ? 'tool_calls' : finalStopReason }
}
