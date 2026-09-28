/**
 * 已加载插件的索引（**插件级**聚合视图），**按工作区**分开存。
 *
 * 加载器每次重载结束后把结果写进来；需要"按插件"看贡献与诊断的地方都从这里读。
 * 之所以是个独立模块而不是挂在加载器上：技能与提示词管理器要取出第三方插件
 * 内联声明的 skills / prompts，而管理器不能反向 import 加载器——加载器要 import
 * 管理器做目录扫描，那样就成了循环依赖。
 *
 * **为什么按工作区分键**：`current` 只保留一份的话，切换项目后的一小段时间里
 * 界面读到的还是上一个项目的插件（诊断、内联技能都会串），而"上一份"什么时候被覆盖
 * 取决于加载时序。分键之后每个工作区各读各的，这个问题不存在了。
 *
 * 与 `src/agent/tools/registry.ts` 的分工：那边管**工具**的注册与溯源（谁注册了
 * 哪个工具、有没有重名），这边管**插件**的清单、状态与诊断。
 */

import type { LoadedPlugin, PluginDiagnostic } from './types'

const byWorkspace = new Map<string, LoadedPlugin[]>()

/** 覆盖某个工作区的索引（加载器在每次重载后调用）。 */
export function setLoadedPlugins(workspace: string, plugins: LoadedPlugin[]): void {
  byWorkspace.set(workspace, [...plugins])
}

/** 读某个工作区已加载的插件。没加载过的工作区返回空数组。 */
export function getLoadedPlugins(workspace: string | undefined): LoadedPlugin[] {
  if (!workspace) return []
  return byWorkspace.get(workspace) ?? []
}

export function clearLoadedPlugins(workspace?: string): void {
  if (workspace === undefined) byWorkspace.clear()
  else byWorkspace.delete(workspace)
}

export function findLoadedPlugin(workspace: string, pluginId: string): LoadedPlugin | undefined {
  return getLoadedPlugins(workspace).find((item) => item.manifest.id === pluginId)
}

/**
 * 诊断汇总，error 在前、warn 其次。
 *
 * 顺序刻意不是"按插件顺序"：诊断是要给人看的，一条 error 埋在十几条 info 中间
 * 等于没说。不给工作区时汇总全部已加载的——界面上只有当前项目在加载，实际就是那一份；
 * 测试里各工作区互不干扰，正好也靠这个分键。
 */
export function getPluginDiagnostics(workspace?: string): PluginDiagnostic[] {
  const weight: Record<PluginDiagnostic['level'], number> = { error: 0, warn: 1, info: 2 }
  const sources =
    workspace === undefined ? [...byWorkspace.values()].flat() : getLoadedPlugins(workspace)
  return sources.flatMap((item) => item.diagnostics).sort((a, b) => weight[a.level] - weight[b.level])
}
