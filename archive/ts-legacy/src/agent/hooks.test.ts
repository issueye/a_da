/**
 * 用户可配置钩子的测试。
 * parseHooksConfig 的容错、before_tool 拦截语义（退出码非零 + stderr 理由）、
 * 工具名匹配、payload 经 stdin 到达钩子命令、agent_end 不拦截。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { HookManager, parseHooksConfig } from './hooks'
import { getAppHome } from './home'

let ws = ''
let savedHooksJson: string | null = null
const hooksFile = join(getAppHome(), 'hooks.json')

beforeAll(async () => {
  ws = await mkdtemp(join(tmpdir(), 'a-da-hooks-'))
  await mkdir(join(ws, '.ada'), { recursive: true })
  // 保存并清空真实的全局 hooks.json，测试只依赖工作区那份
  try {
    const { readFile } = await import('node:fs/promises')
    savedHooksJson = await readFile(hooksFile, 'utf-8')
  } catch {
    savedHooksJson = null
  }
  await writeFile(hooksFile, '').catch(() => {})
})

afterAll(async () => {
  if (savedHooksJson !== null) {
    await writeFile(hooksFile, savedHooksJson).catch(() => {})
  } else {
    await rm(hooksFile, { force: true }).catch(() => {})
  }
  if (ws) await rm(ws, { recursive: true, force: true })
})

describe('parseHooksConfig 容错解析', () => {
  test('合法配置完整解析，tool=* 归一化为全部匹配', () => {
    const { rules, warnings } = parseHooksConfig(
      JSON.stringify({
        hooks: [
          { event: 'before_tool', tool: '*', command: 'echo hi', timeout: 5 },
          { event: 'after_tool', tool: 'write_file, run_command', command: 'prettier --write' },
          { event: 'agent_end', command: 'echo done' },
        ],
      }),
      'test'
    )
    expect(warnings).toHaveLength(0)
    expect(rules).toHaveLength(3)
    expect(rules[0]!.tool).toBeUndefined()
    expect(rules[0]!.timeout).toBe(5)
    expect(rules[1]!.tool).toBe('write_file, run_command')
    expect(rules[2]!.tool).toBeUndefined()
  })

  test('坏条目跳过并给出警告，不整个作废', () => {
    const { rules, warnings } = parseHooksConfig(
      JSON.stringify({
        hooks: [
          { event: 'nope', command: 'x' },
          { event: 'before_tool', command: '' },
          { event: 'before_tool', command: 'echo ok' },
        ],
      }),
      'test'
    )
    expect(rules).toHaveLength(1)
    expect(warnings).toHaveLength(2)
  })

  test('非法 JSON 与缺 hooks 数组都有警告', () => {
    expect(parseHooksConfig('{oops', 'a').warnings.length).toBe(1)
    expect(parseHooksConfig('{"x":1}', 'b').warnings.length).toBe(1)
    expect(parseHooksConfig('', 'c').rules).toHaveLength(0)
  })
})

describe('HookManager 执行语义', () => {
  test('before_tool：退出码 0 放行，payload 经 stdin 到达钩子', async () => {
    const out = join(ws, 'payload.json')
    // findstr 把 stdin 原样落到文件：纯 cmd 内建，绕开各家 shell 的引号差异
    const cmd = 'findstr "." > payload.json'
    await writeFile(join(ws, '.ada', 'hooks.json'), JSON.stringify({
      hooks: [{ event: 'before_tool', tool: 'run_command', command: cmd }],
    }))

    const hooks = new HookManager()
    const result = await hooks.run(ws, 'before_tool', 'run_command', { args: { command: 'echo hi' }, thread_id: 't1', workspace: ws })
    expect(result.blocked).toBeFalsy()

    const { readFile } = await import('node:fs/promises')
    const payload = JSON.parse(await readFile(out, 'utf-8'))
    expect(payload.event).toBe('before_tool')
    expect(payload.tool).toBe('run_command')
    expect(payload.args.command).toBe('echo hi')
    expect(payload.thread_id).toBe('t1')
  })

  test('before_tool：退出码非零拦截，stderr 成为理由', async () => {
    await writeFile(join(ws, '.ada', 'hooks.json'), JSON.stringify({
      hooks: [
        {
          event: 'before_tool',
          command: process.platform === 'win32' ? 'echo LOCKED-FILE >&2 & exit 3' : 'echo LOCKED-FILE >&2; exit 3',
        },
      ],
    }))

    const hooks = new HookManager()
    const result = await hooks.run(ws, 'before_tool', 'edit_file', { args: {} })
    expect(result.blocked).toBe(true)
    expect(result.reason).toContain('LOCKED-FILE')
  }, 30000)

  test('tool 名单精确匹配：名单外的工具不触发', async () => {
    await writeFile(join(ws, '.ada', 'hooks.json'), JSON.stringify({
      hooks: [
        {
          event: 'before_tool',
          tool: process.platform === 'win32' ? 'nothing' : 'nothing',
          command: 'exit 1',
        },
      ],
    }))

    const hooks = new HookManager()
    const result = await hooks.run(ws, 'before_tool', 'read_file', { args: {} })
    expect(result.blocked).toBeFalsy()
    expect(result.runs).toHaveLength(0)
  })

  test('after_tool 与 agent_end 的失败不会产生 block 语义', async () => {
    await writeFile(join(ws, '.ada', 'hooks.json'), JSON.stringify({
      hooks: [{ event: 'after_tool', command: 'exit 1' }, { event: 'agent_end', command: 'exit 1' }],
    }))

    const hooks = new HookManager()
    const after = await hooks.run(ws, 'after_tool', 'write_file', { ok: true, output: 'done' })
    expect(after.blocked).toBeFalsy()
    expect(after.runs).toHaveLength(1)

    const end = await hooks.run(ws, 'agent_end', null, { reason: 'completed' })
    expect(end.blocked).toBeFalsy()
  })

  test('全局与工作区配置合并；无配置时空跑', async () => {
    await rm(join(ws, '.ada', 'hooks.json'), { force: true })
    const hooks = new HookManager()
    const empty = await hooks.run(ws, 'before_tool', 'edit_file', {})
    expect(empty.runs).toHaveLength(0)

    await writeFile(join(getAppHome(), 'hooks.json'), JSON.stringify({
      hooks: [{ event: 'agent_end', command: 'echo global' }],
    }))
    try {
      const merged = await hooks.run(ws, 'agent_end', null, { reason: 'x' })
      expect(merged.runs).toHaveLength(1)
    } finally {
      await writeFile(join(getAppHome(), 'hooks.json'), '').catch(() => {})
    }
  })
})
