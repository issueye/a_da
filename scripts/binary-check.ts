/**
 * Prove the compiled binary is a real app: start `dist/a-da.exe`, wait for the
 * first frame, screenshot it, and close it.
 *
 *   bun run build && bun scripts/binary-check.ts
 */

import { mkdirSync } from 'node:fs'
import path from 'node:path'
import { launch } from '@gpuix/react/automation'

const root = path.join(import.meta.dir, '..')
const binary = path.join(root, 'dist', process.platform === 'win32' ? 'a-da.exe' : 'a-da')
mkdirSync(path.join(root, 'tmp'), { recursive: true })

const app = await launch({
  command: binary,
  args: [],
  cwd: root,
  env: { GPUIX_BACKGROUND: '1' },
})
await app.getByTestId('welcome').waitFor({ timeoutMs: 60_000 })
await app.clock.pause()
await app.screenshot({ path: path.join(root, 'tmp', 'binary.png') })
await app.clock.resume()
await app.close()

console.log(`[binary-check] ${path.relative(root, binary)} launched and painted`)
