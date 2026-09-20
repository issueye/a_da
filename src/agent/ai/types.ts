/**
 * 统一 AI 模型客户端类型定义
 * 参考 @earendil-works/pi-ai 架构
 */

export interface TokenUsage {
  promptTokens: number
  completionTokens: number
  totalTokens: number
  thinkingTokens?: number
}

export interface StreamDelta {
  type: 'text' | 'thinking' | 'tool_call' | 'usage' | 'done' | 'error'
  text?: string
  thinking?: string
  call?: {
    id: string
    name: string
    args: string
  }
  usage?: TokenUsage
  stopReason?: 'stop' | 'tool_calls' | 'length' | 'error' | 'aborted'
  error?: string
}

export interface ModelChatOptions {
  tools?: Array<{
    type: 'function'
    function: {
      name: string
      description: string
      parameters: Record<string, unknown>
    }
  }>
  systemPrompt?: string
  effort?: string
  signal?: AbortSignal
  temperature?: number
}
