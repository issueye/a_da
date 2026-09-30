/**
 * 主机文件服务的行为（协议 §3.14）。
 *
 * 用真临时目录，不打桩——这里要验的正是"真实文件系统上会发生什么"：排序、隐藏项、
 * 截断如实报告、错误如实抛出。**只回元数据**这点也一并钉住（不回内容）。
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { homedir, tmpdir } from 'node:os'
import { join } from 'node:path'
import { MAX_ENTRIES, listDirectory, listRoots, makeDirectory } from './fs-service'

const dirs: string[] = []

function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), 'ada-fs-'))
  dirs.push(dir)
  return dir
}

afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true })
})

describe('fs.list：列目录', () => {
  test('目录在前、按名字排序；只给元数据（没有内容字段）', () => {
    const root = tempDir()
    mkdirSync(join(root, 'zeta'))
    mkdirSync(join(root, 'alpha'))
    writeFileSync(join(root, 'b.txt'), 'hello')
    writeFileSync(join(root, 'a.txt'), 'hi')

    const listing = listDirectory(root)

    expect(listing.path).toBe(root)
    expect(listing.entries.map((entry) => `${entry.kind}:${entry.name}`)).toEqual([
      'dir:alpha',
      'dir:zeta',
      'file:a.txt',
      'file:b.txt',
    ])
    // 元数据在（大小/时间），**内容不在**
    const file = listing.entries.find((entry) => entry.name === 'b.txt')!
    expect(file.sizeBytes).toBe(5)
    expect(typeof file.mtimeMs).toBe('number')
    expect(JSON.stringify(listing)).not.toContain('hello')
  })

  test('隐藏项默认省略并如实计数；要求显示时给出来', () => {
    const root = tempDir()
    mkdirSync(join(root, '.git'))
    writeFileSync(join(root, '.env'), 'SECRET=1')
    writeFileSync(join(root, 'visible.txt'), 'x')

    const hidden = listDirectory(root)
    expect(hidden.entries.map((entry) => entry.name)).toEqual(['visible.txt'])
    expect(hidden.hiddenCount).toBe(2)

    const shown = listDirectory(root, { showHidden: true })
    expect(shown.entries.map((entry) => entry.name)).toEqual(['.git', '.env', 'visible.txt'])
    expect(shown.hiddenCount).toBe(0)
  })

  test('条目超过上限时如实报告截断与省略数量（不静默丢）', () => {
    const root = tempDir()
    for (let i = 0; i < 12; i++) writeFileSync(join(root, `f${String(i).padStart(2, '0')}.txt`), 'x')

    const listing = listDirectory(root, { limit: 5 })
    expect(listing.entries).toHaveLength(5)
    expect(listing.truncated).toBe(true)
    expect(listing.omitted).toBe(7)

    const all = listDirectory(root, { limit: MAX_ENTRIES })
    expect(all.truncated).toBe(false)
    expect(all.omitted).toBe(0)
  })

  test('路径不存在 → 明确报"找不到"并带上那个路径；不是目录 → 明确说是参数问题', () => {
    const root = tempDir()
    const missing = join(root, 'nope')

    expect(() => listDirectory(missing)).toThrow(/路径不存在/)
    expect(() => listDirectory(missing)).toThrow(new RegExp(missing.replace(/\\/g, '\\\\')))

    writeFileSync(join(root, 'a.txt'), 'x')
    expect(() => listDirectory(join(root, 'a.txt'))).toThrow(/不是目录/)
  })

  test('到根时 parent 为 null（界面据此禁用"上一级"）', () => {
    const root = tempDir()
    // 临时目录的上级一定存在，所以先验"普通目录有 parent"
    expect(listDirectory(root).parent).toBeTruthy()

    // 根目录本身（Windows 上取当前盘符）
    const atRoot = listDirectory(process.platform === 'win32' ? `${root.slice(0, 2)}\\` : '/')
    expect(atRoot.parent).toBeNull()
  })
})

describe('fs.roots：可跳转的根', () => {
  test('额外根排在前面并去重，主目录一定在', () => {
    const workspace = tempDir()
    const roots = listRoots([workspace, workspace, ''])

    expect(roots[0]).toMatchObject({ path: workspace, kind: 'workspace' })
    expect(roots.filter((root) => root.path.toLowerCase() === workspace.toLowerCase())).toHaveLength(1)
    expect(roots.some((root) => root.kind === 'home' && root.path === homedir())).toBe(true)
  })
})

describe('fs.mkdir：新建一层目录', () => {
  test('建成后 fs.list 能列到它', () => {
    const root = tempDir()
    const created = makeDirectory(join(root, '新项目'))

    expect(created.path).toBe(join(root, '新项目'))
    expect(listDirectory(root).entries.map((entry) => entry.name)).toContain('新项目')
  })

  test('已存在 → 报错（不静默复用）；上一级不存在 → 报"找不到"', () => {
    const root = tempDir()
    mkdirSync(join(root, 'exists'))

    expect(() => makeDirectory(join(root, 'exists'))).toThrow(/已经存在/)
    expect(() => makeDirectory(join(root, 'no-such-parent', 'child'))).toThrow(/上一级目录不存在/)
  })
})
