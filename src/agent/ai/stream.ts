/**
 * 统一流式 AI 请求引擎
 * 兼容 OpenAI 兼容端点、DeepSeek (含 reasoning_content 思考链) 及主流大模型供应商
 * 参考 @earendil-works/pi-ai 的 streamSimple 与事件聚合设计
 */

import { createParser, type EventSourceMessage } from 'eventsource-parser'
import type { ProviderConfig } from '../config'
import type { ModelChatOptions, StreamDelta, TokenUsage } from './types'

export interface ChatCompletionContentPartText {
  type: 'text'
  text: string
}

export interface ChatCompletionContentPartImage {
  type: 'image_url'
  image_url: {
    url: string
    detail?: 'auto' | 'low' | 'high'
  }
}

export type ChatCompletionContentPart = ChatCompletionContentPartText | ChatCompletionContentPartImage

export interface ChatCompletionMessageParam {
  role: 'system' | 'user' | 'assistant' | 'tool'
  content?: string | ChatCompletionContentPart[] | null
  tool_calls?: Array<{
    id: string
    type: 'function'
    function: { name: string; arguments: string }
  }>
  tool_call_id?: string
}

/**
 * 流式提取正文中的 `<think>...</think>` 标签，将其分离为 thinking 与 text 增量。
 * 兼容本地 Ollama、vLLM、LM Studio 等直接在 content 字段输出思考标签的模型。
 */
export class ThinkTagFilter {
  private inThink = false
  private buffer = ''

  feed(content: string): Array<{ type: 'thinking' | 'text'; text: string }> {
    const results: Array<{ type: 'thinking' | 'text'; text: string }> = []
    let text = this.buffer + content
    this.buffer = ''

    while (text.length > 0) {
      if (!this.inThink) {
        const startIdx = text.toLowerCase().indexOf('<think>')
        if (startIdx >= 0) {
          if (startIdx > 0) {
            results.push({ type: 'text', text: text.slice(0, startIdx) })
          }
          this.inThink = true
          text = text.slice(startIdx + 7)
        } else {
          // 检查结尾是否可能是不完整的 `<think>` 标签前缀（如 `<th`）
          const match = text.match(/<t(?:h(?:i(?:n(?:k)?)?)?)?$/i)
          if (match && match.index !== undefined) {
            this.buffer = text.slice(match.index)
            const safe = text.slice(0, match.index)
            if (safe) results.push({ type: 'text', text: safe })
            text = ''
          } else {
            results.push({ type: 'text', text })
            text = ''
          }
        }
      } else {
        const endIdx = text.toLowerCase().indexOf('</think>')
        if (endIdx >= 0) {
          if (endIdx > 0) {
            results.push({ type: 'thinking', text: text.slice(0, endIdx) })
          }
          this.inThink = false
          text = text.slice(endIdx + 8)
        } else {
          // 检查结尾是否可能是不完整的 `</think>` 标签前缀
          const match = text.match(/<\/(?:t(?:h(?:i(?:n(?:k)?)?)?)?)?$/i)
          if (match && match.index !== undefined) {
            this.buffer = text.slice(match.index)
            const safe = text.slice(0, match.index)
            if (safe) results.push({ type: 'thinking', text: safe })
            text = ''
          } else {
            results.push({ type: 'thinking', text })
            text = ''
          }
        }
      }
    }

    return results
  }

  flush(): Array<{ type: 'thinking' | 'text'; text: string }> {
    if (!this.buffer) return []
    const res = [
      {
        type: this.inThink ? ('thinking' as const) : ('text' as const),
        text: this.buffer,
      },
    ]
    this.buffer = ''
    return res
  }
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
    stream_options: { include_usage: true },
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
      } else if (typeof parsed.message === 'string' && parsed.message) {
        message += `: ${parsed.message}`
        if (parsed.code) message += ` (${parsed.code})`
        if (parsed.data?.detail) {
          try {
            const detailObj = typeof parsed.data.detail === 'string' ? JSON.parse(parsed.data.detail) : parsed.data.detail
            if (detailObj.error?.message && detailObj.error.message !== parsed.message) {
              message += ` [${detailObj.error.message}]`
            }
          } catch {}
        }
      } else if (typeof parsed.error === 'string' && parsed.error) {
        message += `: ${parsed.error}`
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
  let hasUsage = false
  let accumulatedOutput = ''
  let accumulatedThinking = ''
  const thinkFilter = new ThinkTagFilter()

  const parser = createParser({
    onEvent(event: EventSourceMessage) {
      if (!event.data || event.data === '[DONE]') return

      try {
        const chunk = JSON.parse(event.data)

        // 提取 usage 统计（若提供）
        if (chunk.usage) {
          hasUsage = true
          const cachedTokens =
            chunk.usage.prompt_tokens_details?.cached_tokens ??
            chunk.usage.prompt_cache_hit_tokens ??
            chunk.usage.cache_read_input_tokens ??
            0
          const usage: TokenUsage = {
            promptTokens: chunk.usage.prompt_tokens ?? 0,
            completionTokens: chunk.usage.completion_tokens ?? 0,
            totalTokens: chunk.usage.total_tokens ?? 0,
            thinkingTokens: chunk.usage.completion_tokens_details?.reasoning_tokens,
            cachedTokens: cachedTokens > 0 ? cachedTokens : undefined,
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

        // 1. 处理思考链 / Reasoning (DeepSeek-R1 / OpenAI reasoning_content / Anthropic thinking)
        const thinking = delta.reasoning_content || delta.reasoning || delta.thinking
        if (thinking) {
          accumulatedThinking += thinking
          queue.push({ type: 'thinking', thinking })
        }

        // 2. 处理常规正文增量（支持提取正文内嵌的 <think>...</think> 标签）
        if (delta.content) {
          const parts = thinkFilter.feed(delta.content)
          for (const part of parts) {
            if (part.type === 'thinking') {
              accumulatedThinking += part.text
              queue.push({ type: 'thinking', thinking: part.text })
            } else if (part.text) {
              accumulatedOutput += part.text
              queue.push({ type: 'text', text: part.text })
            }
          }
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

  // 结算可能残留在 thinkFilter 缓冲区中的思考或文本
  for (const flushed of thinkFilter.flush()) {
    if (flushed.type === 'thinking') {
      accumulatedThinking += flushed.text
      yield { type: 'thinking', thinking: flushed.text }
    } else if (flushed.text) {
      accumulatedOutput += flushed.text
      yield { type: 'text', text: flushed.text }
    }
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
