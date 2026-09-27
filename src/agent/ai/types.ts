/**
 * 统一 AI 模型客户端类型定义
 * 参考 @earendil-works/pi-ai 架构
 */

export interface TokenUsage {
  promptTokens: number
  completionTokens: number
  totalTokens: number
  thinkingTokens?: number
  cachedTokens?: number
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
  /** 请求阶段的自动重试；仅在响应头到达之前生效，流已开始后绝不重发。 */
  retry?: {
    /** 最多重试几次（默认 3，0 表示不重试）。 */
    maxRetries?: number
    /** 首次重试的基础等待毫秒数（默认 800，之后指数退避）。 */
    baseDelayMs?: number
  }
}
