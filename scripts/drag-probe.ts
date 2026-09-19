/**
 * What does the caption strip do when it is pressed?
 *
 * App-drawn chrome fails invisibly: the press may never reach the handler,
 * `user32` may be unreachable, the window handle may not resolve, or the move
 * may be computed from stale coordinates. This drags the surface through the
 * real input pipeline and screenshots the trace the handler writes, which
 * includes the window position it asked for on every move.
 *
 *   bun scripts/drag-probe.ts
 *
 * `scripts/window-controls.ts` is the pass/fail check; this one is for reading.
 */

import path from 'node:path'
import { controlWindow, readWindowState } from '../src/platform/win32'
import { launchWithPid } from './launch-own'

const root = path.join(import.meta.dir, '..')

if (process.platform !== 'win32') {
  console.log('[drag-probe] Windows only')
  process.exit(0)
}

const { app, child } = await launchWithPid(['app.tsx'])

await app.getByTestId('welcome').waitFor({ timeoutMs: 60_000 })
await app.getByTestId('debug').click()
await app.getByTestId('debug-on').click()
await new Promise((resolve) => setTimeout(resolve, 200))

// The same warm-up window-controls.ts does, since that is where the drag starts
// failing: maximize, restore, minimize, restore.
await app.getByTestId('win-maximize').click()
await new Promise((resolve) => setTimeout(resolve, 400))
await app.getByTestId('win-maximize').click()
await new Promise((resolve) => setTimeout(resolve, 400))
await app.getByTestId('win-minimize').click()
await new Promise((resolve) => setTimeout(resolve, 400))
controlWindow(child.pid!).restore()
await new Promise((resolve) => setTimeout(resolve, 400))

console.log('window before:', JSON.stringify(readWindowState(child.pid!)))
const surface = await app.getByTestId('titlebar-drag').bounds()
console.log('drag surface:', JSON.stringify(surface))

const from = {
  x: surface.x + Math.round(surface.width / 2),
  y: surface.y + Math.round(surface.height / 2),
}
const target = { x: from.x + 120, y: from.y + 80 }
await app.mouse.down(from)
for (const fraction of [0.25, 0.5, 0.75, 1]) {
  await app.mouse.move(
    { x: Math.round(from.x + 120 * fraction), y: Math.round(from.y + 80 * fraction) },
    { pressedButton: 0 },
  )
}
for (let attempt = 0; attempt < 3; attempt++) {
  await app.mouse.move(target, { pressedButton: 0 })
  await new Promise((resolve) => setTimeout(resolve, 150))
}
await app.mouse.up(target)
await new Promise((resolve) => setTimeout(resolve, 400))
console.log('window after: ', JSON.stringify(readWindowState(child.pid!)))

await app.screenshot({ path: path.join(root, 'tmp', 'drag.png') })
await app.close()
console.log('[drag-probe] wrote tmp/drag.png — read the trace lines there')
