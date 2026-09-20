/**
 * Drive a whole turn in a real window, against a local mock endpoint.
 *
 * This is the live counterpart of `src/agent/model.test.tsx`: same flow, but
 * through a child process and the real GPUI window, so it also proves the paint
 * path end to end. `GPUIX_BACKGROUND=1` keeps the window from stealing focus.
 *
 *   bun scripts/smoke.ts
 */

import { mkdirSync, rmSync } from 'node:fs'
import path from 'node:path'
import { launch } from '@gpuix/react/automation'

const root = path.join(import.meta.dir, '..')
mkdirSync(path.join(root, 'tmp'), { recursive: true })

const FILE = 'tmp/agent-note.md'
const CONTENT = '# 由 mock 模型写入\n\n- a_da 正在工作区里干活\n- 这条内容来自一次真实的工具调用\n'

// 上一次跑剩下的文件会让这一轮「内容没变」，patch 就是空的，卡片里也就没有 diff
// 可看。每次都从没有这个文件开始。
rmSync(path.join(root, FILE), { force: true })

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
          // 真实端点（DeepSeek 一类）会在正文之前先吐思考链，这里也吐一段，
          // 好让截图里有那行可折叠的「思考」。
          {
            choices: [
              {
                delta: {
                  reasoning_content:
                    '用户想要一个说明文件。先确认工作区，然后写 tmp/agent-note.md，内容说明我能做什么。',
                },
              },
            ],
          },
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

/**
 * 这一轮跑完了：跑完的行不再挂状态字，所以「等待批准」消失就是终点。
 * （模型回复本身是 `<markdown>`，文字定位器看不见它。）
 */
const settled = async (): Promise<void> => {
  const started = Date.now()
  while (Date.now() - started < 30_000) {
    const text = (await app.call('getAllText', {})).text.join('\n')
    if (!text.includes('等待批准')) return
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error('这一轮没有在 30 秒内结束')
}
await settled()

/**
 * 展开那张工具行，让截图里有 diff。
 *
 * 只能按坐标点，而且要先移一下：
 * - 会话区是虚拟列表（`estimatedItemHeight`），元素边界是估算出来的，文字定位器
 *   给的点会偏几十像素（实测报 625、实际画在 561）
 * - 不先移动就发的合成点击在真实窗口里落不到处理器上（真人用鼠标本来就是先移过去
 *   再按下）
 * 这个 y 是这段固定脚本内容里那一行的位置。
 */
await app.call('mouseMove', { x: 500, y: 576 })
await new Promise((resolve) => setTimeout(resolve, 250))
await app.mouse.click({ x: 500, y: 576 })
await app.getByText('思考').waitFor({ timeoutMs: 10_000 })

await new Promise((resolve) => setTimeout(resolve, 800))
await app.clock.pause()
await app.screenshot({ path: path.join(root, 'tmp', 'agent.png') })
await app.clock.resume()
await app.close()
server.stop(true)

console.log('[smoke] wrote tmp/agent.png')
