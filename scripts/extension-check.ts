/**
 * 扩展插件这条链路，在真实窗口里走一遍。
 *
 *   bun scripts/extension-check.ts
 *
 * 用的是仓库里那个 web_search 扩展（`.ada/extensions/web-search.ts`），因为它会真的
 * 发一次网络请求，最能说明问题。整条链路是：
 *
 *   加载（jiti 跑 .ts）→ 注册进 ToolRegistry → 出现在发给模型的工具表里
 *   → 模型调用它 → 真的去搜 → 结果作为工具结果回给模型
 *
 * 第 1 步和第 4 步都从 mock 端点那边看：它拿得到请求体，所以既能看到工具表，
 * 也能看到搜索结果本身。需要联网。
 */

import { mkdirSync } from 'node:fs'
import path from 'node:path'
import { launch } from '@gpuix/react/automation'

const root = path.join(import.meta.dir, '..')
mkdirSync(path.join(root, 'tmp'), { recursive: true })

const QUERY = 'bun runtime 是什么'
let sawTool: string[] = []
let toolResult = ''

function sse(chunks: unknown[]): string {
  return `${chunks.map((chunk) => `data: ${JSON.stringify(chunk)}`).join('\n\n')}\n\ndata: [DONE]\n\n`
}

const server = Bun.serve({
  port: 0,
  async fetch(request) {
    const body = (await request.json()) as {
      messages: { role: string; content?: string }[]
      tools?: { function: { name: string } }[]
    }
    sawTool = (body.tools ?? []).map((tool) => tool.function.name)

    const outcome = body.messages.find((message) => message.role === 'tool')
    if (outcome) {
      toolResult = outcome.content ?? ''
      return new Response(sse([{ choices: [{ delta: { content: '看到了，谢谢。' } }] }]), {
        headers: { 'content-type': 'text/event-stream' },
      })
    }

    return new Response(
      sse([
        {
          choices: [
            {
              delta: {
                tool_calls: [
                  {
                    index: 0,
                    id: 'call_search',
                    function: { name: 'web_search', arguments: JSON.stringify({ query: QUERY }) },
                  },
                ],
              },
            },
          ],
        },
      ]),
      { headers: { 'content-type': 'text/event-stream' } }
    )
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
    A_DA_BASE_URL: `http://127.0.0.1:${server.port}/v1`,
  },
})

const panel = async (): Promise<string> => (await app.call('getAllText', {})).text.join('\n')

try {
  await app.getByTestId('composer').waitFor({ timeoutMs: 60_000 })
  // 只看不点：把事件日志打开，加载与调用都记在里面。
  await app.getByTestId('debug').click()
  await app.getByTestId('debug-on').click()
  await app.getByTestId('approval').click()
  await app.getByTestId('approval-auto').click()
  await app.getByTestId('composer').fill('搜一下 bun 是什么')
  await app.getByTestId('composer').press('enter')

  // 等到工具结果回来（mock 端拿到它，就说明这一次调用真的跑完了）。
  const started = Date.now()
  while (Date.now() - started < 60_000 && !toolResult) {
    await new Promise((resolve) => setTimeout(resolve, 200))
  }

  const log = await panel()

  try {
    if (!sawTool.includes('web_search')) {
      throw new Error(`模型拿到的工具表里没有 web_search：${sawTool.join(', ')}`)
    }
    console.log('ok   扩展工具进了发给模型的工具表')

    if (!log.includes('已加载扩展工具')) {
      throw new Error('事件日志里没有加载记录')
    }
    console.log('ok   事件日志里有「已加载扩展工具」')

    if (!toolResult) {
      throw new Error('等了一分钟也没等到 web_search 的结果')
    }
    if (!/https?:\/\//.test(toolResult)) {
      throw new Error(`结果里没有链接，多半没搜到：\n${toolResult.slice(0, 400)}`)
    }
    const first = toolResult.split('\n').slice(0, 3).join(' / ').slice(0, 160)
    console.log(`ok   真的搜到了（${toolResult.length} 字符），头一条：${first}`)
  } catch (error) {
    // 出事时把两边的原文都摆出来：面板里能看到加载与调用谁先谁后（每行带时间），
    // 工具结果就是模型那边真正收到的东西。
    console.log(`---- 事件日志（新在上）----\n${log.split('\n').slice(0, 30).join('\n')}\n----`)
    console.log(`---- 工具结果 ----\n${toolResult.slice(0, 600) || '(空)'}\n----`)
    throw error
  }

  await app.clock.pause()
  await app.screenshot({ path: path.join(root, 'tmp', 'extension.png') })
  await app.clock.resume()
} finally {
  await app.close()
  server.stop(true)
}

console.log('[extension-check] wrote tmp/extension.png')
