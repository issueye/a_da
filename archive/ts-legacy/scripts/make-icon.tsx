/**
 * Rasterise the logo into the icon formats a desktop app needs.
 *
 *   bun scripts/make-icon.tsx
 *
 * There is no image library in this project, but there is a GPU: `icon-window.tsx`
 * draws the logo in a real 256x256 window and this takes its screenshot. Windows
 * accepts a PNG payload inside an `.ico`, so the icon container is 22 bytes of
 * header around those bytes and no BMP encoder is needed. `scripts/png-pixels.ts`
 * is what checks the result — the corners must be transparent, or the icon shows
 * white squares wherever the desktop is dark.
 */

import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { launch } from '@gpuix/react/automation'
import { pixelAt, readPng } from './png-pixels'

const root = path.join(import.meta.dir, '..')
const png = path.join(root, 'assets', 'logo.png')
const ico = path.join(root, 'assets', 'logo.ico')

const app = await launch({
  command: 'bun',
  args: ['scripts/icon-window.tsx'],
  cwd: root,
  env: { GPUIX_BACKGROUND: '1' },
})
await app.getByTestId('icon').waitFor({ timeoutMs: 30_000 })
await new Promise((resolve) => setTimeout(resolve, 600))
await app.screenshot({ path: png })
await app.close()

const image = readPng(png)
console.log(`[make-icon] assets/logo.png ${image.width}x${image.height}`)
if (image.width !== 256 || image.height !== 256) {
  throw new Error(`the icon window should be 256x256, got ${image.width}x${image.height}`)
}
const corner = pixelAt(image, 2, 2)
const middle = pixelAt(image, 128, 128)
console.log(`[make-icon] corner rgba(${corner.join(', ')}) centre rgba(${middle.join(', ')})`)
if (corner[3] !== 0) {
  throw new Error('the icon corners are not transparent; the desktop would show through as white')
}

const bytes = readFileSync(png)
// ICONDIR (6) + one ICONDIRENTRY (16). Zero width and height mean 256.
const header = Buffer.alloc(22)
header.writeUInt16LE(0, 0) // reserved
header.writeUInt16LE(1, 2) // 1 = icon
header.writeUInt16LE(1, 4) // one image
header.writeUInt8(0, 6) // width, 0 = 256
header.writeUInt8(0, 7) // height, 0 = 256
header.writeUInt8(0, 8) // palette colours
header.writeUInt8(0, 9) // reserved
header.writeUInt16LE(1, 10) // colour planes
header.writeUInt16LE(32, 12) // bits per pixel
header.writeUInt32LE(bytes.length, 14)
header.writeUInt32LE(22, 18)
writeFileSync(ico, Buffer.concat([header, bytes]))
console.log(`[make-icon] wrote assets/logo.ico (${22 + bytes.length} bytes)`)
if (!existsSync(ico)) throw new Error('the icon was not written')
