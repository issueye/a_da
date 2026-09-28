/**
 * 批量读写提效插件 (batch-ops) 与它的改动安全网。
 *
 * 这个插件存在的理由是「步数」：一次调用覆盖多个文件。所以测试盯两类事：
 * 1. 工具本身真的会读/改多个文件，且失败时行为可预期（不做一半）；
 * 2. 批量写不能绕开既有的安全网——store 侧的检查点快照与「改动审阅」都必须
 *    按文件把批量改动拆开记账，否则批量编辑就成了回滚不到的黑洞。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { BUILTIN_PLUGINS } from './builtin-plugins'
import type { AgentTool } from '../core/types'
import { defaultCheckpointManager } from '../checkpoint'
import { AgentStore } from '../store'

const batchPlugin = BUILTIN_PLUGINS.find((p) => p.id === 'batch-ops')!

function toolNamed(name: string, workspace: string): AgentTool {
  const raw = batchPlugin.tools.find((t) => {
    const inst = typeof t === 'function' ? t(workspace) : t
    return inst.name === name
  })!
  return typeof raw === 'function' ? raw(workspace) : raw
}

let workspace = ''
let home = ''
let oldHome: string | undefined

beforeEach(async () => {
  workspace = await mkdtemp(join(tmpdir(), 'ada-batch-ws-'))
  home = await mkdtemp(join(tmpdir(), 'ada-batch-home-'))
  oldHome = process.env.A_DA_HOME
  process.env.A_DA_HOME = home
})

afterEach(async () => {
  if (oldHome === undefined) delete process.env.A_DA_HOME
  else process.env.A_DA_HOME = oldHome
  await rm(workspace, { recursive: true, force: true })
  await rm(home, { recursive: true, force: true })
})

describe('batch-ops 插件注册', () => {
  test('插件同时提供 read_files 与 edit_files，且具备三位一体结构', () => {
    expect(batchPlugin.id).toBe('batch-ops')
    expect(batchPlugin.skills?.length).toBeGreaterThan(0)
    expect(batchPlugin.prompts?.length).toBeGreaterThan(0)

    const names = batchPlugin.tools.map((t) => (typeof t === 'function' ? t(workspace).name : t.name))
    expect(names).toContain('read_files')
    expect(names).toContain('edit_files')
  })

  test('执行模式：读可并发、写必须串行', () => {
    expect(toolNamed('read_files', workspace).executionMode).toBe('parallel')
    expect(toolNamed('edit_files', workspace).executionMode).toBe('sequential')
  })
})

describe('read_files：一次读多个文件', () => {
  test('批量返回所有文件内容并带行号', async () => {
    await writeFile(join(workspace, 'a.txt'), 'alpha\nbeta\n', 'utf8')
    await writeFile(join(workspace, 'b.txt'), 'gamma\n', 'utf8')

    const result = await toolNamed('read_files', workspace).execute('call-1', {
      paths: ['a.txt', 'b.txt'],
    })

    expect(result.ok).toBe(true)
    expect(result.output).toContain('已批量读取 2/2 个文件')
    expect(result.output).toContain('1 | alpha')
    expect(result.output).toContain('2 | beta')
    expect(result.output).toContain('1 | gamma')
  })

  test('支持逐文件指定行号范围', async () => {
    await writeFile(join(workspace, 'long.txt'), 'L1\nL2\nL3\nL4\nL5\n', 'utf8')

    const result = await toolNamed('read_files', workspace).execute('call-2', {
      files: [{ path: 'long.txt', offset: 2, limit: 2 }],
    })

    expect(result.ok).toBe(true)
    expect(result.output).toContain('2 | L2')
    expect(result.output).toContain('3 | L3')
    expect(result.output).not.toContain('1 | L1')
    expect(result.output).not.toContain('4 | L4')
  })

  test('单个文件不存在不影响同批其它文件', async () => {
    await writeFile(join(workspace, 'ok.txt'), 'fine\n', 'utf8')

    const result = await toolNamed('read_files', workspace).execute('call-3', {
      paths: ['missing.txt', 'ok.txt'],
    })

    // 有文件读到了就算成功，但没读到的要在摘要里点名
    expect(result.ok).toBe(true)
    expect(result.output).toContain('已批量读取 1/2 个文件')
    expect(result.output).toContain('missing.txt')
    expect(result.output).toContain('fine')
  })

  test('越界路径被沙箱拦下', async () => {
    const result = await toolNamed('read_files', workspace).execute('call-4', {
      paths: ['../escape.txt'],
    })

    expect(result.ok).toBe(false)
    expect(result.output).toContain('拒绝访问')
  })

  test('不提供路径时明确报错', async () => {
    const result = await toolNamed('read_files', workspace).execute('call-5', {})
    expect(result.ok).toBe(false)
    expect(result.output).toContain('至少一个')
  })
})

describe('edit_files：一次改多个文件', () => {
  test('批量应用替换并逐文件产出 diff 与统计', async () => {
    await writeFile(join(workspace, 'x.ts'), 'const value = 1\nexport default value\n', 'utf8')
    await writeFile(join(workspace, 'y.ts'), 'const other = 2\n', 'utf8')

    const result = await toolNamed('edit_files', workspace).execute('call-6', {
      files: [
        { path: 'x.ts', edits: [{ old_string: 'const value = 1', new_string: 'const value = 42' }] },
        { path: 'y.ts', old_string: 'const other = 2', new_string: 'const other = 99' },
      ],
    })

    expect(result.ok).toBe(true)
    expect(await readFile(join(workspace, 'x.ts'), 'utf8')).toContain('const value = 42')
    expect(await readFile(join(workspace, 'y.ts'), 'utf8')).toContain('const other = 99')

    // details.files 是 store 侧拆账的依据：每个文件要有自己的分段 patch
    const files = (result.details as any).files as Array<{ path: string; patch: string }>
    expect(files).toHaveLength(2)
    expect(files.map((f) => f.path).sort()).toEqual(['x.ts', 'y.ts'])
    for (const file of files) {
      expect(file.patch).toContain(`--- a/${file.path}`)
    }
  })

  test('old_string 在文件内出现多次时该文件不改动并报错', async () => {
    const original = 'dup\ndup\n'
    await writeFile(join(workspace, 'dup.txt'), original, 'utf8')

    const result = await toolNamed('edit_files', workspace).execute('call-7', {
      files: [{ path: 'dup.txt', edits: [{ old_string: 'dup', new_string: 'once' }] }],
    })

    expect(result.ok).toBe(false)
    expect(result.output).toContain('出现了多次')
    // 报错就必须原样留着，不能改坏
    expect(await readFile(join(workspace, 'dup.txt'), 'utf8')).toBe(original)
  })

  test('一批里一个文件失败不影响其它文件的正常改动', async () => {
    await writeFile(join(workspace, 'good.txt'), 'hello\n', 'utf8')
    await writeFile(join(workspace, 'bad.txt'), 'hello\n', 'utf8')

    const result = await toolNamed('edit_files', workspace).execute('call-8', {
      files: [
        { path: 'good.txt', edits: [{ old_string: 'hello', new_string: 'world' }] },
        { path: 'bad.txt', edits: [{ old_string: 'not-there', new_string: 'x' }] },
      ],
    })

    expect(result.ok).toBe(false)
    expect(await readFile(join(workspace, 'good.txt'), 'utf8')).toContain('world')
    expect(await readFile(join(workspace, 'bad.txt'), 'utf8')).toBe('hello\n')
    expect(result.output).toContain('成功 1 / 2')
  })

  test('文件不存在时报错且不影响同批其它文件', async () => {
    await writeFile(join(workspace, 'present.txt'), 'text\n', 'utf8')

    const result = await toolNamed('edit_files', workspace).execute('call-9', {
      files: [
        { path: 'present.txt', edits: [{ old_string: 'text', new_string: 'changed' }] },
        { path: 'absent.txt', edits: [{ old_string: 'a', new_string: 'b' }] },
      ],
    })

    expect(result.ok).toBe(false)
    expect(result.output).toContain('文件不存在')
    expect(await readFile(join(workspace, 'present.txt'), 'utf8')).toContain('changed')
  })

  test('未提供任何有效替换对时报错', async () => {
    await writeFile(join(workspace, 'z.txt'), 'z\n', 'utf8')
    const result = await toolNamed('edit_files', workspace).execute('call-10', {
      files: [{ path: 'z.txt', edits: [] }],
    })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('成功 0 / 1')
  })

  test('越界路径被沙箱拦下', async () => {
    const result = await toolNamed('edit_files', workspace).execute('call-11', {
      files: [{ path: '../outside.txt', edits: [{ old_string: 'a', new_string: 'b' }] }],
    })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('拒绝访问')
  })
})

describe('批量编辑接入改动安全网', () => {
  test('edit_files 一次调用动多个文件，改动审阅按文件分别入账', async () => {
    const store = new AgentStore(workspace)
    const thread = store.newThread(workspace)
    store.selectThread(thread.id)

    await writeFile(join(workspace, 'one.ts'), 'const a = 1\n', 'utf8')
    await writeFile(join(workspace, 'two.ts'), 'const b = 2\n', 'utf8')

    const tool = toolNamed('edit_files', workspace)
    const result = await tool.execute('call-batch-1', {
      files: [
        { path: 'one.ts', edits: [{ old_string: 'const a = 1', new_string: 'const a = 10' }] },
        { path: 'two.ts', edits: [{ old_string: 'const b = 2', new_string: 'const b = 20' }] },
      ],
    })
    expect(result.ok).toBe(true)

    // 模拟主循环收尾后的卡片：patch 与 details 就是这样落进 thread.items 的
    thread.items.push({
      kind: 'tool',
      id: 'card-batch-1',
      at: Date.now(),
      callId: 'call-batch-1',
      name: 'edit_files',
      args: { files: [{ path: 'one.ts' }, { path: 'two.ts' }] },
      rawArgs: '{}',
      status: 'done',
      output: result.output,
      patch: result.patch,
      details: result.details,
    } as any)

    const changes = store.getThreadFileChanges(thread.id)
    expect(changes.map((c) => c.path).sort()).toEqual(['one.ts', 'two.ts'])
    for (const change of changes) {
      expect(change.additions).toBe(1)
      expect(change.deletions).toBe(1)
      expect(change.latestPatch).toContain(`--- a/${change.path}`)
    }
    expect(store.getThreadChangeCount(thread.id)).toBe(2)

    store.deleteThread(thread.id)
  })

  test('批量改动可逐文件回滚，未点到的文件保持改动后的样子', async () => {
    const store = new AgentStore(workspace)
    const thread = store.newThread(workspace)
    store.selectThread(thread.id)

    const beforeOne = 'const a = 1\n'
    const beforeTwo = 'const b = 2\n'
    await writeFile(join(workspace, 'one.ts'), beforeOne, 'utf8')
    await writeFile(join(workspace, 'two.ts'), beforeTwo, 'utf8')

    // 快照必须覆盖这一批的每个文件，否则回滚会漏
    await defaultCheckpointManager.capture(thread.id, 'call-batch-2', [
      { path: 'one.ts', absolute: join(workspace, 'one.ts') },
      { path: 'two.ts', absolute: join(workspace, 'two.ts') },
    ])

    const tool = toolNamed('edit_files', workspace)
    const result = await tool.execute('call-batch-2', {
      files: [
        { path: 'one.ts', edits: [{ old_string: 'const a = 1', new_string: 'const a = 10' }] },
        { path: 'two.ts', edits: [{ old_string: 'const b = 2', new_string: 'const b = 20' }] },
      ],
    })
    expect(result.ok).toBe(true)

    thread.items.push({
      kind: 'tool',
      id: 'card-batch-2',
      at: Date.now(),
      callId: 'call-batch-2',
      name: 'edit_files',
      args: { files: [{ path: 'one.ts' }, { path: 'two.ts' }] },
      rawArgs: '{}',
      status: 'done',
      checkpointId: undefined,
      output: result.output,
      patch: result.patch,
      details: result.details,
    } as any)

    // 只回滚其中一个文件：另一个必须原封不动
    const reverted = await store.revertFile(thread.id, 'one.ts')
    expect(reverted).toBe(true)
    expect(await readFile(join(workspace, 'one.ts'), 'utf8')).toBe(beforeOne)
    expect(await readFile(join(workspace, 'two.ts'), 'utf8')).toContain('const b = 20')

    store.deleteThread(thread.id)
    await defaultCheckpointManager.discard(thread.id)
  })
})
