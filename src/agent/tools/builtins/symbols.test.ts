/**
 * 符号索引工具与搜索升级的测试。
 * 在临时工作区里摆一套多语言文件，验证定义抽取、大小写/子串匹配、
 * kind 过滤，以及 search_files 的 literal / context / path 新参数。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createFindSymbolTool, getWorkspaceSymbolIndex } from './symbols'
import { createSearchTool } from './search'

let ws = ''

beforeAll(async () => {
  ws = await mkdtemp(join(tmpdir(), 'a-da-sym-'))
  await mkdir(join(ws, 'src', 'deep'), { recursive: true })
  await writeFile(
    join(ws, 'src', 'service.ts'),
    [
      'import { x } from "./x"',
      '',
      'export class UserService {',
      '  ping() { return 1 }',
      '}',
      '',
      'export async function loadUsers(limit: number) {',
      '  return []',
      '}',
      '',
      'export interface UserRecord {',
      '  id: string',
      '}',
      '',
      'type Handler = (req: unknown) => void',
      '',
      'function internalHelper() {',
      '  return loadUsers(1)',
      '}',
      '',
      'enum Color { Red, Green }',
    ].join('\n'),
  )
  await writeFile(
    join(ws, 'src', 'deep', 'worker.py'),
    ['class Worker:', '    def run(self):', '        pass', '', 'def spawn_worker(count):', '    return count'].join('\n'),
  )
  await writeFile(
    join(ws, 'src', 'deep', 'mod.rs'),
    ['pub struct Engine {', '    pub size: u32,', '}', '', 'impl Engine {', '    pub fn start(&self) {}', '}', '', 'pub trait Walker {', '    fn walk(&self);', '}'].join('\n'),
  )
  // 不该被索引的文件
  await writeFile(join(ws, 'plain.txt'), 'export class NotAClass {}\n')
  await writeFile(join(ws, 'src', 'skip.bin'), Buffer.from([0x00, 0x01, 0x02]))
})

afterAll(async () => {
  if (ws) await rm(ws, { recursive: true, force: true })
})

describe('find_symbol 符号索引', () => {
  test('按名字找到 TS 定义并返回文件与行号', async () => {
    const tool = createFindSymbolTool(ws)
    const result = await tool.execute('call_1', { query: 'UserService' })
    expect(result.ok).toBe(true)
    expect(result.output).toContain('src/service.ts:3')
    expect(result.output).toContain('[class]')
    expect(result.output).toContain('export class UserService')
  })

  test('子串与大小写不敏感匹配', async () => {
    const tool = createFindSymbolTool(ws)
    const result = await tool.execute('call_1', { query: 'loaduser' })
    expect(result.ok).toBe(true)
    expect(result.output).toContain('loadUsers')
  })

  test('kind 过滤：同名不同类只留一份', async () => {
    const tool = createFindSymbolTool(ws)
    const all = await tool.execute('call_1', { query: 'Worker' })
    expect(all.output).toContain('[class]')

    const fn = await tool.execute('call_2', { query: 'worker', kind: 'function' })
    expect(fn.output).toContain('spawn_worker')
    expect(fn.output).not.toContain('[class]')
  })

  test('exact 精确匹配排除子串噪音', async () => {
    const tool = createFindSymbolTool(ws)
    const result = await tool.execute('call_1', { query: 'loadUsers', exact: true })
    expect(result.output).toContain('loadUsers')
    expect(result.output).not.toContain('internalHelper')

    const missing = await tool.execute('call_2', { query: 'loadUser', exact: true })
    expect(missing.output).toContain('没有找到符号')
  })

  test('Python 与 Rust 的定义同样入索引', async () => {
    const tool = createFindSymbolTool(ws)
    const py = await tool.execute('call_1', { query: 'spawn_worker', exact: true })
    expect(py.output).toContain('src/deep/worker.py')

    const rs = await tool.execute('call_2', { query: 'Engine', exact: true })
    expect(rs.output).toContain('src/deep/mod.rs')
    expect(rs.output).toContain('[struct]')

    const trait = await tool.execute('call_3', { query: 'Walker', kind: 'trait' })
    expect(trait.output).toContain('pub trait Walker')
  })

  test('txt 与二进制不入索引；refresh 参数可重建', async () => {
    const index = await getWorkspaceSymbolIndex(ws, true)
    expect(index.byName.has('NotAClass')).toBe(false)
    expect(index.byName.has('internalHelper')).toBe(true)
    expect(index.filesScanned).toBeGreaterThan(0)
  })
})

describe('search_files 升级参数', () => {
  test('literal 纯文本搜索把正则元字符当字面量', async () => {
    const tool = createSearchTool(ws)
    await writeFile(join(ws, 'src', 'literal.txt'), 'a.c() costs $100\nliteral needle (x)\n')

    const regexSearch = await tool.execute('call_1', { pattern: 'a.c' })
    // 正则里 a.c 匹配 a.c()，也会匹配任何 a?c；这里只要求命中
    expect(regexSearch.output).toContain('a.c()')

    const literalSearch = await tool.execute('call_2', { pattern: 'needle (x)', literal: true })
    expect(literalSearch.ok).toBe(true)
    expect(literalSearch.output).toContain('literal needle (x)')
  })

  test('context 提供匹配行的上下文', async () => {
    const tool = createSearchTool(ws)
    await writeFile(join(ws, 'src', 'ctx.txt'), 'line-one\nline-two TARGET\nline-three\nline-four\n')
    const result = await tool.execute('call_1', { pattern: 'TARGET', context: 1 })
    expect(result.output).toContain('ctx.txt:2')
    expect(result.output).toContain('line-one')
    expect(result.output).toContain('line-three')
    expect(result.output).not.toContain('line-four')
  })

  test('case_sensitive 区分大小写', async () => {
    const tool = createSearchTool(ws)
    await writeFile(join(ws, 'src', 'case.txt'), 'MixedCase\nmixedcase\n')
    const insensitive = await tool.execute('call_1', { pattern: 'mixedcase' })
    expect(insensitive.output).toContain('MixedCase')

    const sensitive = await tool.execute('call_2', { pattern: 'mixedcase', case_sensitive: true })
    expect(sensitive.output).not.toContain('MixedCase')
    expect(sensitive.output).toContain('mixedcase')
  })

  test('path 限定子目录搜索', async () => {
    const tool = createSearchTool(ws)
    const scoped = await tool.execute('call_1', { pattern: 'Worker', path: 'src/deep' })
    expect(scoped.output).toContain('worker.py')

    const escaped = await tool.execute('call_2', { pattern: 'x', path: '../../' })
    expect(escaped.ok).toBe(false)
    expect(escaped.output).toContain('工作区')
  })
})
