/**
 * 内置扩展插件库统一注册导出
 */

import { gitToolsPlugin } from './git-tools'
import { codeOutlinePlugin } from './code-outline'
import { projectInspectorPlugin } from './project-inspector'
import { testRunnerPlugin } from './test-runner'
import { batchOpsPlugin } from './batch-ops'
import { decisionPlugin } from './decision'
import { approvalGuardPlugin } from './approval-guard'
import { askUserPlugin } from './ask-user'
import type { PluginDescriptor } from './types'

export * from './types'
export { gitToolsPlugin } from './git-tools'
export { codeOutlinePlugin } from './code-outline'
export { projectInspectorPlugin } from './project-inspector'
export { testRunnerPlugin } from './test-runner'
export { batchOpsPlugin } from './batch-ops'
export { decisionPlugin } from './decision'
export { approvalGuardPlugin } from './approval-guard'
export { askUserPlugin } from './ask-user'

/** 系统预置的官方内置插件包清单 */
export const BUILTIN_PLUGINS: PluginDescriptor[] = [
  gitToolsPlugin,
  codeOutlinePlugin,
  projectInspectorPlugin,
  testRunnerPlugin,
  batchOpsPlugin,
  decisionPlugin,
  approvalGuardPlugin,
  askUserPlugin,
]
