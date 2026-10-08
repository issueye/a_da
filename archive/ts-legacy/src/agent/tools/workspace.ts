/**
 * 工作区规则：路径沙箱与遍历跳过清单。
 *
 * 所有内置工具共用这一份，因为它们是同一句承诺的两半——「Agent 只能碰这个项目
 * 里该看的文件」。散成多份的话，某个工具的清单迟早会和别人不一样。
 *
 * 沙箱按**真实落点**判断：路径先做字符串级检查，再用 realpath 解析符号链接，
 * 解析后跑出工作区的一律拒绝。目录遍历用 isPathInsideWorkspace 对 symlink
 * 条目做同样的把关。
 */

import { realpathSync } from 'node:fs'
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path'

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

/** 上溯找最近一个真实存在的祖先做 realpath，避免「写新文件」时 ENOENT。 */
function realpathDeepestExisting(p: string): string {
  let current = p
  const missingTail: string[] = []
  for (let depth = 0; depth < 64; depth++) {
    try {
      const real = realpathSync(current)
      return missingTail.length > 0 ? join(real, ...missingTail.reverse()) : real
    } catch {
      missingTail.push(basename(current))
      const parent = dirname(current)
      if (parent === current) return p
      current = parent
    }
  }
  return p
}

/**
 * 工作区根目录的真实落点：根自己也可能是个 symlink（或 Windows 大小写差异），
 * 目录遍历的相对路径计算要用同一份基准。
 */
export function resolveRealWorkspace(workspace: string): string {
  const resolved = resolve(workspace)
  try {
    return realpathSync(resolved)
  } catch {
    return resolved
  }
}

function escapes(base: string, target: string): boolean {
  const rel = relative(base, target)
  return rel.startsWith('..') || isAbsolute(rel)
}

/**
 * 把模型给的路径解析回工作区根目录，越界即抛错。
 *
 * 字符串级检查先行，随后 realpath 解析符号链接：工作区内的 symlink 指向外面时
 * 同样会被拒绝，返回的是真实路径——后续 fs 操作不再经过任何符号链接。目标文件
 * 尚不存在时（write_file 建新文件），按最近存在的祖先目录解析后再拼回去。
 */
export function checkWorkspaceSandbox(workspace: string, targetPath: string): string {
  const normWorkspace = resolve(workspace)
  const full = isAbsolute(targetPath) ? resolve(targetPath) : resolve(normWorkspace, targetPath)
  if (escapes(normWorkspace, full)) {
    throw new Error(`拒绝访问工作区外的路径：${targetPath}`)
  }

  const realWorkspace = resolveRealWorkspace(normWorkspace)
  const realFull = realpathDeepestExisting(full)
  if (escapes(realWorkspace, realFull)) {
    throw new Error(`路径经符号链接指向工作区外：${targetPath}`)
  }
  return realFull
}

/**
 * 遍历时对 symlink 条目的把关：绝对路径（通常已 realpath）必须真实落在
 * 工作区根内。realpath 失败（悬空链接）一律视为在外。
 */
export function isPathInsideWorkspace(realWorkspace: string, absolutePath: string): boolean {
  let real: string
  try {
    real = realpathSync(absolutePath)
  } catch {
    return false
  }
  return !escapes(realWorkspace, real)
}
