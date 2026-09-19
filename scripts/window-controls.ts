/**
 * Verify the drawn window buttons against the real OS window.
 *
 * The title row is app-drawn because GPUI opens a borderless window on
 * Windows, so "the buttons paint" is not the same as "the window moves". This
 * spawns the app itself (to know its pid), drives the chrome through the
 * automation protocol, and reads the window rectangle back through the same
 * user32 calls the app uses.
 *
 *   bun scripts/window-controls.ts
 *
 * Dragging is covered here too: the app moves the window from the pointer
 * events, so a synthetic drag is a real drag. (A system move loop could not be
 * checked this way, which is one reason the app does not use one.)
 */

import path from 'node:path'
import { controlWindow, readWindowState, type WindowState } from '../src/platform/win32'
import { launchWithPid } from './launch-own'

const root = path.join(import.meta.dir, '..')

if (process.platform !== 'win32') {
  console.log('[window-controls] Windows only, nothing to check')
  process.exit(0)
}

const { app, child } = await launchWithPid(['app.tsx'])

const controls = controlWindow(child.pid!)
let failures = 0

function check(label: string, ok: boolean, detail = ''): void {
  if (!ok) failures += 1
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${label}${detail ? ` — ${detail}` : ''}`)
}

async function waitForWindow(timeoutMs = 60_000) {
  const started = Date.now()
  while (Date.now() - started < timeoutMs) {
    const state = readWindowState(child.pid!)
    if (state) return state
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error('the app never opened a window')
}

await app.getByTestId('welcome').waitFor({ timeoutMs: 60_000 })
const initial = await waitForWindow()
check('window found', Boolean(initial), JSON.stringify(initial))
check('opens as a normal window', !initial.minimized && !initial.maximized)

await app.getByTestId('win-maximize').click()
await new Promise((resolve) => setTimeout(resolve, 400))
const maximized = readWindowState(child.pid!)!
check('maximize fills the work area', maximized.maximized && maximized.width > initial.width)

await app.getByTestId('win-maximize').click()
await new Promise((resolve) => setTimeout(resolve, 400))
const restored = readWindowState(child.pid!)!
check(
  'restore returns the old rectangle',
  !restored.maximized && restored.width === initial.width && restored.x === initial.x,
)

await app.getByTestId('win-minimize').click()
await new Promise((resolve) => setTimeout(resolve, 400))
check('minimize hides the window', readWindowState(child.pid!)!.minimized)

controls.restore()
await new Promise((resolve) => setTimeout(resolve, 400))
check('restore shows it again', !readWindowState(child.pid!)!.minimized)

// The panel is opened first so a failed drag can be diagnosed from the trace it
// writes, without changing anything about the drag itself.
await app.getByTestId('debug').click()
await app.getByTestId('debug-on').click()
await new Promise((resolve) => setTimeout(resolve, 200))

// Drag by the caption strip: the window must follow the pointer, and land
// exactly on the delta, because the move is computed from the pointer events.
//
// The moves are sent one by one on purpose. `mouse.drag({ steps })` interpolates,
// and the last steps of the interpolation get swallowed, so the window stops
// half way — which says nothing about the app, only about the test.
//
// Three drags, with negative deltas too, because a sign error in the offset
// arithmetic would still land on the first one.
async function dragBy(dx: number, dy: number): Promise<{ from: WindowState; to: WindowState }> {
  const before = readWindowState(child.pid!)!
  const surface = await app.getByTestId('titlebar-drag').bounds()
  const from = {
    x: surface.x + Math.round(surface.width / 2),
    y: surface.y + Math.round(surface.height / 2),
  }
  const target = { x: from.x + dx, y: from.y + dy }

  await app.mouse.down(from)
  for (const fraction of [0.25, 0.5, 0.75, 1]) {
    await app.mouse.move(
      {
        x: Math.round(from.x + dx * fraction),
        y: Math.round(from.y + dy * fraction),
      },
      { pressedButton: 0 },
    )
  }
  // The protocol occasionally drops a synthetic move, so the last position is
  // sent again until the window agrees. What is under test is that the window
  // lands on the position of the last move it received, not that the transport
  // is lossless.
  let now = readWindowState(child.pid!)!
  for (let attempt = 0; attempt < 4 && (now.x !== before.x + dx || now.y !== before.y + dy); attempt++) {
    await app.mouse.move(target, { pressedButton: 0 })
    await new Promise((resolve) => setTimeout(resolve, 150))
    now = readWindowState(child.pid!)!
  }
  await app.mouse.up(target)
  await new Promise((resolve) => setTimeout(resolve, 300))
  return { from: before, to: readWindowState(child.pid!)! }
}

for (const [dx, dy] of [
  [120, 80],
  [-60, 40],
  [40, -100],
] as const) {
  const { from, to } = await dragBy(dx, dy)
  check(
    `the strip drags the window by ${dx},${dy}`,
    to.x === from.x + dx && to.y === from.y + dy,
    `${from.x},${from.y} → ${to.x},${to.y}`,
  )
  check(
    `the ${dx},${dy} drag keeps the size`,
    to.width === from.width && to.height === from.height,
  )
}

// A release outside the strip must not leave the window glued to the pointer.
const held = readWindowState(child.pid!)!
await app.mouse.move({ x: held.x + 600, y: held.y + 500 })
await new Promise((resolve) => setTimeout(resolve, 200))
check('the window stays put after the release', readWindowState(child.pid!)!.x === held.x)

await app.getByTestId('win-close').click()
const exited = await Promise.race([
  child.exited.then(() => true),
  new Promise<boolean>((resolve) => setTimeout(() => resolve(false), 5000)),
])
check('close quits the process', exited)
if (!exited) child.kill()

console.log(failures ? `\n[window-controls] ${failures} failed` : '\n[window-controls] all good')
process.exit(failures ? 1 : 0)
