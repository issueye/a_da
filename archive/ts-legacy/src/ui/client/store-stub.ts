/**
 * UI 生产打包轻量桩模块
 *
 * 在桌面独立可执行程序形态下，所有 Agent 核心逻辑均运行在纯 Rust 原生无头 Host 中，
 * UI 侧只通过轻量客户端与宿主通信，不需要在前端打包体积庞大、带有 async generator 的 Node 侧 store。
 */
import type { ApprovalMode, Effort } from '../../shared/protocol'

export const APPROVAL_OPTIONS: { value: ApprovalMode; label: string }[] = [
  { value: 'auto', label: '自动批准' },
  { value: 'ask', label: '每次询问' },
  { value: 'readonly', label: '只读' },
]

export const EFFORT_OPTIONS: { value: Effort; label: string }[] = [
  { value: 'max', label: '最高' },
  { value: 'high', label: '高' },
  { value: 'medium', label: '中' },
  { value: 'low', label: '低' },
]

export const store = {
  snapshot: () => ({
    threads: [],
    activeThreadId: null,
    ui: { activeId: null, openTabIds: [] },
    config: {},
    workspace: { project: '', files: [], dirs: [], scanning: false, entries: [] },
  }),
  subscribe: () => () => {},
  dispatch: () => {},
} as any
