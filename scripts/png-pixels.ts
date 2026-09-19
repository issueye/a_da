/**
 * Read a PNG the app wrote, without an image library.
 *
 * `captureScreenshot` always writes 8-bit RGBA, not interlaced, filter method 0,
 * so a decoder is: parse IHDR, inflate the IDAT stream, undo the per-row filters.
 * That is small enough to keep here, and it is the only way to assert anything
 * about *pixels* — which is exactly what "the menu has black corners" and "the
 * icon has transparent corners" are.
 *
 *   bun scripts/png-pixels.ts assets/logo.png [x] [y]
 */

import { inflateSync } from 'node:zlib'
import { readFileSync } from 'node:fs'

export interface Png {
  width: number
  height: number
  /** Row-major RGBA, four bytes per pixel. */
  pixels: Buffer
}

export function readPng(path: string): Png {
  const file = readFileSync(path)
  if (file.readUInt32BE(0) !== 0x89504e47) throw new Error('not a PNG')
  const width = file.readUInt32BE(16)
  const height = file.readUInt32BE(20)
  if (file[24] !== 8 || file[25] !== 6) {
    throw new Error(`expected 8-bit RGBA, got bit depth ${file[24]} type ${file[25]}`)
  }
  if (file[28] !== 0) throw new Error('interlaced PNGs are not supported')

  const chunks: Buffer[] = []
  let offset = 8
  while (offset < file.length) {
    const length = file.readUInt32BE(offset)
    const type = file.toString('ascii', offset + 4, offset + 8)
    if (type === 'IDAT') chunks.push(file.subarray(offset + 8, offset + 8 + length))
    offset += length + 12
  }
  const raw = inflateSync(Buffer.concat(chunks))

  const stride = width * 4
  const pixels = Buffer.alloc(stride * height)
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)]!
    const row = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1))
    const out = pixels.subarray(y * stride, (y + 1) * stride)
    const up = pixels.subarray((y - 1) * stride, y * stride)
    for (let x = 0; x < stride; x++) {
      const left = x >= 4 ? out[x - 4]! : 0
      const value = row[x]!
      switch (filter) {
        case 0:
          out[x] = value
          break
        case 1:
          out[x] = (value + left) & 0xff
          break
        case 2:
          out[x] = (value + (y > 0 ? up[x]! : 0)) & 0xff
          break
        case 3:
          out[x] = (value + ((left + (y > 0 ? up[x]! : 0)) >> 1)) & 0xff
          break
        case 4: {
          const a = left
          const b = y > 0 ? up[x]! : 0
          const c = y > 0 && x >= 4 ? up[x - 4]! : 0
          const p = a + b - c
          const pa = Math.abs(p - a)
          const pb = Math.abs(p - b)
          const pc = Math.abs(p - c)
          out[x] = (value + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c)) & 0xff
          break
        }
        default:
          throw new Error(`unknown filter ${filter}`)
      }
    }
  }
  return { width, height, pixels }
}

export function pixelAt(png: Png, x: number, y: number): [number, number, number, number] {
  const offset = (y * png.width + x) * 4
  return [png.pixels[offset]!, png.pixels[offset + 1]!, png.pixels[offset + 2]!, png.pixels[offset + 3]!]
}

if (import.meta.main) {
  const [file, x = '0', y = '0'] = process.argv.slice(2)
  if (!file) throw new Error('usage: bun scripts/png-pixels.ts <file.png> [x] [y]')
  const png = readPng(file)
  console.log(`${file}: ${png.width}x${png.height}`)
  console.log(`pixel ${x},${y} = rgba(${pixelAt(png, Number(x), Number(y)).join(', ')})`)
}
