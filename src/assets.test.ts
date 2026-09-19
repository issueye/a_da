/**
 * The logo, as the desktop will see it.
 *
 * The icon is generated (`bun scripts/make-icon.tsx`) from `assets/logo.svg`, so
 * these check the things a bad generation would get wrong and nothing else would
 * notice: a corner that is not transparent shows as a white square on a dark
 * taskbar, and an `.ico` with a broken directory makes Windows fall back to the
 * default icon without saying anything.
 */

import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { pixelAt, readPng } from '../scripts/png-pixels'

const root = path.join(import.meta.dir, '..')
const png = path.join(root, 'assets', 'logo.png')
const ico = path.join(root, 'assets', 'logo.ico')
const svg = readFileSync(path.join(root, 'assets', 'logo.svg'), 'utf8')

describe('logo', () => {
  test('the icon is a 256x256 square with transparent corners', () => {
    const image = readPng(png)
    expect(image.width).toBe(256)
    expect(image.height).toBe(256)
    for (const [x, y] of [
      [1, 1],
      [254, 1],
      [1, 254],
      [254, 254],
    ]) {
      expect(pixelAt(image, x!, y!)[3], `corner ${x},${y} alpha`).toBe(0)
    }
  })

  test('the mark is painted on a dark tile', () => {
    const image = readPng(png)
    // The chevron's tip sits on the centre line, the tile around it is ink.
    expect(pixelAt(image, 128, 128)).toEqual([255, 255, 255, 255])
    const tile = pixelAt(image, 128, 24)
    expect(tile[3]).toBe(255)
    expect(tile[0]).toBeLessThan(60)
  })

  test('the ico is one 256px image whose payload is that png', () => {
    const file = readFileSync(ico)
    expect(file.readUInt16LE(0)).toBe(0) // reserved
    expect(file.readUInt16LE(2)).toBe(1) // 1 = icon
    expect(file.readUInt16LE(4)).toBe(1) // one image
    expect(file.readUInt8(6)).toBe(0) // 0 = 256 wide
    expect(file.readUInt8(7)).toBe(0) // 0 = 256 tall
    expect(file.readUInt16LE(10)).toBe(1) // colour planes
    expect(file.readUInt16LE(12)).toBe(32) // bits per pixel
    expect(file.readUInt32LE(18)).toBe(22) // payload offset

    const payload = file.subarray(22)
    expect(file.readUInt32LE(14)).toBe(payload.length)
    expect(payload.subarray(0, 8)).toEqual(readFileSync(png).subarray(0, 8))
  })

  test('the source svg is the shell prompt the icon shows', () => {
    expect(svg).toContain('stroke="#FFFFFF"')
    expect(svg).toContain('fill="#E8624F"')
    expect(svg).toContain('viewBox="0 0 1024 1024"')
  })
})
