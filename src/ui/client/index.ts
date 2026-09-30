/**
 * 客户端层的统一出口。
 *
 * **只有这一层允许 import `agent/store`**（M0 的适配器）；`src/ui/**` 的组件一律从这里取
 * 类型与单例——守门测试 `src/ui/protocol-boundary.test.ts` 盯着这条线。
 */

import { store } from '../../agent/store'
import { createInProcessClient } from './in-process'

export type { AgentClient, ClientState, ConfirmOptions, UiActions } from './types'
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
 * 这样组件不必（也不许）直接 import `agent/store`。
 */
export { APPROVAL_OPTIONS, EFFORT_OPTIONS } from '../../agent/store'
