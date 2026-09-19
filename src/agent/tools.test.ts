/**
 * The sandbox is the one promise the UI makes to the user — "the agent can only
 * reach files inside this project" — so it gets a test of its own.
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { unifiedPatch, patchStats } from './patch'
import { describeTool, resolveProjectPath, runTool, scanWorkspace, WorkspaceError } from './tools'

let root = ''

beforeAll(async () => {
  root = await mkdtemp(join(tmpdir(), 'a-da-test-'))
  await mkdir(join(root, 'src'), { recursive: true })
  await writeFile(join(root, 'src/app.ts'), 'export const one = 1\nexport const two = 2\n')
  await writeFile(join(root, 'README.md'), '# demo\n')
})

afterAll(async () => {
  if (root) await rm(root, { recursive: true, force: true })
})

describe('workspace sandbox', () => {
  test('reads a file inside the workspace', async () => {
    const result = await runTool(root, { name: 'read_file', args: { path: 'src/app.ts' } })
    expect(result.ok).toBe(true)
    expect(result.output).toContain('export const two = 2')
  })

  test('refuses a path above the workspace', async () => {
    for (const path of ['../secrets.txt', 'src/../../outside.txt', join(root, '..', 'x')]) {
      const result = await runTool(root, { name: 'read_file', args: { path } })
      expect(result.ok).toBe(false)
      expect(result.output).toContain('拒绝访问工作区外的路径')
    }
  })

  test('refuses a command whose cwd escapes the workspace', async () => {
    const result = await runTool(root, {
      name: 'run_command',
      args: { command: 'echo hi', cwd: '..' },
    })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('拒绝访问工作区外的路径')
  })

  test('lists files and skips node_modules', async () => {
    await mkdir(join(root, 'node_modules/pkg'), { recursive: true })
    await writeFile(join(root, 'node_modules/pkg/index.js'), 'module.exports = {}\n')
    const result = await runTool(root, { name: 'list_files', args: { depth: 3 } })
    expect(result.ok).toBe(true)
    expect(result.output).toContain('src/app.ts')
    expect(result.output).not.toContain('node_modules')
  })

  test('searches with a regular expression', async () => {
    const result = await runTool(root, {
      name: 'search_files',
      args: { pattern: 'const (one|two)' },
    })
    expect(result.ok).toBe(true)
    expect(result.output).toContain('src/app.ts:1: export const one = 1')
    expect(result.output).toContain('src/app.ts:2: export const two = 2')
  })

  test('edits exactly one occurrence and returns a patch', async () => {
    const ok = await runTool(root, {
      name: 'edit_file',
      args: { path: 'src/app.ts', old_string: 'const two = 2', new_string: 'const two = 22' },
    })
    expect(ok.ok).toBe(true)
    expect(patchStats(ok.patch!)).toEqual({ added: 1, removed: 1 })

    const ambiguous = await runTool(root, {
      name: 'edit_file',
      args: { path: 'src/app.ts', old_string: 'export const', new_string: 'export let' },
    })
    expect(ambiguous.ok).toBe(false)
    expect(ambiguous.output).toContain('出现了多次')

    const missing = await runTool(root, {
      name: 'edit_file',
      args: { path: 'src/app.ts', old_string: 'nope', new_string: 'x' },
    })
    expect(missing.ok).toBe(false)
    expect(missing.output).toContain('不存在')
  })

  test('writes a new file and reports the patch as an addition', async () => {
    const result = await runTool(root, {
      name: 'write_file',
      args: { path: 'src/new.ts', content: 'a\nb\n' },
    })
    expect(result.ok).toBe(true)
    expect(patchStats(result.patch!)).toEqual({ added: 2, removed: 0 })
  })

  test('runs a command with the workspace as cwd', async () => {
    const result = await runTool(root, {
      name: 'run_command',
      args: { command: process.platform === 'win32' ? 'cd' : 'pwd' },
    })
    expect(result.ok).toBe(true)
    expect(result.output.replace(/\\/g, '/').toLowerCase()).toContain(
      root.replace(/\\/g, '/').toLowerCase(),
    )
  })

  test('reports a failing command as not ok', async () => {
    const result = await runTool(root, { name: 'run_command', args: { command: 'exit 3' } })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('退出码 3')
  })

  test('counts the workspace for the sidebar', async () => {
    const info = await scanWorkspace(root)
    expect(info.files).toBeGreaterThanOrEqual(3)
    expect(info.entries).toContain('src/')
  })
})

describe('unified patch', () => {
  test('marks context, removals and additions', () => {
    const patch = unifiedPatch('a.txt', 'one\ntwo\nthree\n', 'one\nTWO\nthree\n')
    expect(patch.split('\n')[0]).toBe('--- a/a.txt')
    expect(patch.split('\n')[1]).toBe('+++ b/a.txt')
    expect(patch).toContain('@@ -1,3 +1,3 @@')
    expect(patch).toContain('-two')
    expect(patch).toContain('+TWO')
    expect(patch).toContain(' one')
  })

  test('is empty when nothing changed', () => {
    expect(unifiedPatch('a.txt', 'same\n', 'same\n')).toBe('')
  })
})

describe('project paths', () => {
  test('accepts a directory, with or without quotes', async () => {
    expect(await resolveProjectPath(root)).toEqual({ path: root })
    expect(await resolveProjectPath(`"${root}"`)).toEqual({ path: root })
  })

  test('rejects a file, a missing path and an empty string', async () => {
    expect(await resolveProjectPath(join(root, 'README.md'))).toHaveProperty('error', `不是目录：${join(root, 'README.md')}`)
    expect(await resolveProjectPath(join(root, 'nope'))).toHaveProperty(
      'error',
      `路径不存在：${join(root, 'nope')}`,
    )
    expect(await resolveProjectPath('  ')).toEqual({ error: '请输入目录路径' })
  })
})

describe('tool summaries', () => {
  test('describe a call in one line', () => {
    expect(describeTool('run_command', { command: 'bun test' })).toBe('bun test')
    expect(describeTool('read_file', { path: 'src/app.ts' })).toBe('src/app.ts')
    expect(describeTool('search_files', { pattern: 'useState' })).toBe('/useState/')
  })

  test('a sandbox violation is a WorkspaceError', () => {
    expect(new WorkspaceError('x')).toBeInstanceOf(Error)
  })
})
