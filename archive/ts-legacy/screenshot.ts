/**
 * Drive the agent window and write a PNG, so a change can be checked without
 * looking at the screen.
 *
 *   bun run screenshot              writes tmp/app.png
 *   bun run screenshot out.png      writes that path instead
 *
 * `GPUIX_BACKGROUND=1` keeps the window from stealing focus while it runs.
 */

import { mkdirSync } from 'node:fs'
import path from 'node:path'
import { launch } from '@gpuix/react/automation'

const out = process.argv[2] ?? 'tmp/app.png'
mkdirSync(path.dirname(out), { recursive: true })

const app = await launch({
  command: 'bun',
  args: ['app.tsx'],
  env: { GPUIX_BACKGROUND: '1' },
})
await app.getByTestId('welcome').waitFor({ timeoutMs: 60_000 })
await app.clock.pause()
await app.screenshot({ path: out })
await app.close()

console.log(`[screenshot] wrote ${out}`)
