/**
 * Look at the session delete affordance in a live window.
 *
 *   bun scripts/session-delete-check.ts
 *
 * 会话行右边的垃圾桶是删会话的唯一入口，所以要看的是三件事：图标在行尾、第一下
 * 之后变成红色「确认删除」、第二下之后行真的没了。会话行的 testId 里带着运行时
 * 生成的 id，脚本没法预先知道，所以按坐标点：会话行就在「新建会话」下面一行，
 * 垃圾桶在那行的最右边（它右对齐，左边的「确认删除」出现时它不会挪位置）。
 */

import { mkdirSync } from 'node:fs'
import path from 'node:path'
import { launch } from '@gpuix/react/automation'

const root = path.join(import.meta.dir, '..')
mkdirSync(path.join(root, 'tmp'), { recursive: true })

const app = await launch({
  command: 'bun',
  args: ['app.tsx'],
  cwd: root,
  env: { GPUIX_BACKGROUND: '1' },
})

/**
 * 整帧的文字。「新会话」还出现在标题栏和欢迎卡片里，所以只比前后差值。
 * （用 `getAllText`：真实窗口下 `getPaintedText` 返回空数组。）
 */
const rows = async (): Promise<number> =>
  (await app.call('getAllText', {})).text.join('\n').split('新会话').length - 1

await app.getByTestId('welcome').waitFor({ timeoutMs: 60_000 })
await app.getByTestId('new-thread').click()
await new Promise((resolve) => setTimeout(resolve, 300))

const before = await rows()
const anchor = await app.getByTestId('new-thread').bounds()
const trash = { x: anchor.x + anchor.width - 12, y: anchor.y + anchor.height * 1.5 }

await app.call('mouseMove', trash)
await new Promise((resolve) => setTimeout(resolve, 300))
await app.screenshot({ path: path.join(root, 'tmp', 'session-delete-hover.png') })

await app.mouse.click(trash)
await app.getByText('确认删除').waitFor({ timeoutMs: 10_000 })
await new Promise((resolve) => setTimeout(resolve, 300))
await app.screenshot({ path: path.join(root, 'tmp', 'session-delete-armed.png') })
console.log('ok   第一下之后是「确认删除」')

await app.mouse.click(trash)
await new Promise((resolve) => setTimeout(resolve, 500))
const after = await rows()
await app.screenshot({ path: path.join(root, 'tmp', 'session-delete-done.png') })

if (after !== before - 1) {
  throw new Error(`删除之后会话行没有减少：${before} -> ${after}`)
}
console.log(`ok   第二下之后真的删掉了（会话行 ${before} -> ${after}）`)

await app.close()
console.log('[session-delete-check] wrote tmp/session-delete-{hover,armed,done}.png')
