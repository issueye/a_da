/**
 * Look at the popup menus, which are the only rounded surfaces GPUIX draws over
 * a floating layer.
 *
 *   bun scripts/menu-check.ts
 *
 * The corners are the point: the `<anchored>` layer paints an opaque `#1A1A1A`
 * behind its children, so a rounded card there shows it as black corners.
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

await app.getByTestId('welcome').waitFor({ timeoutMs: 60_000 })

await app.getByTestId('approval').click()
await new Promise((resolve) => setTimeout(resolve, 300))
await app.screenshot({ path: path.join(root, 'tmp', 'menu-approval.png') })

// A different chip, so the first menu is closed before the second opens.
await app.getByTestId('composer-add').click()
await new Promise((resolve) => setTimeout(resolve, 300))
await app.screenshot({ path: path.join(root, 'tmp', 'menu-add.png') })

await app.close()
console.log('[menu-check] wrote tmp/menu-approval.png and tmp/menu-add.png')
