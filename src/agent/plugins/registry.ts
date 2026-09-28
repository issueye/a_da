/**
 * 已加载插件的索引（**插件级**聚合视图）。
 *
 * 加载器每次重载结束后把结果写进来；需要"按插件"看贡献与诊断的地方都从这里读。
 * 之所以是个独立模块而不是挂在加载器上：技能与提示词管理器要取出第三方插件
 * 内联声明的 skills / prompts，而管理器不能反向 import 加载器——加载器要 import
 * 管理器做目录扫描，那样就成了循环依赖。
 *
 * 与 `src/agent/tools/registry.ts` 的分工：那边管**工具**的注册与溯源（谁注册了
 * 哪个工具、有没有重名），这边管**插件**的清单、状态与诊断。
 */

import type { LoadedPlugin, PluginDiagnostic } from './types'

let loaded: LoadedPlugin[] = []

/** 覆盖当前索引（加载器在每次重载后调用）。 */
export function setLoadedPlugins(plugins: LoadedPlugin[]): void {
  loaded = [...plugins]
}

export function getLoadedPlugins(): LoadedPlugin[] {
  return loaded
}

export function clearLoadedPlugins(): void {
  loaded = []
}

export function findLoadedPlugin(pluginId: string): LoadedPlugin | undefined {
  return loaded.find((item) => item.manifest.id === pluginId)
}

/**
 * 所有插件的诊断汇总，error 在前、warn 其次。
 *
 * 顺序刻意不是"按插件顺序"：诊断是要给人看的，一条 error 埋在十几条 info 中间
 * 等于没说。
 */
export function getPluginDiagnostics(): PluginDiagnostic[] {
  const weight: Record<PluginDiagnostic['level'], number> = { error: 0, warn: 1, info: 2 }
  return loaded
    .flatMap((item) => item.diagnostics)
    .sort((a, b) => weight[a.level] - weight[b.level])
}
