/**
 * The one thing about a menu that no other test can see.
 *
 * GPUIX's `<anchored>` paints an opaque `#1A1A1A` behind its children, and the
 * floating layer's div sits on top of it. If the rounded card *is* that div, the
 * cut corners show that dark fill and the menu grows black corners — which the
 * GPU renderer tests cannot catch, because they assert text and bounds, not
 * pixels. So the shape is pinned here: a square, opaque layer, with the radius
 * on a card inside it — and both follow the installed palette, so a theme switch
 * does not leave the menu on the old mode's colours.
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { menuCard, menuLayer } from './controls'
import { applyAppearance } from '../theme'

afterEach(() => applyAppearance('light'))

describe('menu surface', () => {
  test('the floating layer is an opaque square that hides the anchored fill', () => {
    expect(menuLayer().backgroundColor).toBe(menuCard().backgroundColor)
    expect(menuLayer().borderRadius).toBeUndefined()
  })

  test('the card inside it carries the radius, border and shadow', () => {
    const card = menuCard()
    expect(card.borderRadius).toBeGreaterThan(0)
    expect(card.borderWidth).toBeGreaterThan(0)
    expect(card.boxShadow).toBeDefined()
  })

  test('both boxes repaint when the appearance changes', () => {
    const light = menuCard().backgroundColor
    applyAppearance('dark')
    expect(menuCard().backgroundColor).not.toBe(light)
    expect(menuLayer().backgroundColor).toBe(menuCard().backgroundColor)
  })
})
