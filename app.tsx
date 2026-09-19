/**
 * a_da — a local coding agent with a native GPU window.
 *
 * Run it with `bun --hot app.tsx`, or ship it with `bun run build`. The agent
 * works inside the window's project directory and asks before it writes; see
 * README.md for the model configuration.
 */

import React from 'react'
import { render } from '@gpuix/react'
import { AgentWindow } from './src/AgentWindow'

render(<AgentWindow />, {
  title: 'a_da',
  width: 1120,
  height: 760,
  titlebarTransparent: true,
  trafficLightX: 16,
  trafficLightY: 17,
  // An agent runs this to look at its own UI; never take the user's keyboard.
  focus: process.env.GPUIX_BACKGROUND !== '1',
})
