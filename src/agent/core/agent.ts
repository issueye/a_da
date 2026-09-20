/**
 * Agent 状态机与控制器
 * 参考 @earendil-works/pi-agent-core/src/agent.ts 设计
 */

import type { ProviderConfig } from '../config'
import { runAgentLoop } from './agent-loop'
import type {
  AgentEvent,
  AgentLoopOptions,
  AgentMessage,
  AgentState,
  AgentTool,
  AssistantMessage,
  BeforeToolCallContext,
  BeforeToolCallResult,
  AfterToolCallContext,
  AfterToolCallResult,
  ShouldStopAfterTurnContext,
  ToolExecutionMode,
} from './types'

export interface AgentOptions {
  initialMessages?: AgentMessage[]
  systemPrompt?: string
  tools?: AgentTool[]
  /** 传给接口的 reasoning_effort。 */
  effort?: string
  toolExecution?: ToolExecutionMode
  beforeToolCall?: (context: BeforeToolCallContext, signal?: AbortSignal) => Promise<BeforeToolCallResult | undefined>
  afterToolCall?: (context: AfterToolCallContext, signal?: AbortSignal) => Promise<AfterToolCallResult | undefined>
  shouldStopAfterTurn?: (context: ShouldStopAfterTurnContext, signal?: AbortSignal) => boolean | Promise<boolean>
}

export class Agent implements AgentState {
  systemPrompt: string
  tools: AgentTool[]
  messages: AgentMessage[] = []

  private _isStreaming = false
  private _pendingToolCalls = new Set<string>()
  private _errorMessage?: string

  private listeners = new Set<(event: AgentEvent) => void>()
  private abortController: AbortController | null = null
  private steeringQueue: AgentMessage[] = []
  private followUpQueue: AgentMessage[] = []
  private options: AgentOptions

  constructor(options: AgentOptions = {}) {
    this.options = options
    this.systemPrompt = options.systemPrompt ?? ''
    this.tools = options.tools ?? []
    this.messages = options.initialMessages ? [...options.initialMessages] : []
  }

  get isStreaming(): boolean {
    return this._isStreaming
  }

  get pendingToolCalls(): ReadonlySet<string> {
    return this._pendingToolCalls
  }

  get errorMessage(): string | undefined {
    return this._errorMessage
  }

  /**
   * 订阅 Agent 生命周期事件
   */
  subscribe(listener: (event: AgentEvent) => void): () => void {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  private emit(event: AgentEvent): void {
    for (const listener of this.listeners) {
      try {
        listener(event)
      } catch (err) {
        console.error('[Agent event listener error]', err)
      }
    }
  }

  /**
   * 中止当前正在运行的任务
   */
  abort(): void {
    if (this.abortController) {
      this.abortController.abort()
      this.abortController = null
    }
  }

  /**
   * 添加转向指令 (在下一轮思考前优先插入)
   */
  steer(text: string): void {
    this.steeringQueue.push({
      role: 'user',
      content: text,
      timestamp: Date.now(),
    })
  }

  /**
   * 添加后续任务 (在没有工具调用且结束时继续排队)
   */
  followUp(text: string): void {
    this.followUpQueue.push({
      role: 'user',
      content: text,
      timestamp: Date.now(),
    })
  }

  /**
   * 发起一次新的会话轮次
   */
  async prompt(text: string, config: ProviderConfig): Promise<AgentMessage[]> {
    if (this._isStreaming) {
      throw new Error('Agent 正在运行中，请等待完成或调用 abort()。')
    }

    const userMessage: AgentMessage = {
      role: 'user',
      content: text,
      timestamp: Date.now(),
    }
    this.messages.push(userMessage)

    return this.run(config)
  }

  /**
   * 从当前上下文继续执行 (如重试或从上次中断恢复)
   */
  async continue(config: ProviderConfig): Promise<AgentMessage[]> {
    if (this._isStreaming) {
      throw new Error('Agent 正在运行中。')
    }
    return this.run(config)
  }

  private async run(config: ProviderConfig): Promise<AgentMessage[]> {
    this._isStreaming = true
    this._errorMessage = undefined
    this._pendingToolCalls.clear()
    this.abortController = new AbortController()

    const loopOptions: AgentLoopOptions = {
      systemPrompt: this.systemPrompt,
      tools: this.tools,
      effort: this.options.effort,
      toolExecution: this.options.toolExecution ?? 'parallel',
      signal: this.abortController.signal,
      beforeToolCall: this.options.beforeToolCall,
      afterToolCall: this.options.afterToolCall,
      shouldStopAfterTurn: this.options.shouldStopAfterTurn,
      getSteeringMessages: async () => {
        const msgs = [...this.steeringQueue]
        this.steeringQueue = []
        return msgs
      },
      getFollowUpMessages: async () => {
        const msgs = [...this.followUpQueue]
        this.followUpQueue = []
        return msgs
      },
    }

    try {
      for await (const event of runAgentLoop(this.messages, config, loopOptions)) {
        // 维护内部状态
        if (event.type === 'tool_execution_start') {
          this._pendingToolCalls.add(event.toolCallId)
        } else if (event.type === 'tool_execution_end') {
          this._pendingToolCalls.delete(event.toolCallId)
        } else if (event.type === 'turn_end') {
          if (event.message.stopReason === 'error') {
            this._errorMessage = event.message.errorMessage
          }
        } else if (event.type === 'agent_end') {
          this.messages = event.messages
        }

        // 分发给外部订阅者
        this.emit(event)
      }
    } finally {
      this._isStreaming = false
      this._pendingToolCalls.clear()
      this.abortController = null
    }

    return this.messages
  }
}
