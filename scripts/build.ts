/**
 * Ship a single executable.
 *
 * `bun build --compile` embeds the JavaScript, the React reconciler and the
 * `@gpuix/native` addon, so the result runs on a machine with no Bun and no
 * Node install. On Windows `hideConsole` matters: without it the app opens with
 * a console window behind it.
 *
 *   bun run build              writes dist/a-da (dist/a-da.exe on Windows)
 */

import { existsSync, mkdirSync } from 'node:fs'
import path from 'node:path'

const root = path.join(import.meta.dir, '..')
const outfile = process.env.A_DA_OUT ?? path.join(root, 'dist', 'a-da')
// `bun scripts/make-icon.tsx` writes this from assets/logo.svg.
const icon = path.join(root, 'assets', 'logo.ico')

mkdirSync(path.dirname(outfile), { recursive: true })

async function build() {
  return Bun.build({
    entrypoints: [path.join(root, 'app.tsx')],
    minify: true,
    compile: {
      outfile,
      windows:
        process.platform === 'win32'
          ? {
              hideConsole: true,
              title: 'a_da',
              publisher: 'a_da',
              version: '0.1.0',
              description: 'A local coding agent rendered with GPUIX',
              ...(existsSync(icon) ? { icon } : {}),
            }
          : undefined,
    },
  })
}

// Windows keeps a just-exited executable locked for a moment, which makes Bun
// fail with EPERM while it moves the new binary into place.
let result = await build()
for (let attempt = 0; attempt < 3 && !result.success; attempt++) {
  const locked = result.logs.some((message) => message.message.includes('EPERM'))
  if (!locked) break
  await new Promise((resolve) => setTimeout(resolve, 500))
  result = await build()
}

if (!result.success) {
  for (const message of result.logs) console.error(message)
  process.exit(1)
}

console.log(`[build] wrote ${path.relative(root, result.outputs[0]?.path ?? outfile)}`)
