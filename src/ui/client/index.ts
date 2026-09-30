/**
 * 客户端层的统一出口。
 *
 * **只有这一层允许 import `agent/store`**（M0 的适配器）；`src/ui/**` 的组件一律从这里取
 * 类型与单例——守门测试 `src/ui/protocol-boundary.test.ts` 盯着这条线。
 */

import { store } from '../../agent/store'
import { createInProcessClient } from './in-process'

export type { AgentClient, ClientState, ConfirmOptions, UiActions } from './types'
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
