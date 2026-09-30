/**
 * 审批策略插件（approval-guard）的行为验收。
 *
 * 重点不在 happy path，而在三条规则**互相之间的优先级**——审批是安全边界，
 * "白名单把高危命令一起放过"这类错误只会在组合情况下出现：
 *
 * 1. 高危确认**压过**免问白名单（用户开了自动批准也照样问）；
 * 2. 只读档位下**不发出**注定被忽略的 allow 意图；
 * 3. `deny` 的理由必须具体（含命中的模式），否则用户无从判断该改配置还是改命令。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { cleanupTempDir } from '../../../../scripts/test-preload'
import type { BeforeApprovalContext } from '../../core/events'
import { createApprovalGuardHook, APPROVAL_GUARD_PLUGIN_ID } from './approval-guard'

let home = ''
let oldHome: string | undefined

beforeEach(async () => {
  home = await mkdtemp(join(tmpdir(), 'ada-guard-home-'))
  oldHome = process.env.A_DA_HOME
  process.env.A_DA_HOME = home
})

afterEach(async () => {
  if (oldHome === undefined) delete process.env.A_DA_HOME
  else process.env.A_DA_HOME = oldHome
  await cleanupTempDir(home)
})

/** 写一份插件配置到 `pluginConfig['approval-guard']`。 */
async function configure(patch: Record<string, unknown>): Promise<void> {
  await writeFile(
    join(home, 'config.json'),
    JSON.stringify({ pluginConfig: { [APPROVAL_GUARD_PLUGIN_ID]: patch } }),
    'utf8',
  )
}

/** 造一个审批上下文；默认是一次写文件的普通调用。 */
function ctxOf(
  overrides: Partial<BeforeApprovalContext> & { toolName?: string; command?: string } = {}
): BeforeApprovalContext {
  const { toolName = 'write_file', command, ...rest } = overrides
  const args = command === undefined ? { path: 'a.ts' } : { command }
  return {
    kind: 'main',
    threadId: 'thread-1',
    workspace: '/tmp/ws',
    approvalMode: 'ask',
    isWrite: true,
    toolCall: {
      id: 'call_1',
      name: toolName,
      arguments: args,
      rawArguments: JSON.stringify(args),
    } as BeforeApprovalContext['toolCall'],
    ...rest,
  }
}

describe('审批策略：免问白名单', () => {
  test('默认白名单为空——"默认自动批准"不是可接受的默认值', async () => {
    const hook = createApprovalGuardHook()!
    const verdict = await hook(ctxOf({ toolName: 'write_file' }))
    expect(verdict).toBeUndefined()
  })

  test('白名单里的工具直接放行，并说明是命中白名单', async () => {
    await configure({ autoApprove: ['read_file'] })
    const hook = createApprovalGuardHook()!
    const verdict = await hook(ctxOf({ toolName: 'read_file' }))
    expect(verdict?.decision).toBe('allow')
    expect(verdict?.reason).toContain('白名单')
  })

  test('白名单外的写工具照常问用户', async () => {
    await configure({ autoApprove: ['read_file'] })
    const hook = createApprovalGuardHook()!
    expect(await hook(ctxOf({ toolName: 'write_file' }))).toBeUndefined()
  })
})

describe('审批策略：高危命令二次确认（压过白名单）', () => {
  test('命中高危模式时即使用户开了自动批准也要问', async () => {
    // 这是本插件最容易写错的地方：白名单若排在前面，会把最需要确认的调用一起放过
    await configure({ autoApprove: ['run_command'] })
    const asked: string[] = []
    const hook = createApprovalGuardHook()!

    const verdict = await hook(
      ctxOf({
        toolName: 'run_command',
        command: 'rm -rf build',
        askUser: async (request) => {
          asked.push(request.reason ?? '')
          return { approved: true, answeredBy: 'user' }
        },
      })
    )

    expect(asked).toHaveLength(1)
    expect(asked[0]).toContain('rm ')
    expect(verdict?.decision).toBe('allow')
  })

  test('用户拒绝高危命令时给出具体理由（含命中的模式）', async () => {
    const hook = createApprovalGuardHook()!
    const verdict = await hook(
      ctxOf({
        toolName: 'run_command',
        command: 'git push --force',
        askUser: async () => ({ approved: false, answeredBy: 'user' }),
      })
    )

    expect(verdict?.decision).toBe('deny')
    // 理由必须具体：只说"危险"用户无从判断该改配置还是改命令
    expect(verdict?.reason).toContain('git push')
    expect(verdict?.reason).toContain('用户拒绝')
  })

  test('会话被中止时，理由与"用户拒绝"可区分', async () => {
    const hook = createApprovalGuardHook()!
    const verdict = await hook(
      ctxOf({
        toolName: 'run_command',
        command: 'rm -rf /',
        askUser: async () => ({ approved: false, answeredBy: 'aborted' }),
      })
    )

    expect(verdict?.decision).toBe('deny')
    expect(verdict?.reason).toContain('中止')
    // 两者都要拒绝，但必须能分辨——否则事后审计看不出"用户没答"还是"用户说不"
    expect(verdict?.reason).not.toContain('用户拒绝')
  })

  test('没有 askUser 能力时（如子智能体循环）不自己猜答案，交给核心', async () => {
    const hook = createApprovalGuardHook()!
    const verdict = await hook(ctxOf({ toolName: 'run_command', command: 'rm -rf x' }))
    expect(verdict).toBeUndefined()
  })

  test('普通命令不受影响（不因"命令类工具"就一律要确认）', async () => {
    const hook = createApprovalGuardHook()!
    const verdict = await hook(ctxOf({ toolName: 'run_command', command: 'ls -la' }))
    expect(verdict).toBeUndefined()
  })

  test('自定义高危模式生效', async () => {
    // 模式是**字面子串**匹配（不是通配/正则）：`docker system prune -a` 里没有连续的
    // `docker prune`，所以那种写法匹配不到。测试故意用真实可命中的片段。
    await configure({ confirmCommands: ['system prune'] })
    const asked: string[] = []
    const hook = createApprovalGuardHook()!
    const verdict = await hook(
      ctxOf({
        toolName: 'run_command',
        command: 'docker system prune -a',
        askUser: async (r) => {
          asked.push(r.reason ?? '')
          return { approved: true, answeredBy: 'user' }
        },
      })
    )
    expect(asked[0]).toContain('system prune')
    expect(verdict?.decision).toBe('allow')
  })

  test('模式是字面子串匹配：跨越的词匹配不到（避免用户误以为支持通配）', async () => {
    await configure({ confirmCommands: ['docker prune'] })
    const asked: string[] = []
    const hook = createApprovalGuardHook()!
    const verdict = await hook(
      ctxOf({
        toolName: 'run_command',
        command: 'docker system prune -a',
        askUser: async (r) => {
          asked.push(r.reason ?? '')
          return { approved: true, answeredBy: 'user' }
        },
      })
    )
    expect(asked).toHaveLength(0)
    expect(verdict).toBeUndefined()
  })
})

describe('审批策略：只读档位的硬约束', () => {
  test('只读档位下的写工具**不发出** allow（核心会忽略，插件也不该发）', async () => {
    // 发出注定被忽略的意图会让"为什么没生效"变成谜，所以这里必须是 undefined
    await configure({ autoApprove: ['write_file'] })
    const hook = createApprovalGuardHook()!
    const verdict = await hook(ctxOf({ toolName: 'write_file', approvalMode: 'readonly' }))
    expect(verdict).toBeUndefined()
  })

  test('只读档位下，只读工具仍可走白名单', async () => {
    await configure({ autoApprove: ['read_file'] })
    const hook = createApprovalGuardHook()!
    const verdict = await hook(
      ctxOf({ toolName: 'read_file', approvalMode: 'readonly', isWrite: false })
    )
    expect(verdict?.decision).toBe('allow')
  })

  test('只读档位下高危命令照样二次确认（不被档位短路）', async () => {
    const asked: string[] = []
    const hook = createApprovalGuardHook()!
    const verdict = await hook(
      ctxOf({
        toolName: 'run_command',
        command: 'rm -rf y',
        approvalMode: 'readonly',
        askUser: async (r) => {
          asked.push(r.reason ?? '')
          return { approved: false, answeredBy: 'user' }
        },
      })
    )
    expect(asked).toHaveLength(1)
    expect(verdict?.decision).toBe('deny')
  })
})

describe('审批策略：配置形态与容错', () => {
  test('数组形态（配置文件 / 测试直接写）照常可用', async () => {
    await configure({ autoApprove: ['  ', '', 'read_file'] })
    const hook = createApprovalGuardHook()!
    expect((await hook(ctxOf({ toolName: 'read_file' })))?.decision).toBe('allow')
    expect(await hook(ctxOf({ toolName: 'write_file' }))).toBeUndefined()
  })

  // 界面上的配置表单对 `type: 'string'` 的字段存的就是字符串（插件配置表单不做解析）。
  // 早先只认数组，于是"在插件卡里填了免问白名单"会被静默忽略、照旧每次都问——
  // 只有按**真实写入路径**造的用例能发现它。
  test('界面写入的逗号分隔字符串同样生效', async () => {
    await configure({ autoApprove: 'read_file, git_status' })
    const hook = createApprovalGuardHook()!

    expect((await hook(ctxOf({ toolName: 'read_file' })))?.decision).toBe('allow')
    expect((await hook(ctxOf({ toolName: 'git_status' })))?.decision).toBe('allow')
    // 没列进去的照常问
    expect(await hook(ctxOf({ toolName: 'write_file' }))).toBeUndefined()
  })

  test('高危模式用字符串配置也生效；中文逗号与换行都能分隔', async () => {
    await configure({ confirmCommands: 'docker system prune，rm -rf\nformat' })
    const hook = createApprovalGuardHook()!
    const asked: string[] = []

    const verdict = await hook(
      ctxOf({
        toolName: 'run_command',
        command: 'docker system prune -a',
        askUser: async (request) => {
          asked.push(request.reason ?? '')
          return { approved: false, answeredBy: 'user' }
        },
      })
    )

    expect(asked).toHaveLength(1)
    expect(asked[0]).toContain('docker system prune')
    expect(verdict?.decision).toBe('deny')
  })

  test('空字符串按"未配置"处理：不会把内置高危清单清空', async () => {
    // 表单保存时未填的字段就是空串，不能因为用户没动它就把默认清单清掉
    await configure({ autoApprove: '', confirmCommands: '' })
    const hook = createApprovalGuardHook()!
    const asked: string[] = []

    const verdict = await hook(
      ctxOf({
        toolName: 'run_command',
        command: 'rm -rf build',
        askUser: async () => {
          asked.push('asked')
          return { approved: false, answeredBy: 'user' }
        },
      })
    )

    expect(asked).toHaveLength(1)
    expect(verdict?.decision).toBe('deny')
  })

  test('类型压根不对（数字 / 对象）时回落默认，而不是崩溃或全部免问', async () => {
    await configure({ autoApprove: 42, confirmCommands: { a: 1 } })
    const hook = createApprovalGuardHook()!
    // 回落默认 → 白名单为空 → 普通写工具照常问
    expect(await hook(ctxOf({ toolName: 'write_file' }))).toBeUndefined()
  })
})
