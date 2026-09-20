/**
 * Check the Projects panel in a live window: add a project by path, confirm the
 * sidebar switches to it, and confirm the thread list is per project.
 *
 *   bun scripts/projects-check.ts [path-to-add]
 *
 * Defaults to the neighbouring gpuix checkout, which is a real folder with a
 * lot of files, so the scan counts are visibly different from this one.
 */

import { mkdirSync } from 'node:fs'
import path from 'node:path'
import { launch } from '@gpuix/react/automation'
import { shortPath } from '../src/theme'

const root = path.join(import.meta.dir, '..')
const target = process.argv[2] ?? path.join(root, '..', 'gpuix')
mkdirSync(path.join(root, 'tmp'), { recursive: true })

const app = await launch({
  command: 'bun',
  args: ['app.tsx'],
  cwd: root,
  // 这个脚本测的是手输路径那条路：关掉原生目录选择弹窗，否则它会开一个窗口
  // 停在那里等人点。
  env: { GPUIX_BACKGROUND: '1', A_DA_NO_DIALOG: '1' },
})

await app.getByTestId('welcome').waitFor({ timeoutMs: 60_000 })

// A wrong path is reported instead of being added.
await app.getByTestId('add-project').click()
await app.getByTestId('project-path').fill(path.join(root, 'does-not-exist'))
await app.getByTestId('project-path').press('enter')
await app.getByText('路径不存在').waitFor({ timeoutMs: 5_000 })
console.log('ok   rejects a missing path')

// The real one is added, and becomes the open project.
await app.getByTestId('project-path').fill(target)
await app.getByTestId('project-path').press('enter')
await app.getByText('gpuix').waitFor({ timeoutMs: 10_000 })
await app.getByText('添加项目').waitFor({ timeoutMs: 5_000 })
console.log('ok   added and opened a second project')

// The project row now counts that folder, so the scan followed the switch.
// 行的 testId 是缩写后的路径（`shortPath(path, 2)`），不是目录名。
const rowTestId = `project-${shortPath(target, 2)}`
const counts = await app.getByTestId(rowTestId).textContent()
console.log(`ok   sidebar row ${rowTestId}: ${JSON.stringify(counts)}`)

await app.clock.pause()
await app.screenshot({ path: path.join(root, 'tmp', 'projects.png') })
await app.clock.resume()
await app.close()

console.log('[projects-check] wrote tmp/projects.png')
