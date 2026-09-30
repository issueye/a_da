/**
 * 主机侧的文件服务（协议 §3.14）：界面要选工作区目录、附件、图片时，**不再开原生选择窗口**，
 * 而是让应用自己的选择器通过这几个方法浏览主机的文件系统。
 *
 * ## 为什么要有它
 *
 * 原生选择窗口（PowerShell 的 FolderBrowserDialog、GPUIX 的 `promptForPaths`）有三个问题：
 * ① 只有本机能用——将来 Web/H5 前端（协议 §12）根本没有这些 API；
 * ② 测试里必须打桩（真弹窗会把自动化卡住），于是"选择路径"这条路在测试里从来没被真正跑过；
 * ③ 界面拿不到"这次选了哪些"以外的任何信息，做不了最近目录、过滤、预览。
 * 改成服务之后，这三件事都归主机，客户端只画界面。
 *
 * ## 边界（很重要）
 *
 * - **只回元数据，不回文件内容**：`fs.list` 给名字/类型/大小/时间，不读内容。
 *   图片预览（dataUrl）是另一件事，等真需要时再按协议 §12.3 的能力位加。
 * - **不跟随越权**：本服务只服务"跑主机的那个用户本来就能读的路径"。绑定 `127.0.0.1` + 令牌
 *   意味着只有本机、且拿得到令牌的客户端能调（协议 §1.6）。远端客户端要用它，必须先过
 *   能力协商（协议 §12.2/§12.3）——这是**留给 M4 的决策点**，不是现在偷偷放开。
 * - **不静默截断**：条目超过上限时返回 `truncated: true` 并给出实际省略的条数，
 *   界面必须说出来（"不允许静默失效"）。
 */

import { existsSync, readdirSync, statSync, mkdirSync } from 'node:fs'
import { homedir } from 'node:os'
import path from 'node:path'
import { appError, ProtocolError, RpcErrorCode } from '../../shared/protocol'
import type { FsEntry, FsListing, FsRoot } from '../../shared/protocol'

/**
 * `FsEntry` / `FsListing` / `FsRoot` 的形状已归位到契约层 `src/shared/protocol`
 * （协议设计 §7.1）：管理界面要跨进程拿到它们。这里原样再导出，既有调用点不受影响。
 */
export type { FsEntry, FsListing, FsRoot }

/** 一次列目录最多回多少条（超出如实报告，见 `truncated`/`omitted`）。 */
export const MAX_ENTRIES = 2000

/** 名字算不算"隐藏"：以点开头。Windows 的隐藏属性要 winapi，这里刻意不碰（文档已写明）。 */
function isHiddenName(name: string): boolean {
  return name.startsWith('.')
}

/**
 * 可跳转的根：驱动器（Windows）/ 文件系统根、主目录、以及调用方给的额外根（如当前工作区）。
 *
 * 额外根去重且排在前面——用户最常回的就是"我现在这个项目"。
 */
export function listRoots(extraRoots: readonly string[] = []): FsRoot[] {
  const roots: FsRoot[] = []
  const seen = new Set<string>()

  const push = (root: FsRoot): void => {
    const key = root.path.toLowerCase()
    if (seen.has(key)) return
    seen.add(key)
    roots.push(root)
  }

  for (const workspace of extraRoots) {
    if (!workspace) continue
    push({ path: workspace, label: path.basename(workspace) || workspace, kind: 'workspace' })
  }

  const home = homedir()
  if (home) push({ path: home, label: '主目录', kind: 'home' })

  if (process.platform === 'win32') {
    for (let code = 67; code <= 90; code++) {
      // C..Z：A/B 多半是软驱，列出来只会碍事
      const drive = `${String.fromCharCode(code)}:\\`
      if (existsSync(drive)) push({ path: drive, label: drive, kind: 'drive' })
    }
  } else {
    push({ path: '/', label: '/', kind: 'drive' })
  }

  return roots
}

/** 列一个目录：目录在前、各自按名字排序（大小写不敏感，中文按本地顺序）。 */
export function listDirectory(
  rawPath: string,
  options: { showHidden?: boolean; limit?: number } = {}
): FsListing {
  const limit = Math.min(options.limit ?? MAX_ENTRIES, MAX_ENTRIES)
  const target = resolveExistingPath(rawPath)

  let stats: ReturnType<typeof statSync>
  try {
    stats = statSync(target)
  } catch (error) {
    throw appError('Denied', `读不了这个路径：${(error as Error).message}`, { reason: String(error) })
  }
  if (!stats.isDirectory()) {
    throw new ProtocolError(RpcErrorCode.InvalidParams, `不是目录：${target}`, { path: target })
  }

  const dirs: FsEntry[] = []
  const files: FsEntry[] = []
  let hiddenCount = 0

  let names: string[]
  try {
    names = readdirSync(target)
  } catch (error) {
    throw appError('Denied', `读不了这个目录：${(error as Error).message}`, { reason: String(error) })
  }

  for (const name of names) {
    if (!options.showHidden && isHiddenName(name)) {
      hiddenCount += 1
      continue
    }
    const full = path.join(target, name)
    try {
      const entryStats = statSync(full)
      if (entryStats.isDirectory()) {
        dirs.push({ name, path: full, kind: 'dir', mtimeMs: entryStats.mtimeMs })
      } else if (entryStats.isFile()) {
        files.push({ name, path: full, kind: 'file', sizeBytes: entryStats.size, mtimeMs: entryStats.mtimeMs })
      }
      // 既不是文件也不是目录（设备、socket…）：跳过——界面也选不了它
    } catch {
      // 单个条目读不了（权限/竞态）不该让整次浏览失败：跳过它，其余照常
      continue
    }
  }

  const byName = (a: FsEntry, b: FsEntry): number =>
    a.name.localeCompare(b.name, 'zh-Hans-CN', { sensitivity: 'base' })
  dirs.sort(byName)
  files.sort(byName)

  const all = [...dirs, ...files]
  const entries = all.slice(0, limit)
  const parent = path.dirname(target)
  return {
    path: target,
    parent: parent === target ? null : parent,
    entries,
    truncated: all.length > entries.length,
    omitted: Math.max(0, all.length - entries.length),
    hiddenCount,
  }
}

/** 新建一个目录（只建一层：父目录必须已存在，已存在则如实报错）。 */
export function makeDirectory(rawPath: string): { path: string } {
  const target = path.resolve(rawPath)
  if (existsSync(target)) {
    throw new ProtocolError(RpcErrorCode.InvalidParams, `已经存在：${target}`, { path: target })
  }
  const parent = path.dirname(target)
  if (!existsSync(parent)) {
    throw appError('NotFound', `上一级目录不存在：${parent}`, { kind: 'path', id: parent })
  }
  try {
    mkdirSync(target)
  } catch (error) {
    throw appError('Denied', `建不了目录：${(error as Error).message}`, { reason: String(error) })
  }
  return { path: target }
}

/**
 * 把用户给的路径解析成绝对路径；不存在就抛"找不到"。
 *
 * 单独拆出来是为了让错误信息统一：界面要能直接说"哪个路径不存在"，
 * 而不是笼统地报"操作失败"。
 */
export function resolveExistingPath(rawPath: string): string {
  const candidate = (rawPath ?? '').trim()
  if (!candidate) {
    throw new ProtocolError(RpcErrorCode.InvalidParams, '路径不能为空', { path: rawPath })
  }
  // Windows 上 `/` 也能被用户输入：交给 path.resolve 处理，它会按平台规范化
  const target = path.resolve(candidate)
  if (!existsSync(target)) {
    throw appError('NotFound', `路径不存在：${target}`, { kind: 'path', id: target })
  }
  return target
}
