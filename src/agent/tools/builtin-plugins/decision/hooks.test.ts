/**
 * 决策插件的钩子：工具路由与回执核对（设计文档 §6.5 第一行）。
 *
 * 这是**钩子机制的第一个真实消费者**，所以这些用例除了验行为，也顺带钉住
 * "插件能解释自己做了什么"这件事：收窄了多少、为什么没干预、请求与实际是否一致。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { AssistantMessage, AgentTool } from '../../../core/types'
import { savePluginConfig } from '../../../config'
import { parseToolRouting, readToolRouting, DECISION_PLUGIN_ID } from './config'
import { createDecisionHooks } from './hooks'

let homeDir = ''
let oldHome: string | undefined

beforeEach(async () => {
  homeDir = await mkdtemp(join(tmpdir(), 'ada-decision-hooks-'))
  oldHome = process.env.A_DA_HOME
  process.env.A_DA_HOME = homeDir
})

afterEach(async () => {
  if (oldHome !== undefined) process.env.A_DA_HOME = oldHome
  else delete process.env.A_DA_HOME
  await rm(homeDir, { recursive: true, force: true }).catch(() => {})
})

const tool = (name: string): AgentTool => ({
  name,
  description: name,
  parameters: { type: 'object' },
  async execute() {
    return { output: name, ok: true }
  },
})

const assistant: AssistantMessage = {
  role: 'assistant',
  content: '做完了',
  thinking: '',
  toolCalls: [],
  timestamp: 1,
}

/** 取钩子返回的工具名；`tools` 的契约类型含尚未实现的 'casual'，这里统一收口。 */
const namesOf = (result: { tools?: AgentTool[] | 'casual' } | undefined): string[] | undefined =>
  Array.isArray(result?.tools) ? result.tools.map((entry) => entry.name) : undefined

const turnContext = (tools: string[]) => ({
  step: 0,
  messages: [],
  tools: tools.map(tool),
  kind: 'main' as const,
  threadId: 'thread-1',
})

describe('parseToolRouting', () => {
  test('接受空格、中英文逗号混用', () => {
    expect(parseToolRouting('read_file search_files,run_command')).toEqual([
      'read_file',
      'search_files',
      'run_command',
    ])
    expect(parseToolRouting('read_file，search_files')).toEqual(['read_file', 'search_files'])
  })

  test('空值与非字符串一律当作不干预', () => {
    expect(parseToolRouting('')).toEqual([])
    expect(parseToolRouting('   ')).toEqual([])
    expect(parseToolRouting(undefined)).toEqual([])
    expect(parseToolRouting(['read_file'])).toEqual([])
  })
})

describe('工具路由：默认不干预', () => {
  test('没配 toolRouting 时，beforeTurn 不表态、afterTurn 不做事', async () => {
    const hooks = createDecisionHooks()
    const before = await hooks.beforeTurn!(turnContext(['read_file', 'run_command']))
    expect(before).toBeUndefined()

    const after = await hooks.afterTurn!({
      step: 0,
      message: assistant,
      toolResults: [],
      effectiveToolNames: ['read_file', 'run_command'],
      llmDurationMs: 1,
      toolsDurationMs: 1,
      kind: 'main',
      threadId: 'thread-1',
    })
    expect(after).toBeUndefined()
  })
})

describe('工具路由：配置后生效', () => {
  test('只保留白名单里的工具，并把收窄幅度说出来', async () => {
    await savePluginConfig(DECISION_PLUGIN_ID, { toolRouting: 'read_file get_outline' })
    const traces: string[] = []

    const hooks = createDecisionHooks()
    const result = await hooks.beforeTurn!({
      ...turnContext(['read_file', 'run_command', 'get_outline']),
      trace: (message) => traces.push(message),
    })

    expect(namesOf(result)).toEqual(['read_file', 'get_outline'])
    expect(traces.some((line) => line.includes('保留 2/3'))).toBe(true)
  })

  test('配置的名字一个都没命中时不做任何改动，并说明原因', async () => {
    await savePluginConfig(DECISION_PLUGIN_ID, { toolRouting: 'read_flie run_comand' })
    const traces: string[] = []

    const hooks = createDecisionHooks()
    // 拼错了两个名字：宁可不动工具表，也不能让模型突然没有工具可用
    const result = await hooks.beforeTurn!({
      ...turnContext(['read_file', 'run_command']),
      trace: (message) => traces.push(message),
    })

    expect(result).toBeUndefined()
    expect(traces.some((line) => line.includes('都不在当前工具表里'))).toBe(true)
  })

  test('配置改动下一轮就生效（不缓存）', async () => {
    const hooks = createDecisionHooks()
    await savePluginConfig(DECISION_PLUGIN_ID, { toolRouting: 'read_file' })
    expect(namesOf(await hooks.beforeTurn!(turnContext(['read_file', 'run_command'])))).toEqual([
      'read_file',
    ])

    await savePluginConfig(DECISION_PLUGIN_ID, { toolRouting: 'run_command' })
    expect(namesOf(await hooks.beforeTurn!(turnContext(['read_file', 'run_command'])))).toEqual([
      'run_command',
    ])

    expect(await readToolRouting()).toEqual(['run_command'])
  })
})

describe('回执核对：事前决策与事后实际', () => {
  test('实际生效与请求一致时不说话', async () => {
    await savePluginConfig(DECISION_PLUGIN_ID, { toolRouting: 'read_file' })
    const traces: string[] = []
    const hooks = createDecisionHooks()

    await hooks.beforeTurn!({ ...turnContext(['read_file', 'run_command']), threadId: 'same' })
    await hooks.afterTurn!({
      step: 0,
      message: assistant,
      toolResults: [],
      effectiveToolNames: ['read_file'],
      llmDurationMs: 1,
      toolsDurationMs: 0,
      kind: 'main',
      threadId: 'same',
    })
    expect(traces).toEqual([])
  })

  test('实际与请求不一致时如实报告（这就是成对要解决的问题）', async () => {
    await savePluginConfig(DECISION_PLUGIN_ID, { toolRouting: 'read_file' })
    const traces: string[] = []
    const hooks = createDecisionHooks()

    await hooks.beforeTurn!({ ...turnContext(['read_file', 'run_command']), threadId: 'diff' })
    await hooks.afterTurn!({
      step: 0,
      message: assistant,
      toolResults: [],
      // 比如被更高优先级的钩子又收窄了一层
      effectiveToolNames: ['read_file', 'run_command'],
      llmDurationMs: 1,
      toolsDurationMs: 0,
      kind: 'main',
      threadId: 'diff',
      trace: (message) => traces.push(message),
    })

    expect(traces.some((line) => line.includes('不一致'))).toBe(true)
  })

  test('没有事前请求时 afterTurn 不表态（不能凭空报告不一致）', async () => {
    const traces: string[] = []
    const hooks = createDecisionHooks()
    const result = await hooks.afterTurn!({
      step: 7,
      message: assistant,
      toolResults: [],
      effectiveToolNames: ['read_file'],
      llmDurationMs: 1,
      toolsDurationMs: 0,
      kind: 'main',
      threadId: 'never-requested',
      trace: (message) => traces.push(message),
    })

    expect(result).toBeUndefined()
    expect(traces).toEqual([])
  })
})
