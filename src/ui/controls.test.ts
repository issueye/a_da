/**
 * The one thing about a menu that no other test can see.
 *
 * GPUIX's `<anchored>` paints an opaque `#1A1A1A` behind its children, and the
 * floating layer's div sits on top of it. If the rounded card *is* that div, the
 * cut corners show that dark fill and the menu grows black corners — which the
 * GPU renderer tests cannot catch, because they assert text and bounds, not
 * pixels. So the shape is pinned here: a square, opaque layer, with the radius
 * on a card inside it.
 */

import { describe, expect, test } from 'bun:test'
import { MENU_CARD, MENU_LAYER } from './controls'

describe('menu surface', () => {
  test('the floating layer is an opaque square that hides the anchored fill', () => {
    expect(MENU_LAYER.backgroundColor).toBe(MENU_CARD.backgroundColor)
    expect(MENU_LAYER.borderRadius).toBeUndefined()
  })

  test('the card inside it carries the radius, border and shadow', () => {
    expect(MENU_CARD.borderRadius).toBeGreaterThan(0)
    expect(MENU_CARD.borderWidth).toBeGreaterThan(0)
    expect(MENU_CARD.boxShadow).toBeDefined()
  })
})
