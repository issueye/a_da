/**
 * The icon rasteriser's window: just the logo, at exactly the icon size.
 *
 * A window of its own rather than the test renderer, because the offscreen test
 * window is a fixed 1536x1061 and would put the logo in a corner of a large
 * transparent frame. A real window's client area is exactly what `render` asked
 * for, and its screenshot is exactly that many pixels.
 *
 *   bun scripts/icon-window.tsx
 *
 * `windowBackground: "transparent"` is what keeps the rounded corners of the
 * tile transparent instead of white, so the icon sits on the desktop properly.
 */

import React from 'react'
import { render } from '@gpuix/react'
import logo from '../assets/logo.svg' with { type: 'text' }

const SIZE = 256

render(
  <div testId="icon" style={{ display: 'flex', width: SIZE, height: SIZE }}>
    <img
      src={`data:image/svg+xml;base64,${Buffer.from(logo).toString('base64')}`}
      style={{ width: SIZE, height: SIZE }}
    />
  </div>,
  {
    title: 'a_da icon',
    width: SIZE,
    height: SIZE,
    windowBackground: 'transparent',
    focus: false,
  },
)
