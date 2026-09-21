/**
 * The model path, against a local mock endpoint.
 *
 * A real API key cannot be part of a test, so this serves an OpenAI-compatible
 * SSE stream from an ephemeral port and points the store at it. That exercises
 * the streaming parser, the tool loop, the approval gate and the diff card —
 * every path the user actually takes, with no network and no key.
 *
 * Model replies render as `<markdown>`, which paints inside GPUI and is
 * invisible to a text locator, so the waits read `getPaintedText()`.
 *
 *   bun test
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { existsSync } from 'node:fs'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import React from 'react'
import { connectTest } from '@gpuix/react/automation'
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing'
import { AgentWindow } from '../AgentWindow'
import { store } from './store'
import { defaultToolRegistry } from './tools'

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const FILE = 'note.md'
const CONTENT = '# hello from the mock model\n'

function sse(chunks: unknown[]): string {
  return `${chunks.map((chunk) => `data: ${JSON.stringify(chunk)}`).join('\n\n')}\n\ndata: [DONE]\n\n`
}

function toolCallChunk(id: string, name: string, argsDelta: string) {
  return {
    choices: [{ delta: { tool_calls: [{ index: 0, id, function: { name, arguments: argsDelta } }] } }],
  }
}

function textChunk(text: string) {
  return { choices: [{ delta: { content: text } }] }
}

/** DeepSeek 一类的端点在正文之前先吐思考链，字段名是 reasoning_content。 */
function reasoningChunk(text: string) {
  return { choices: [{ delta: { reasoning_content: text } }] }
}

const REASONING = '用户要一个说明文件，先看看工作区里有什么。'

let workspaces: string[] = []
let workspace = ''
let server: ReturnType<typeof Bun.serve>
let requests = 0
/** 最近一次请求里模型拿到的工具表——工具是注册中心给的，不是写死的常量。 */
let lastTools: string[] = []

beforeAll(() => {
  server = Bun.serve({
    port: 0,
    async fetch(request) {
      const body = (await request.json()) as {
        messages: { role: string }[]
        tools?: { function: { name: string } }[]
      }
      requests += 1
      lastTools = (body.tools ?? []).map((tool) => tool.function.name)
      const sawToolResult = body.messages.some((message) => message.role === 'tool')

      // First round: reasoning, then a write_file whose arguments arrive in
      // pieces, the way a real endpoint streams them. Second round: the closing
      // summary.
      const stream = sawToolResult
        ? sse([textChunk('已写入 '), textChunk('`'), textChunk(FILE), textChunk('`。')])
        : sse([
            reasoningChunk(REASONING),
            textChunk('我先写一个文件。'),
            toolCallChunk('call_1', 'write_file', '{"path":"'),
            toolCallChunk('call_1', 'write_file', FILE),
            toolCallChunk('call_1', 'write_file', '","content":"# hello from the mock model\\n"}'),
          ])

      return new Response(stream, { headers: { 'content-type': 'text/event-stream' } })
    },
  })

  store.newThread(process.cwd())
  process.env.A_DA_API_KEY = 'test-key'
  process.env.A_DA_MODEL = 'mock-model'
  process.env.A_DA_BASE_URL = `http://127.0.0.1:${server.port}/v1`
})

afterAll(async () => {
  delete process.env.A_DA_API_KEY
  delete process.env.A_DA_MODEL
  delete process.env.A_DA_BASE_URL
  server.stop(true)
  for (const dir of workspaces) await rm(dir, { recursive: true, force: true })
})

async function mount(approval: 'auto' | 'ask' | 'readonly') {
  // Every test gets its own folder, so a file left by an earlier test cannot
  // make a later "nothing was written" check pass for the wrong reason.
  workspace = await mkdtemp(join(tmpdir(), 'a-da-model-'))
  workspaces.push(workspace)
  store.setApproval(approval)
  // A thread owns its project, so the mock runs against this folder.
  store.newThread(workspace)
  const { render, renderer } = createTestRoot({ width: 1120, height: 760 })
  render(<AgentWindow />)
  const app = await connectTest(renderer)

  const screen = () => renderer.getPaintedText().join('\n')
  const painted = async (needle: string, timeoutMs = 15_000) => {
    const started = Date.now()
    while (Date.now() - started < timeoutMs) {
      if (screen().includes(needle)) return
      renderer.flush?.()
      await new Promise((resolve) => setTimeout(resolve, 60))
    }
    throw new Error(`the frame never painted ${needle}\n${screen()}`)
  }
  const ask = async (text: string) => {
    await app.getByTestId('composer').fill(text)
    await app.getByTestId('composer').press('enter')
  }
  return { app, screen, painted, ask }
}

describeNative('the model loop', () => {
  test(
    'streams an answer, asks before writing, then writes on approval',
    async () => {
      const { app, screen, painted, ask } = await mount('ask')

      await ask('写一个 note.md')
      await painted('我先写一个文件。')
      await painted('等待批准')
      expect(existsSync(join(workspace, FILE))).toBe(false)

      await app.getByTestId('approve').click()

      await painted('已写入 note.md。')
      expect(await readFile(join(workspace, FILE), 'utf8')).toBe(CONTENT)

      // 完成后除了报告外的内容收缩到一起，点击过程条展开后，工具卡片默认收起只留一行；再展开 `<diff>`
      await app.getByText('执行过程').click()
      expect(screen()).not.toContain('@@ -1,0 +1,1 @@')
      await app.getByText('写入文件').click()
      await painted('@@ -1,0 +1,1 @@')
      expect(screen()).toContain('# hello from the mock model')
      expect(requests).toBe(2)

      await app.close()
    },
    30_000,
  )

  test(
    'shows the model reasoning as a row that opens on click',
    async () => {
      // 用自动批准：这个用例只关心思考行，但一轮必须真的跑完——停在审批闸门上的
      // 那一轮会让 store.running 一直是真，后面每个用例的输入都会变成排队。
      const { app, screen, painted, ask } = await mount('auto')

      await ask('写一个 note.md')
      await painted('已写入 note.md。')

      // 完成后过程收缩到一起，展开执行过程卡片查看思考持续时长与推理原文
      await app.getByText('执行过程').click()
      await painted('思考')
      // 收起时只有一行：时长写在标签旁边，推理原文不占地方。
      expect(screen()).toContain('持续')
      expect(screen()).not.toContain(REASONING)

      await app.getByText('思考').click()
      await painted(REASONING)

      await app.close()
    },
    30_000,
  )

  test(
    'a rejected call is not written and is reported back to the model',
    async () => {
      const { app, screen, painted, ask } = await mount('ask')

      await ask('再写一次 note.md')
      await painted('等待批准')
      await app.getByTestId('deny').click()

      // 拒绝后整轮结束，过程收缩到一起，展开执行过程卡片查看拒绝理由
      await painted('已拒绝')
      await app.getByText('执行过程').click()
      await app.getByText('写入文件').click()
      await painted('已拒绝执行')
      // The refusal is fed back to the model instead of the tool result.
      const lastToolMessage = store.active.messages.find((message) => message.role === 'toolResult')
      expect(lastToolMessage?.content).toContain('用户拒绝了这次调用')
      expect(existsSync(join(workspace, FILE))).toBe(false)

      await app.close()
    },
    30_000,
  )

  test(
    '自动批准 writes without a prompt',
    async () => {
      const { app, painted, ask } = await mount('auto')

      await ask('自动写一次')
      await painted('已写入 note.md。')
      expect(await readFile(join(workspace, FILE), 'utf8')).toBe(CONTENT)

      await app.close()
    },
    30_000,
  )

  test(
    'a thread keeps its own project when another one is opened',
    async () => {
      const { app, painted, ask } = await mount('ask')
      const owning = workspace
      const other = await mkdtemp(join(tmpdir(), 'a-da-model-'))
      workspaces.push(other)

      await ask('写一个 note.md')
      await painted('等待批准')

      // Open a second project while the write is waiting for an answer. The
      // pending call must still belong to the thread that asked for it.
      const pending = store.active
      store.newThread(other)
      await painted('只能访问当前项目内的文件')
      store.selectThread(pending.id)
      await painted('等待批准')

      await app.getByTestId('approve').click()
      await painted('已写入 note.md。')

      expect(await readFile(join(owning, FILE), 'utf8')).toBe(CONTENT)
      expect(existsSync(join(other, FILE))).toBe(false)
      // The other project is still its own thread list.
      expect(store.projects.slice(0, 2)).toEqual([other, owning])

      await app.close()
    },
    30_000,
  )

  test(
    'advertises registry tools — including extensions — to the model',
    async () => {
      const { app, painted, ask } = await mount('auto')

      // 扩展注册进来的工具必须出现在发给模型的工具表里，否则就是「装了但叫不到」。
      defaultToolRegistry.register({
        name: 'custom_probe',
        description: '扩展提供的工具',
        parameters: { type: 'object', properties: {} },
        async execute() {
          return { output: 'ok', ok: true }
        },
      })

      try {
        await ask('写一个 note.md')
        await painted('已写入 note.md。')

        expect(lastTools).toContain('read_file')
        expect(lastTools).toContain('write_file')
        expect(lastTools).toContain('custom_probe')
      } finally {
        defaultToolRegistry.unregister('custom_probe')
        await app.close()
      }
    },
    30_000,
  )
})
