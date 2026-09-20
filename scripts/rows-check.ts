/**
 * Look at a long run of tool rows in a real window.
 *
 *   bun scripts/rows-check.ts
 *
 * 这三件事只有像素能回答，所以这里直接量：
 * - 行距：一屏该装下几十次调用，而不是被空隙吃掉一半
 * - 收起行下面**不该有线**：盒子收起时若still渲染，一个没有内容的带边框容器会塌成
 *   一条 1px 的线，看起来就像每行都带下划线（这条线曾经真的存在）
 * - 长参数：超出宽度要用省略号收尾，而不是被内容列硬裁
 *
 * 判据来自 `png-pixels.ts`：整条横线在很多 x 上同色，文字抗锯齿只在个别 x 上撞色。
 */

import { mkdirSync } from 'node:fs'
import path from 'node:path'
import { launch } from '@gpuix/react/automation'
import { readPng } from './png-pixels'

const root = path.join(import.meta.dir, '..')
mkdirSync(path.join(root, 'tmp'), { recursive: true })

const ROWS = 8 // 与下面 CALLS 的条数一致
const LONG_COMMAND =
  'powershell -NoProfile -Command "Get-ChildItem -Path . -Directory -Force -Filter \'.work\' -Recurse -Depth 2 | Select-Object -ExpandProperty FullName"'

const CALLS: { name: string; args: string }[] = [
  { name: 'read_file', args: '{"path":"package.json"}' },
  { name: 'list_files', args: '{"path":"src","depth":2}' },
  { name: 'search_files', args: '{"pattern":"workspace|task|board"}' },
  { name: 'run_command', args: JSON.stringify({ command: LONG_COMMAND }) },
  { name: 'read_file', args: '{"path":"src/AgentWindow.tsx"}' },
  { name: 'list_files', args: '{"path":"docs"}' },
  { name: 'write_file', args: '{"path":"tmp/rows-check.md","content":"# rows\\n\\n- one\\n- two\\n"}' },
  { name: 'search_files', args: '{"pattern":"useState"}' },
]

function sse(chunks: unknown[]): string {
  return `${chunks.map((chunk) => `data: ${JSON.stringify(chunk)}`).join('\n\n')}\n\ndata: [DONE]\n\n`
}

let step = 0
const server = Bun.serve({
  port: 0,
  async fetch() {
    const call = CALLS[step]
    step += 1
    // 最后一步什么都不给：模型没话说、也没工具要调，这一轮就结束了，于是会话区
    // 里只有用户那句话和这一列工具行——量行距时不会被别的东西干扰。
    const stream = call
      ? sse([
          {
            choices: [
              {
                delta: {
                  tool_calls: [
                    { index: 0, id: `call_${step}`, function: { name: call.name, arguments: call.args } },
                  ],
                },
              },
            ],
          },
        ])
      : sse([{ choices: [{ delta: {} }] }])
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
    A_DA_BASE_URL: `http://127.0.0.1:${server.port}/v1`,
  },
})

await app.getByTestId('composer').waitFor({ timeoutMs: 60_000 })
await app.getByTestId('approval').click()
await app.getByTestId('approval-auto').click()
await app.getByTestId('composer').fill('把项目过一遍')
await app.getByTestId('composer').press('enter')

/** 等到这一列都画出来：最后一条命令出现了就说明前面的也都跑完了。 */
const painted = async (needle: string, timeoutMs = 30_000): Promise<void> => {
  const started = Date.now()
  while (Date.now() - started < timeoutMs) {
    const text = (await app.call('getAllText', {})).text.join('\n')
    if (text.includes(needle)) return
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`没等到 ${needle}`)
}
let composerTop = 0
try {
  await painted('useState')
  // 输入框不在虚拟列表里，它的边界是准的：拿它当扫描的下界。
  composerTop = Math.floor((await app.getByTestId('composer').bounds()).y)
  await app.clock.pause()
  await app.screenshot({ path: path.join(root, 'tmp', 'rows.png') })
  await app.clock.resume()
} finally {
  await app.close()
  server.stop(true)
}

const png = readPng(path.join(root, 'tmp', 'rows.png'))
const at = (x: number, y: number): string => {
  const i = (y * png.width + x) * 4
  return `#${[png.pixels[i], png.pixels[i + 1], png.pixels[i + 2]]
    .map((v) => (v ?? 0).toString(16).padStart(2, '0'))
    .join('')}`
}

// 只扫会话区：输入框的上边框和占位文字也落在这一列 x 上，别把它们数成行。
const scanEnd = composerTop - 2

// 1) 收起行下面不该有整条横线（#ecece8 = C.cardBorder 的展开框边框色）。
const stray: number[] = []
for (let y = 200; y < scanEnd; y++) {
  if ([450, 600, 750, 900, 1000].every((x) => at(x, y) === '#ecece8')) stray.push(y)
}
if (stray.length) {
  throw new Error(`收起行下面出现了整条横线：y=${stray.join(', ')}`)
}
console.log('ok   收起行下面没有多余的横线')

// 2) 行距：一列工具行应该贴在一起。按「工具名那四个字」这一段的墨迹认行——
//    三角和图标是两个独立图形，各自成段，会把一行数成两行。
const bands: { top: number; bottom: number }[] = []
for (let y = 200; y < scanEnd; y++) {
  const inked = [345, 360, 380].some((x) => at(x, y) !== '#ffffff')
  const last = bands[bands.length - 1]
  if (inked) {
    if (last && y - last.bottom <= 10) last.bottom = y
    else bands.push({ top: y, bottom: y })
  }
}
// 一行文字有 ~10px 高；1px 的墨迹是输入框上边框之类的东西，不是行。
const rows = bands.filter((band) => band.bottom - band.top >= 4)
const pitch = rows.slice(1).map((band, i) => band.top - rows[i]!.top)
const max = Math.max(...pitch)
if (max > 32) {
  throw new Error(`行距太大：最大 ${max}px（期望 ≤32；这列间距是「行高 + 3」）`)
}
console.log(`ok   ${rows.length} 行工具调用的行距：${pitch.join(', ')}px`)

// 3) 长参数要收缩，不许顶到内容列右边缘：顶到就说明文字没收缩、被硬裁了，
//    而不是以省略号收尾。右边缘从用户气泡量出来——它右对齐，占着内容列的右端。
const rightmostInk = (y: number, from: number, to: number): number => {
  for (let x = to; x >= from; x--) if (at(x, y) !== '#ffffff') return x
  return 0
}
let columnRight = 0
for (let y = 380; y < rows[0]!.top - 6; y++) {
  const x = rightmostInk(y, 900, 1090)
  if (x > columnRight) columnRight = x
}
if (columnRight === 0) throw new Error('量不到用户气泡的右边缘，内容列的宽度无从判断')

const commandBand = rows[3]!
const rowY = Math.round((commandBand.top + commandBand.bottom) / 2)
const commandRight = rightmostInk(rowY, 500, columnRight + 20)
if (commandRight >= columnRight - 2) {
  throw new Error(`长命令顶到了 x=${commandRight}（内容列右边缘 ${columnRight}）：被硬裁了`)
}
const tail = [...Array(10).keys()]
  .map((i) => (at(commandRight - i, rowY) === '#ffffff' ? '.' : '#'))
  .join('')
console.log(`ok   长命令在 x=${commandRight} 收尾，右边还剩 ${columnRight - commandRight}px；尾部 ${tail}`)

console.log('[rows-check] wrote tmp/rows.png')
