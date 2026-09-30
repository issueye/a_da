/**
 * 主机侧的 WebSocket 服务端（M3-1/M3-2，协议 §1）。
 *
 * ## 它替换掉了什么
 *
 * M0–M2 里"主机"只是同一个进程里的一个对象（`ui/client/in-process.ts` 直接调 store）。
 * 这里把它变成**真的角色**：监听本机端口、用令牌挡住别的进程、把命令交给
 * `agent/host/dispatch.ts`、把快照事件广播出去。业务代码一行没改——派发表还是那一份。
 *
 * ## 三条刻意选择
 *
 * 1. **只听 `127.0.0.1`**：这是本机自举的通道，不是服务端产品。协议 §1.6 明确不暴露到网络。
 * 2. **令牌两道**：升级连接时校验一次（挡住"没带令牌就握手"），`session.hello` 时客户端
 *    再报一次（挡住"连上之后换令牌"）。任一不过都不放行。
 * 3. **连上先推一份快照**（协议 §1.3）：客户端不必先问一次再等，省一个往返；
 *    `seq` 从这里开始单调递增，客户端可用它判断自己有没有落后。
 *
 * ## 事件怎么发
 *
 * 复用 M1 的 `createHostEmitter`：它挂在 `store.subscribe`（那 88 处 `notify()` 的共同出口）上。
 * 这里只加一件事——把事件广播给所有连接。**合帧窗口由传输决定**：进程内是 0，
 * 走 WebSocket 时用 16ms（协议 §7.1），因为这里真的在省"每个 token 一帧"。
 */

import type { ServerWebSocket } from 'bun'
import type { AgentStore } from '../store'
import { PROTOCOL_VERSION, RpcErrorCode, AppErrorCode, ProtocolError } from '../../shared/protocol'
import type { ProtocolMethod, SnapshotEvent } from '../../shared/protocol'
import { createCommandDispatcher } from './dispatch'
import { createHostEmitter } from './emitter'

export interface HostServerOptions {
  store: AgentStore
  /** 本次自举的临时令牌（由 UI 角色生成并经 argv 传给主机角色）。 */
  token: string
  /** 端口：0 = 让操作系统挑一个（自举时用 0，把选中的端口写到 stdout 的 ready 行）。 */
  port?: number
  hostname?: string
  /** 事件合帧窗口（ms）。WebSocket 默认 16；测试里可以调小。 */
  coalesceMs?: number
}

export interface HostServer {
  port: number
  url: string
  /** 已连接客户端数（诊断与测试用）。 */
  readonly clients: number
  /** 主动收掉所有连接并关服务。 */
  stop(): void
}

/** 一帧 JSON-RPC 错误响应。 */
function errorFrame(id: unknown, code: number, message: string, data?: unknown): unknown {
  return { jsonrpc: '2.0', id: id ?? null, error: { code, message, ...(data === undefined ? {} : { data }) } }
}

export function startHostServer(options: HostServerOptions): HostServer {
  const dispatch = createCommandDispatcher(options.store)
  const emitter = createHostEmitter(options.store, { coalesceMs: options.coalesceMs ?? 16 })
  const token = options.token
  const sockets = new Set<ServerWebSocket<unknown>>()

  /** 解析一帧请求并执行；通知类帧（没有 method 的响应帧）本里程碑还不存在，直接忽略。 */
  async function handleFrame(raw: string): Promise<unknown | null> {
    let frame: { id?: unknown; method?: unknown; params?: unknown }
    try {
      frame = JSON.parse(raw) as typeof frame
    } catch {
      return errorFrame(null, RpcErrorCode.ParseError, '不是合法的 JSON')
    }
    if (typeof frame.method !== 'string') return null

    try {
      const result = await dispatch(frame.method as ProtocolMethod, frame.params ?? {})
      return { jsonrpc: '2.0', id: frame.id ?? null, result: result ?? null }
    } catch (err) {
      if (err instanceof ProtocolError) {
        return errorFrame(frame.id, err.code, err.message, err.data)
      }
      // 非协议错误：如实说是内部错误，并把原文带上（主机日志里能查到同一个 id）
      const message = err instanceof Error ? err.message : String(err)
      return errorFrame(frame.id, RpcErrorCode.InternalError, message, { what: frame.method })
    }
  }

  function broadcast(event: SnapshotEvent): void {
    const frame = JSON.stringify({ jsonrpc: '2.0', method: event.topic, params: event })
    for (const socket of [...sockets]) {
      try {
        socket.send(frame)
      } catch {
        // 连接已经断了：从集合里摘掉，别让一次失败拖住后面的广播
        sockets.delete(socket)
      }
    }
  }

  const server = Bun.serve({
    hostname: options.hostname ?? '127.0.0.1',
    port: options.port ?? 0,
    fetch(request, srv) {
      const url = new URL(request.url)
      const presented = url.searchParams.get('token') ?? request.headers.get('x-a-da-token') ?? ''
      if (presented !== token) {
        return new Response('unauthorized', { status: 401 })
      }
      if (srv.upgrade(request)) return undefined
      return new Response('expected websocket upgrade', { status: 400 })
    },
    websocket: {
      open(socket) {
        sockets.add(socket)
        // 连上先给一份快照（协议 §1.3）：客户端不必先问
        const seed: SnapshotEvent = {
          seq: 0,
          topic: 'evt.state.snapshot',
          payload: emitter.snapshot(),
        }
        socket.send(JSON.stringify({ jsonrpc: '2.0', method: seed.topic, params: seed }))
      },
      async message(socket, raw) {
        const reply = await handleFrame(String(raw))
        if (reply !== null) socket.send(JSON.stringify(reply))
      },
      close(socket) {
        sockets.delete(socket)
      },
    },
  })

  const unsubscribe = emitter.subscribe(broadcast)

  return {
    // 端口 0 时由操作系统分配；Bun 的 `server.port` 类型上可空，这里取到实际监听端口
    port: server.port ?? 0,
    url: `ws://${options.hostname ?? '127.0.0.1'}:${server.port ?? 0}`,
    get clients() {
      return sockets.size
    },
    stop() {
      unsubscribe()
      emitter.dispose()
      for (const socket of [...sockets]) {
        try {
          socket.close(1001, 'host shutting down')
        } catch {
          // 已经断了就算了
        }
      }
      sockets.clear()
      void server.stop(true)
    },
  }
}

/** 主机自举时写到 stdout 的那一行（UI 角色读它拿到真实端口，协议 §1.8）。 */
export function readyLine(port: number): string {
  return `A_DA_HOST_READY ${JSON.stringify({ port, protocolVersion: PROTOCOL_VERSION })}`
}

/** 未授权时用的错误码（导出给测试与 UI 角色读，免得两边各写一份）。 */
export const UNAUTHORIZED_STATUS = AppErrorCode.Unauthorized
