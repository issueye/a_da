/**
 * 协议错误码（JSON-RPC 2.0 标准码 + 应用级 `-32000..-32012`）。
 *
 * 规则（协议 §6）：应用级错误的 `data` **必须结构化**——界面要能直接说人话，而不是显示一句
 * "内部错误"。因此每个错误都配一个 `data` 形状。
 */

/** JSON-RPC 2.0 标准错误码。 */
export const RpcErrorCode = {
  ParseError: -32700,
  InvalidRequest: -32600,
  MethodNotFound: -32601,
  InvalidParams: -32602,
  InternalError: -32603,
} as const

/** 应用级错误码（-32000..-32099，协议 §6 的表）。 */
export const AppErrorCode = {
  ProtocolVersionMismatch: -32000,
  Unauthorized: -32001,
  NotFound: -32002,
  NotReady: -32003,
  Denied: -32004,
  Timeout: -32005,
  AlreadyAnswered: -32006,
  WorkspaceDenied: -32007,
  TooLarge: -32008,
  Cancelled: -32009,
  NeedResync: -32010,
  Busy: -32011,
  ConfigInvalid: -32012,
} as const

export type AppErrorName = keyof typeof AppErrorCode
export type RpcErrorName = keyof typeof RpcErrorCode

/** 各错误码的 `data` 形状：调用方与界面都按这个读。 */
export interface AppErrorData {
  ProtocolVersionMismatch: { serverVersion: string; minClient?: string }
  Unauthorized: Record<string, never>
  NotFound: { kind: string; id: string }
  NotReady: { what: string; retryAfterMs?: number }
  Denied: { reason?: string; gate?: unknown }
  Timeout: { callId?: string; timeoutMs?: number }
  AlreadyAnswered: { callId: string; by?: string }
  WorkspaceDenied: { path: string; workspace: string }
  TooLarge: { limitBytes: number; gotBytes: number }
  Cancelled: { id: number | string }
  NeedResync: { fromSeq: number; currentSeq: number }
  Busy: { threadId: string; runningCommand?: string }
  ConfigInvalid: { key: string; invalid: string[] }
}

/** 抛给调用方的协议错误（主机侧构造，客户端按 `code`/`data` 处理）。 */
export class ProtocolError extends Error {
  readonly code: number
  readonly data: unknown

  constructor(code: number, message: string, data?: unknown) {
    super(message)
    this.name = 'ProtocolError'
    this.code = code
    this.data = data
  }
}

/** 便捷构造：应用级错误。 */
export function appError<N extends AppErrorName>(
  name: N,
  message: string,
  data: AppErrorData[N]
): ProtocolError {
  return new ProtocolError(AppErrorCode[name], message, data)
}
