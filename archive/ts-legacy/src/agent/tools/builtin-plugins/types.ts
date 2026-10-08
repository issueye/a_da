/**
 * 内置插件包数据契约
 *
 * 类型定义已迁到 `src/agent/plugins/types.ts`（插件系统的单一事实来源）。
 * 本文件只做转发，保持既有 import 路径可用。
 */

export type {
  PluginToolFactory,
  PluginScope,
  PluginSkill,
  PluginPrompt,
  PluginManifest,
  PluginConfigProperty,
  PluginConfigSchema,
  PluginContributions,
  PluginDescriptor,
  PluginDiagnostic,
  PluginDiagnosticLevel,
  PluginStatus,
  LoadedPlugin,
} from '../../plugins/types'
