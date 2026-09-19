/**
 * Drive a whole turn in a real window, against a local mock endpoint.
 *
 * This is the live counterpart of `src/agent/model.test.tsx`: same flow, but
 * through a child process and the real GPUI window, so it also proves the paint
 * path end to end. `GPUIX_BACKGROUND=1` keeps the window from stealing focus.
 *
 *   bun scripts/smoke.ts
 */

import { mkdirSync } from 'node:fs'
import path from 'node:path'
import { launch } from '@gpuix/react/automation'

const root = path.join(import.meta.dir, '..')
mkdirSync(path.join(root, 'tmp'), { recursive: true })

const FILE = 'tmp/agent-note.md'
const CONTENT = '# 由 mock 模型写入\n\n- a_da 正在工作区里干活\n- 这条内容来自一次真实的工具调用\n'

function sse(chunks: unknown[]): string {
  return `${chunks.map((chunk) => `data: ${JSON.stringify(chunk)}`).join('\n\n')}\n\ndata: [DONE]\n\n`
}

const server = Bun.serve({
  port: 8791,
  async fetch(request) {
    const body = (await request.json()) as { messages: { role: string }[] }
    const secondRound = body.messages.some((message) => message.role === 'tool')
    const stream = secondRound
      ? sse([
          { choices: [{ delta: { content: '已写入 `' } }] },
          { choices: [{ delta: { content: `${FILE}` } }] },
          { choices: [{ delta: { content: '`，内容如上。\n\n下一步可以让我运行测试。' } }] },
        ])
      : sse([
          { choices: [{ delta: { content: '先看一下工作区，然后写一个说明文件。\n' } }] },
          {
            choices: [
              {
                delta: {
                  tool_calls: [
                    { index: 0, id: 'call_1', function: { name: 'write_file', arguments: '{"path":"' } },
                  ],
                },
              },
            ],
          },
          {
            choices: [
              {
                delta: {
                  tool_calls: [
                    { index: 0, function: { name: 'write_file', arguments: `${FILE}","content":"` } },
                  ],
                },
              },
            ],
          },
          {
            choices: [
              {
                delta: {
                  tool_calls: [
                    {
                      index: 0,
                      function: {
                        name: 'write_file',
                        arguments: `${CONTENT.replace(/\n/g, '\\n')}"}`,
                      },
                    },
                  ],
                },
              },
            ],
          },
        ])
    return new Response(stream, { headers: { 'content-type': 'text/event-stream' } })
  },
})

const app = await launch({
  command: 'bun',
  args: ['app.tsx'],
  cwd: root,
  env: {
    GPUIX_BACKGROUND: '1',
    A_DA_API_KEY: 'mock',
    A_DA_MODEL: 'mock-model',
    A_DA_BASE_URL: 'http://127.0.0.1:8791/v1',
  },
})

await app.getByTestId('composer').waitFor({ timeoutMs: 60_000 })
await app.getByTestId('approval').click()
await app.getByTestId('approval-ask').click()
await app.getByTestId('composer').fill('写一个 agent-note.md，说明你能做什么')
await app.getByTestId('composer').press('enter')

// The gate holds the write until the card is answered.
await app.getByText('等待批准').waitFor({ timeoutMs: 30_000 })
await app.getByTestId('approve').click()
await app.getByText('完成').waitFor({ timeoutMs: 30_000 })

await new Promise((resolve) => setTimeout(resolve, 800))
await app.clock.pause()
await app.screenshot({ path: path.join(root, 'tmp', 'agent.png') })
await app.clock.resume()
await app.close()
server.stop(true)

console.log('[smoke] wrote tmp/agent.png')
