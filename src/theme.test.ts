/**
 * 明暗两套调色板。
 *
 * The window has no pixel assertions in the renderer tests, so the tests that
 * actually matter here are structural: the two palettes must define the same
 * tokens (a missing one would paint as transparent), the native text themes must
 * swap by identity (GPUIX diffs those props by `!==`, so a mutated object would
 * never reach Rust), and a switch must be visible in `C`.
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { appearance, applyAppearance, C, docTheme, editorTheme, palette } from './theme'

afterEach(() => applyAppearance('light'))

/** Every token either palette defines, as hex or 8-digit alpha hex. */
const HEX = /^#[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?$/

/** Perceived brightness of a `#RRGGBB` token, 0-255. Enough to tell "dark" apart. */
function luminance(color: string): number {
  const red = parseInt(color.slice(1, 3), 16)
  const green = parseInt(color.slice(3, 5), 16)
  const blue = parseInt(color.slice(5, 7), 16)
  return 0.299 * red + 0.587 * green + 0.114 * blue
}

describe('palettes', () => {
  test('both modes define the same tokens', () => {
    const light = Object.keys(palette()).sort()
    applyAppearance('dark')
    expect(Object.keys(palette()).sort()).toEqual(light)
  })

  test('every token is a colour, so none of them paints as undefined', () => {
    for (const mode of ['light', 'dark'] as const) {
      applyAppearance(mode)
      for (const [key, value] of Object.entries(palette())) {
        expect(value, `${mode}.${key}`).toMatch(HEX)
      }
    }
  })

  test('the two modes actually differ where it matters', () => {
    applyAppearance('light')
    const light = { ...palette() }
    applyAppearance('dark')
    const dark = palette()

    // The chrome is inverted, not merely adjusted.
    expect(dark.canvas).not.toBe(light.canvas)
    expect(dark.text).not.toBe(light.text)
    expect(luminance(light.canvas)).toBeGreaterThan(luminance(light.text))
    expect(luminance(dark.canvas)).toBeLessThan(luminance(dark.text))

    // `inverse` is the filled-button colour, so it has to cross the surface it
    // sits on: a dark chip on a light window, a light chip on a dark one.
    expect(luminance(light.inverse)).toBeLessThan(luminance(light.canvas))
    expect(luminance(dark.inverse)).toBeGreaterThan(luminance(dark.canvas))
    expect(luminance(dark.onInverse)).toBeLessThan(luminance(dark.inverse))
  })
})

describe('applying a palette', () => {
  test('writes through to C, which is what the UI reads', () => {
    applyAppearance('light')
    const lightCanvas = C.canvas
    applyAppearance('dark')
    expect(C.canvas).not.toBe(lightCanvas)
    expect(C.canvas).toBe(palette().canvas)
    expect(appearance()).toBe('dark')
  })

  test('reinstalling the same mode is idempotent', () => {
    applyAppearance('dark')
    const first = { ...C }
    applyAppearance('dark')
    expect({ ...C }).toEqual(first)
  })
})

describe('native text themes', () => {
  test('each mode gets its own object, so the identity diff fires', () => {
    applyAppearance('light')
    const light = docTheme()
    applyAppearance('dark')
    const dark = docTheme()
    expect(dark).not.toBe(light)
    expect(dark.appearance).toBe('dark')
    expect(dark.bg).not.toBe(light.bg)
  })

  test('the same mode hands back the same object, so no frame resends it', () => {
    applyAppearance('dark')
    expect(docTheme()).toBe(docTheme())
    expect(editorTheme()).toBe(editorTheme())
  })

  test('the editor theme follows the mode like the doc theme', () => {
    applyAppearance('light')
    const light = editorTheme()
    applyAppearance('dark')
    expect(editorTheme()).not.toBe(light)
    expect(editorTheme().appearance).toBe('dark')
    expect(editorTheme().caret).toBe(docTheme().caret)
  })

  test('the native type scale stays in both modes', () => {
    for (const mode of ['light', 'dark'] as const) {
      applyAppearance(mode)
      expect(docTheme().metrics?.mdHeadingSizes).toHaveLength(4)
      expect(editorTheme().metrics?.mdLineHeight).toBe(21)
    }
  })
})
