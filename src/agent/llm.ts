/**
 * The model client.
 *
 * Any OpenAI-compatible `/chat/completions` endpoint works. Where the endpoint
 * comes from lives in `config.ts`, so the settings dialog and a running turn
 * read the same rules.
 */

import type { LlmConfig } from './config'
import type { ChatMessage, ToolCall } from './types'

export type StreamEvent =
  | { type: 'text'; text: string }
  | { type: 'tools'; calls: ToolCall[] }

interface DeltaToolCall {
  index?: number
  id?: string
  function?: { name?: string; arguments?: string }
}

export async function* streamChat(
  config: LlmConfig,
  messages: ChatMessage[],
  options: { tools: unknown[]; effort: string; signal: AbortSignal },
): AsyncGenerator<StreamEvent> {
  const body: Record<string, unknown> = {
    model: config.model,
    messages,
    stream: true,
  }
  if (options.tools.length) {
    body.tools = options.tools
    body.tool_choice = 'auto'
  }
  if (options.effort && options.effort !== 'default') body.reasoning_effort = options.effort

  const response = await fetch(`${config.baseUrl}/chat/completions`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      authorization: `Bearer ${config.apiKey}`,
    },
    body: JSON.stringify(body),
    signal: options.signal,
  })
  if (!response.ok) {
    const detail = await response.text().catch(() => '')
    throw new Error(`模型接口返回 ${response.status}：${detail.slice(0, 400)}`)
  }
  if (!response.body) throw new Error('模型接口没有返回流式内容')

  const pending = new Map<number, { id: string; name: string; args: string }>()
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })
      let boundary = buffer.indexOf('\n\n')
      while (boundary >= 0) {
        const chunk = buffer.slice(0, boundary)
        buffer = buffer.slice(boundary + 2)
        boundary = buffer.indexOf('\n\n')
        for (const line of chunk.split('\n')) {
          if (!line.startsWith('data:')) continue
          const payload = line.slice(5).trim()
          if (!payload || payload === '[DONE]') continue
          let parsed: any
          try {
            parsed = JSON.parse(payload)
          } catch {
            continue
          }
          const delta = parsed?.choices?.[0]?.delta
          if (!delta) continue
          const text = typeof delta.content === 'string' ? delta.content : ''
          if (text) yield { type: 'text', text }
          const calls = delta.tool_calls as DeltaToolCall[] | undefined
          if (Array.isArray(calls)) {
            for (const call of calls) {
              const index = call.index ?? 0
              const current = pending.get(index) ?? { id: '', name: '', args: '' }
              if (call.id) current.id = call.id
              if (call.function?.name) current.name = call.function.name
              if (call.function?.arguments) current.args += call.function.arguments
              pending.set(index, current)
            }
          }
        }
      }
    }
  } finally {
    reader.releaseLock()
  }

  const calls = [...pending.entries()]
    .sort((a, b) => a[0] - b[0])
    .map(([, call]) => call)
    .filter((call) => call.name)
    .map<ToolCall>((call, index) => ({
      id: call.id || `call_${index}_${call.name}`,
      name: call.name,
      args: call.args || '{}',
    }))
  if (calls.length) yield { type: 'tools', calls }
}

export function parseToolArgs(raw: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(raw || '{}')
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {}
  } catch {
    return {}
  }
}
