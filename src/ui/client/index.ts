/**
 * 客户端层的统一出口。
 *
 * **只有这一层允许 import `agent/store`**（M0 的适配器）；`src/ui/**` 的组件一律从这里取
 * 类型与单例——守门测试 `src/ui/protocol-boundary.test.ts` 盯着这条线。
 */

import { store } from '../../agent/store'
import type { AgentClient } from './types'
import { createInProcessClient } from './in-process'

export type {
  AgentClient,
  ClientConnectionState,
  ClientState,
  ConfirmOptions,
  FilePickerRequest,
  UiActions,
} from './types'
/**
 * 协议类型对 UI 的再导出：组件需要 `Thread`/`Item`/`FileChange` 这类形状时从这里拿，
 * 不必（也不许）去 import `agent/**` 的实现模块。
 */
export type {
  AgentMessage,
  AgentMode,
  AgentQuestion,
  ApprovalMode,
  BuiltinToolInfo,
  ClientSnapshot,
  DebugEntry,
  Effort,
  FileChange,
  FsEntry,
  FsListing,
  FsRoot,
  Item,
  PluginCapabilities,
  PluginItem,
  PluginToolInfo,
  PromptItem,
  ProviderConfig,
  ProviderPreset,
  QueuedItem,
  SkillSummary,
  SnapshotEvent,
  SubagentProfile,
  Thread,
  ThreadStats,
  WorkspaceInfo,
} from '../../shared/protocol'
export { createInProcessClient }

/**
 * 应用用的客户端单例。
 *
 * M0/M1/M2 是**进程内**直连 store 单例；M3 起按 Transport 决定（打包 exe 走 WebSocket，
 * 开发与测试仍走进程内，见 `docs/ui-host-split-dev-plan.md` M3-6）。
 */
export const agentClient = createInProcessClient(store)

/**
 * 界面要用、但形状仍属于实现侧的小常量表，在这里转发一次——
 * 这样组件不必（也不许）直接 import `agent/store` 或 `agent/config`。
 */
export { APPROVAL_OPTIONS, EFFORT_OPTIONS } from '../../agent/store'
/**
 * 能力开关的**默认值**：界面首帧占位用（那一帧还没有 `plugin.list` 的结果）。
 * 真实值一律来自 `plugin.list` 返回的 `capabilities`——这里是只读镜像，不是第二份真相。
 */
export { DEFAULT_PLUGIN_CAPABILITIES } from '../../agent/config'

/**
 * 选传输并造出客户端（M3-6）。
 *
 * - `inprocess`（默认）：进程内直连主机派发表。开发与测试走这条——不起进程、不起端口，
 *   真窗口的单窗口约束与测试速度都不受影响（协议 §1.8"开发与测试"）。
 * - `ws`：**自己 spawn 自己当主机**（同一个二进制带 `--host`），然后走 WebSocket。
 *   打包形态走这条：交付物仍是一个 exe，用户无感。
 *
 * **默认规则**：`A_DA_TRANSPORT` 显式指定优先；否则**独立可执行**（`bun build --compile`
 * 的产物，用 `Bun.isStandaloneExecutable` 判）走 `ws`，其余（`bun run dev`、`bun test`）走
 * `inprocess`。这正是协议 §1.8 说的"本机也走 WS，但开发与测试不 spawn"。
 *
 * 返回值里的 `info` 是给诊断与测试看的（走的哪条、端口与 pid 是多少）；
 * `shutdown` 必须在应用退出前调用——否则会留下主机进程（协议 §1.8 的生命周期约定）。
 */
export async function resolveAgentClient(
  options: { transport?: 'inprocess' | 'ws' } = {}
): Promise<{
  client: AgentClient
  shutdown: () => void
  info: { transport: 'inprocess' | 'ws'; port?: number; pid?: number; url?: string }
}> {
  const explicit = process.env.A_DA_TRANSPORT
  const fallback: 'inprocess' | 'ws' = Bun.isStandaloneExecutable ? 'ws' : 'inprocess'
  const transport =
    options.transport ?? (explicit === 'ws' || explicit === 'inprocess' ? explicit : fallback)

  if (transport === 'inprocess') {
    return { client: agentClient, shutdown: () => {}, info: { transport: 'inprocess' } }
  }

  // 动态 import：进程内模式不该顺带把自举与 ws 客户端加载进来
  const [{ spawnHostProcess }, { createWebSocketClient }] = await Promise.all([
    import('./host-bootstrap'),
    import('./ws'),
  ])

  const host = await spawnHostProcess()
  const client = createWebSocketClient({ url: host.url, token: host.token })
  try {
    await client.ready()
  } catch (err) {
    // 连不上就把主机收掉再报错，别留一个孤儿进程
    host.stop()
    throw err
  }

  let stopped = false
  const shutdown = (): void => {
    if (stopped) return
    stopped = true
    client.close()
    host.stop()
  }
  // 兜底：不管从哪条路径退出，都别留下主机
  process.once('exit', shutdown)
  process.once('SIGINT', shutdown)
  process.once('SIGTERM', shutdown)

  return {
    client,
    shutdown,
    info: { transport: 'ws', port: host.port, pid: host.pid, url: host.url },
  }
}
