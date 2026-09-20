/**
 * 工作区规则：路径沙箱与遍历跳过清单。
 *
 * 所有内置工具共用这一份，因为它们是同一句承诺的两半——「Agent 只能碰这个项目
 * 里该看的文件」。散成多份的话，某个工具的清单迟早会和别人不一样。
 */

import { isAbsolute, relative, resolve } from 'node:path'

/** 目录遍历一律跳过的名字。 */
export const SKIP_DIRS = new Set([
  'node_modules',
  '.git',
  'dist',
  'build',
  'out',
  'target',
  '.next',
  '.cache',
  '.venv',
  '__pycache__',
  '.turbo',
  'coverage',
])

/**
 * 把模型给的路径解析回工作区根目录，越界即抛错。
 *
 * 注意这里按路径字符串判断，不解析符号链接：工作区内的一个 symlink 指向外面
 * 时仍会被放行。
 */
export function checkWorkspaceSandbox(workspace: string, targetPath: string): string {
  const normWorkspace = resolve(workspace)
  const full = isAbsolute(targetPath) ? resolve(targetPath) : resolve(normWorkspace, targetPath)
  const rel = relative(normWorkspace, full)
  if (rel.startsWith('..') || isAbsolute(rel)) {
    throw new Error(`拒绝访问工作区外的路径：${targetPath}`)
  }
  return full
}
