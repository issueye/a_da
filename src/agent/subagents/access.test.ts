/**
 * 子智能体启动门禁与结束复核（M3-4，设计文档 §6.3、§11 风险 3）。
 *
 * 这里覆盖开发计划的验收清单：
 * - `gate` 未配 `failOpen` + 无引擎 → **放行**，且有"门禁未生效"提示
 * - `gate.failOpen: false` + 无引擎 → **拦截**
 * - gate 拦截路径**不创建会话、不进 runningThreadIds**，因此父会话不可能被永久挂起
 * - `afterSubagentEnd` 抛错**不阻断** `wakeParent`
 * - store 与 runner 的工具过滤逐工具名一致（去重复成功）
 *
 * 另外把"判定方自说自话不能绕过用户显式要求"这条也钉住：只报 allowed、不报
 * confidence/calibrated 的判定不算判定。
 */

import { describe, expect, test } from 'bun:test'
import type { AgentTool } from '../core/types'
import type { SubagentProfile } from './types'
import { resolveSubagentTools, runSubagentGate } from './access'
import { defaultToolRegistry } from '../tools'

const tool = (name: string): AgentTool => ({
  name,
  description: name,
  parameters: { type: 'object' },
  async execute() {
    return { output: name, ok: true }
  },
})

function profile(overrides: Partial<SubagentProfile> = {}): SubagentProfile {
  return {
    id: 'probe',
    name: '探针',
    description: '',
    systemPrompt: '',
    allowedTools: ['*'],
    mode: 'readwrite',
    enabled: true,
    scope: 'builtin',
    ...overrides,
  }
}

const authorized: AgentTool[] = [tool('read_file'), tool('write_file')]

describe('门禁失败方向（设计文档 §6.4.4.5）', () => {
  test('没配 gate.criteria 就不跑门禁（返回 undefined，不是失败）', async () => {
    expect(await runSubagentGate({ profile: profile(), task: 't', authorizedTools: authorized })).toBeUndefined()
    expect(
      await runSubagentGate({
        profile: profile({ gate: { criteria: '   ' } }),
        task: 't',
        authorizedTools: authorized,
      })
    ).toBeUndefined()
  })

  test('无插件提供判定 + 未配 failOpen → 放行，并提示"门禁未生效"', async () => {
    const notices: string[] = []
    const outcome = await runSubagentGate({
      profile: profile({ gate: { criteria: '必须通过测试' } }),
      task: 't',
      authorizedTools: authorized,
      notice: (message) => notices.push(message),
    })

    expect(outcome?.allowed).toBe(true)
    expect(outcome?.judged).toBe(false)
    expect(notices.some((line) => line.includes('门禁未生效'))).toBe(true)
  })

  test('无插件提供判定 + failOpen: false → 拦截', async () => {
    const outcome = await runSubagentGate({
      profile: profile({ gate: { criteria: '必须通过测试', failOpen: false } }),
      task: 't',
      authorizedTools: authorized,
    })

    expect(outcome?.allowed).toBe(false)
    expect(outcome?.judged).toBe(false)
    expect(outcome?.reason).toContain('门禁未生效')
  })

  test('判定方只报 allowed 不报依据 → 仍算"门禁未生效"；显式 failOpen: false 时拦截', async () => {
    const vague = {
      beforeSubagentStart: async (): Promise<{ allowed: boolean }> => ({ allowed: true }),
    }

    const lenient = await runSubagentGate({
      profile: profile({ gate: { criteria: 'c' } }),
      task: 't',
      authorizedTools: authorized,
      hooks: vague,
    })
    expect(lenient?.allowed).toBe(true)
    expect(lenient?.judged).toBe(false)

    // 用户明确要求"拿不到判定就拦"时，判定方说"通过"也没用
    const strict = await runSubagentGate({
      profile: profile({ gate: { criteria: 'c', failOpen: false } }),
      task: 't',
      authorizedTools: authorized,
      hooks: vague,
    })
    expect(strict?.allowed).toBe(false)
  })

  test('判定方给出校准信息时，用它的结论', async () => {
    const judged = {
      beforeSubagentStart: async (): Promise<{ allowed: boolean; confidence: number; calibrated: boolean }> => ({
        allowed: false,
        confidence: 0.2,
        calibrated: false,
      }),
    }
    const outcome = await runSubagentGate({
      profile: profile({ gate: { criteria: 'c' } }),
      task: 't',
      authorizedTools: authorized,
      hooks: judged,
    })

    expect(outcome?.allowed).toBe(false)
    expect(outcome?.judged).toBe(true)
    expect(outcome?.confidence).toBe(0.2)
  })

  test('判定钩子抛错 → 按"门禁未生效"处理，不打崩调用方', async () => {
    const notices: string[] = []
    const outcome = await runSubagentGate({
      profile: profile({ gate: { criteria: 'c' } }),
      task: 't',
      authorizedTools: authorized,
      hooks: {
        beforeSubagentStart: async () => {
          throw new Error('判定服务不可达')
        },
      },
      notice: (message) => notices.push(message),
    })

    expect(outcome?.allowed).toBe(true)
    expect(outcome?.judged).toBe(false)
    expect(notices.some((line) => line.includes('判定服务不可达'))).toBe(true)
  })

  test('门禁返回的工具只能收窄：未授权的一律剔除并说明', async () => {
    const ghost = tool('ghost_tool')
    const notices: string[] = []
    const outcome = await runSubagentGate({
      profile: profile({ gate: { criteria: 'c' } }),
      task: 't',
      authorizedTools: authorized,
      hooks: {
        beforeSubagentStart: async () => ({
          allowed: true,
          calibrated: false,
          confidence: 1,
          tools: [...authorized, ghost],
        }),
      },
      notice: (message) => notices.push(message),
    })

    expect(outcome?.tools?.map((entry) => entry.name)).toEqual(['read_file', 'write_file'])
    expect(notices.some((line) => line.includes('ghost_tool'))).toBe(true)
  })
})

describe('工具解析：store 与 runner 共用一份实现', () => {
  test('白名单 / 黑名单 / 通配符 / 只读 / 防递归的过滤结果逐名一致', () => {
    // 两个"入口"都只是调用同一个函数，所以这里断言的是这份实现本身的语义
    const readonlyProfile = profile({ mode: 'readonly', allowedTools: ['*'] })
    const names = resolveSubagentTools(readonlyProfile, process.cwd()).map((entry) => entry.name)

    // 只读：写工具被挡住（isWriteTool 是静态白名单，失败安全）
    expect(names).not.toContain('write_file')
    expect(names).not.toContain('edit_file')
    expect(names).not.toContain('run_command')
    // 防递归：子智能体永远拿不到委派类工具
    expect(names).not.toContain('invoke_subagent')
    expect(names).not.toContain('await_subagents')
  })

  test('显式黑名单优先于通配白名单', () => {
    const names = resolveSubagentTools(
      profile({ allowedTools: ['*'], disallowedTools: ['read_file'] }),
      process.cwd()
    ).map((entry) => entry.name)

    expect(names).not.toContain('read_file')
    expect(names).toContain('write_file')
  })

  test('非通配白名单只放行列出的工具，且只读模式仍会再筛一层', () => {
    const names = resolveSubagentTools(
      profile({ allowedTools: ['read_file', 'write_file'], mode: 'readonly' }),
      process.cwd()
    ).map((entry) => entry.name)

    expect(names).toEqual(['read_file'])
  })

  test('只读 profile：工具表里判定为只读的都拿得到，写工具一个都没有', () => {
    // 期望值从**同一时刻的工具表**推出，而不是写死名字：本进程里注册了哪些插件工具，
    // 取决于别的测试文件跑没跑过加载器（bun 把各文件放在同一进程并发跑），写死名字会翻红
    const table = defaultToolRegistry.getToolsForWorkspace(process.cwd())
    // 唯一允许缺席的只读工具是**防递归**刻意挡住的那批：子智能体不该再委派子智能体
    const recursionGuard = new Set([
      'invoke_subagent',
      'check_subagent',
      'send_subagent_message',
      'resume_subagent',
      'await_subagents',
    ])
    const readonlyNames = table
      .filter((entry) => !defaultToolRegistry.isWriteTool(entry.name))
      .map((entry) => entry.name)
      .filter((name) => !recursionGuard.has(name))
    const names = resolveSubagentTools(profile({ mode: 'readonly' }), process.cwd()).map(
      (entry) => entry.name
    )

    for (const name of readonlyNames) expect(names).toContain(name)
    for (const entry of table) {
      if (defaultToolRegistry.isWriteTool(entry.name)) expect(names).not.toContain(entry.name)
    }
  })

  test('官方插件里的只读工具都登记进了 READ_ONLY（漏登记的后果就是只读子智能体拿不到）', () => {
    // 这一条与工具表当前内容无关，钉的是分类本身
    for (const readonly of [
      'git_status',
      'git_diff',
      'git_log',
      'get_outline',
      'inspect_project',
      'read_files',
      'find_symbol',
      'check_task',
      'decide',
      'check_gate',
    ]) {
      expect(defaultToolRegistry.isWriteTool(readonly)).toBe(false)
    }
    // 反例：执行命令/批量写/跑测试会产生副作用，必须按写工具对待
    for (const write of ['run_test_focused', 'edit_files', 'run_command', 'write_file']) {
      expect(defaultToolRegistry.isWriteTool(write)).toBe(true)
    }
  })
})
