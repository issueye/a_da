/**
 * 内置扩展插件库统一注册导出
 */

import { gitToolsPlugin } from './git-tools'
import { codeOutlinePlugin } from './code-outline'
import { projectInspectorPlugin } from './project-inspector'
import { testRunnerPlugin } from './test-runner'
import type { BuiltinPluginPackage } from './types'

export * from './types'
export { gitToolsPlugin } from './git-tools'
export { codeOutlinePlugin } from './code-outline'
export { projectInspectorPlugin } from './project-inspector'
export { testRunnerPlugin } from './test-runner'

/** 系统预置的官方内置插件包清单 */
export const BUILTIN_PLUGINS: BuiltinPluginPackage[] = [
  gitToolsPlugin,
  codeOutlinePlugin,
  projectInspectorPlugin,
  testRunnerPlugin,
]
