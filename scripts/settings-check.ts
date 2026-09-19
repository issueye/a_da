/**
 * Look at the settings dialog and the caption strip in a real window.
 *
 *   bun scripts/settings-check.ts
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

async function step(label: string, action: () => Promise<unknown>): Promise<void> {
  try {
    await action()
    console.log(`ok   ${label}`)
  } catch (error) {
    console.log(`FAIL ${label} — ${(error as Error).message.split('\n')[0]}`)
    process.exitCode = 1
  }
}

const ids = ['open-settings', 'settings-dialog', 'settings-nav-workspace', 'settings-close']
async function report(): Promise<void> {
  const counts = await Promise.all(ids.map(async (id) => `${id}=${await app.getByTestId(id).count()}`))
  console.log(`     ${counts.join(' ')}`)
}

await step('window is up', () => app.getByTestId('welcome').waitFor({ timeoutMs: 60_000 }))
await step('caption screenshot', () => app.screenshot({ path: path.join(root, 'tmp', 'bar.png') }))
await report()

await step('open settings', () => app.getByTestId('open-settings').click())
await report()
await step('dialog screenshot', () => app.screenshot({ path: path.join(root, 'tmp', 'settings.png') }))
await report()

await step('switch to the workspace section', () => app.getByTestId('settings-nav-workspace').click())
await step('workspace screenshot', () =>
  app.screenshot({ path: path.join(root, 'tmp', 'settings-workspace.png') }),
)
await report()

await step('close the dialog with Escape', async () => {
  // Back to 供应商, because the field only exists in that section.
  await app.getByTestId('settings-nav-provider').click()
  // `fill` focuses the field, so the key goes to the dialog's subtree. Note
  // that `locator.click()` on an `<input>` in a live window finds no painted
  // box, which is why this drives the field by keyboard instead.
  await app.getByTestId('settings-base-url').fill('https://api.openai.com/v1')
  await app.getByTestId('settings-base-url').press('escape')
  await new Promise((resolve) => setTimeout(resolve, 200))
  if ((await app.getByTestId('settings-dialog').count()) !== 0) {
    throw new Error('the dialog stayed open')
  }
})

await app.close()
console.log('[settings-check] wrote tmp/bar.png, tmp/settings.png, tmp/settings-workspace.png')
