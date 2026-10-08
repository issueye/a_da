/**
 * 应用数据目录。
 *
 * 配置、会话流水、全局扩展都落在这一个目录下——一个应用不该有两个家。
 * `A_DA_HOME` 可以换掉它（测试就靠这个不碰你的真实目录）；配置文件另有
 * `A_DA_CONFIG` 指向单个文件。
 */

import { homedir } from 'node:os'
import { join } from 'node:path'
import { shortPath } from '../theme'

export function getAppHome(): string {
  return process.env.A_DA_HOME || join(homedir(), '.a-da')
}

/** 公共区在界面上的名字：它是一个工作区，但不是用户的项目，所以不显示路径。 */
export const PUBLIC_WORKSPACE_LABEL = '公共区'

/**
 * 由应用数据目录推导公共区路径（纯函数）。
 *
 * 单独拆出来是为了能直接验算，而不必去改 `A_DA_HOME`：环境变量是进程级的，测试
 * 套件并发跑在一个进程里，改它会把别的文件的会话目录也换掉（AGENTS.md §13）。
 */
export function publicWorkspaceOf(home: string): string {
  return join(home, 'workspace')
}

/**
 * 公共区目录：a-da 自带的工作区（`~/.a-da/workspace`）。
 *
 * 它就是一个普通工作区（同样的会话落盘、同样的工具沙箱），只是目录由 a-da
 * 提供而不是用户挑的项目目录——「不绑定任何项目」的对话有地方可去。
 *
 * **只在启动时取一次并存下来**（`AgentStore` 就是这么做的），判断时用存下来那份：
 * `A_DA_HOME` 是进程级变量，每次重算既不必要，也会让并发测试互相踩。
 */
export function getPublicWorkspace(): string {
  return publicWorkspaceOf(getAppHome())
}

/**
 * 两个路径是不是同一个工作区。
 *
 * 归一化后再比：同一个目录会以 `E:/code` 与 `E:\code`、大小写差异等写法出现，
 * 不归一化就会算成两个工作区（与 `session/manager.ts` 的 `workspaceSlug` 同一套规则）。
 */
export function sameWorkspacePath(a: string, b: string): boolean {
  const canonical = (path: string) => path.replace(/[\\/]+/g, '/').replace(/\/+$/, '').toLowerCase()
  return canonical(a) === canonical(b)
}

/** 这个路径是不是公共区（`publicPath` 由调用方传入，避免读进程级环境变量）。 */
export function isPublicWorkspace(path: string, publicPath: string): boolean {
  return sameWorkspacePath(path, publicPath)
}

/** 工作区的展示名：公共区不暴露实现路径，其余仍是短路径。 */
export function workspaceLabel(path: string, publicPath: string): string {
  return isPublicWorkspace(path, publicPath) ? PUBLIC_WORKSPACE_LABEL : shortPath(path, 2)
}
