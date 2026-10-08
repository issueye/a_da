/**
 * Design tokens for the a_da agent window.
 *
 * One accent for the agent's warnings, and graphite for the send button. Every
 * value here is a plain string so the whole theme can be swapped from one object.
 *
 * There are two palettes, light and dark, and one of them is *installed* into the
 * exported `C` object. `C` stays a plain mutable object rather than a proxy or a
 * getter bag because that is what every UI module already imports and reads: the
 * window has no memoized components, so a palette swap followed by the store's
 * `notify()` repaints the whole tree with the new values.
 *
 * The exception is the native text themes (`docTheme()`, `editorTheme()`): GPUIX
 * diffs those props by reference identity, so a mutated object would never reach
 * Rust. They are cached one per mode and handed back by identity instead.
 */

import type { GpuixMetrics, GpuixTheme } from '@gpuix/react'

const platform = typeof process !== 'undefined' ? process.platform : 'darwin'

export const IS_WINDOWS = platform === 'win32'
export const IS_MACOS = platform === 'darwin'

export const FONT_SANS = IS_WINDOWS
  ? 'Segoe UI'
  : IS_MACOS
    ? 'Helvetica Neue'
    : 'Sans'

export const FONT_MONO = IS_WINDOWS ? 'Cascadia Mono' : 'Menlo'

export type Appearance = 'light' | 'dark'

export const APPEARANCES: Appearance[] = ['light', 'dark']

/**
 * Every colour the window paints. Both palettes define all of them: the test in
 * `theme.test.ts` fails if one gains a token the other is missing, because a
 * missing key would fall through as `undefined` and paint as transparent.
 */
export interface Palette {
  /** The window and transcript background. */
  canvas: string
  sidebar: string
  sidebarBorder: string
  border: string
  borderStrong: string
  /** Surfaces that sit above the canvas: dialogs, menus, cards' interiors. */
  raised: string

  text: string
  secondary: string
  tertiary: string
  faint: string
  /** Disabled glyphs. */
  ghost: string

  accent: string
  /** The wash behind a warning, sized to sit on the canvas. */
  accentSoft: string
  link: string

  chip: string
  chipHover: string
  chipBorder: string

  card: string
  cardBorder: string
  tab: string

  user: string
  tool: string
  code: string

  /** The filled button colour. It inverts with the palette. */
  inverse: string
  onInverse: string

  /** Interactions layered on an arbitrary surface, so they are alpha-only. */
  overlay: string
  overlayStrong: string

  /** Drop shadows, as colours rather than offsets: the offsets are per site. */
  shadow: string
  shadowStrong: string
  /** The fill behind a modal. Its own token, not `overlay`, because a modal has
   *  to dim the window at any size. */
  scrim: string

  success: string
  danger: string
}

const LIGHT: Palette = {
  canvas: '#FFFFFF',
  sidebar: '#FAFAF9',
  sidebarBorder: '#E9E9E6',
  border: '#E6E6E3',
  borderStrong: '#D9D9D5',
  raised: '#FFFFFF',

  text: '#1E2023',
  secondary: '#5D6169',
  tertiary: '#8B9098',
  faint: '#A9AEB6',
  ghost: '#C6CACF',

  accent: '#C0453C',
  accentSoft: '#FDF2F1',
  link: '#2F6FEB',

  chip: '#F4F4F2',
  chipHover: '#ECECE9',
  chipBorder: '#E4E4E0',

  card: '#F7F7F5',
  cardBorder: '#ECECE8',
  tab: '#F1F1EE',

  user: '#F7F7F5',
  tool: '#FBFBFA',
  code: '#F7F7F5',

  inverse: '#1F1F21',
  onInverse: '#FFFFFF',

  overlay: '#0000000A',
  overlayStrong: '#00000014',

  shadow: '#00000024',
  shadowStrong: '#00000033',
  scrim: '#00000038',

  success: '#2F855A',
  danger: '#C0453C',
}

/**
 * The dark palette.
 *
 * Surfaces climb *up* from the canvas instead of down: `sidebar` is the darkest
 * thing, `raised` the lightest, so a dialog still reads as the frontmost layer.
 * The accent and the link are lifted, because the light ones were chosen to carry
 * on white and go muddy on a dark surface.
 */
const DARK: Palette = {
  canvas: '#1C1D1F',
  sidebar: '#171819',
  sidebarBorder: '#2A2B2D',
  border: '#2C2D30',
  borderStrong: '#3A3C40',
  raised: '#232426',

  text: '#E8E9EA',
  secondary: '#A8ACB3',
  tertiary: '#82868D',
  faint: '#6B6F76',
  ghost: '#4A4D52',

  accent: '#E8735F',
  accentSoft: '#3A2320',
  link: '#6E9BF5',

  chip: '#2A2C2F',
  chipHover: '#323438',
  chipBorder: '#343639',

  card: '#202123',
  cardBorder: '#2E3033',
  tab: '#2A2C2F',

  user: '#26282B',
  tool: '#1F2022',
  code: '#232426',

  inverse: '#E8E9EA',
  onInverse: '#1C1D1F',

  overlay: '#FFFFFF0A',
  overlayStrong: '#FFFFFF14',

  shadow: '#00000059',
  shadowStrong: '#00000080',
  scrim: '#00000059',

  success: '#4EC28A',
  danger: '#E8735F',
}

const PALETTES: Record<Appearance, Palette> = { light: LIGHT, dark: DARK }

let current: Appearance = 'light'

/** The installed mode. Read by `docTheme()` and by anything laying out native text. */
export function appearance(): Appearance {
  return current
}

/**
 * Install a palette. Deliberately does not notify anything: the caller (the
 * store) applies this *before* its own `notify()`, so the re-render already sees
 * the new values.
 */
export function applyAppearance(next: Appearance): void {
  current = next
  Object.assign(C, PALETTES[next])
}

/** The installed palette, for the rare caller that wants to read several keys. */
export function palette(): Palette {
  return PALETTES[current]
}

/**
 * Colours the whole window is built from. Mutated in place by
 * `applyAppearance()`, so a module that reads `C.text` inside a render always
 * gets the installed mode — but a module that copies a value out at import time
 * gets whatever was installed then. That second trap is why the places which used
 * to hold a colour in a module-level object are now functions: `menuCard()` in
 * `ui/controls.tsx` and `statusOf()` in `ui/Transcript.tsx`.
 */
export const C: Palette = { ...LIGHT }

/** Row heights, paddings and text sizes the whole window is built from. */
export const M = {
  /** Narrow caption strip: the window buttons are the tallest thing in it. */
  titleBar: 36,
  barButton: 24,
  tab: 24,
  /** The caption pill in the title bar, not the tab strip height. */
  tabStrip: 34,
  windowButton: 36,
  sidebar: 232,
  row: 26,
  iconButton: 26,
  composerMax: 780,
  transcriptMax: 760,
  contentPadding: 28,
  radius: 10,
  settingsWidth: 660,
  settingsNav: 168,
  settingsHeader: 46,
  settingsMinHeight: 320,
} as const

/** Layout of the native text components, shared by both modes. */
const METRICS: GpuixMetrics = {
  mdTextSize: 13.5,
  mdLineHeight: 21,
  mdBlockGap: 12,
  mdHeadingSizes: [18, 15.5, 13.5, 13.5],
  mdHeadingLineHeights: [26, 22, 21, 21],
  codeTextSize: 12,
  codeLineHeight: 18,
  diffLineHeight: 18,
  diffFileHeaderHeight: 30,
}

type NativeTheme = GpuixTheme & { appearance: Appearance; metrics: GpuixMetrics }

function nativeTheme(mode: Appearance): NativeTheme {
  const p = PALETTES[mode]
  return {
    // This one field is what switches Rust's built-in syntax palette; the tokens
    // below then override the surfaces to match this window.
    appearance: mode,
    text: p.text,
    textMuted: p.secondary,
    textFaint: p.tertiary,
    textDim: p.secondary,
    border: p.border,
    bg: p.canvas,
    accent: p.link,
    caret: p.link,
    fontSans: FONT_SANS,
    fontMono: FONT_MONO,
    // Inline code is tinted rather than filled on light, and the reverse on dark.
    codeText: mode === 'dark' ? '#E5A08F' : '#9A3E2E',
    codeWash: mode === 'dark' ? '#FFFFFF0D' : '#0000000D',
    metrics: METRICS,
  }
}

/**
 * Cached one per mode, and that cache is load-bearing rather than an
 * optimisation: GPUIX decides whether to resend a custom prop by identity
 * (`oldValue !== value`), so a fresh object on every render would push the theme
 * to Rust on every frame, and mutating one object in place would never push it at
 * all. Same mode, same object; new mode, new object.
 */
const DOC_THEMES: Record<Appearance, NativeTheme> = {
  light: nativeTheme('light'),
  dark: nativeTheme('dark'),
}

/** Theme for the native `<markdown>` / `<diff>` elements. */
export function docTheme(): NativeTheme {
  return DOC_THEMES[current]
}

/**
 * Theme for the native `<input>` / `<textarea>` elements.
 *
 * The same object as `docTheme()`: the editor caret the old constant overrode by
 * hand was the link colour the doc theme already carried, so the override only
 * looked like a distinction. The name is kept because it says which elements this
 * is for at the call sites.
 */
export function editorTheme(): NativeTheme {
  return DOC_THEMES[current]
}

/** `C:\Users\me\code\a_da\dist` becomes `_a_da/dist`, like the sidebar in the mock. */
export function shortPath(path: string, segments = 2): string {
  const parts = path.split(/[\\/]/).filter(Boolean)
  if (parts.length <= segments) return parts.join('/')
  return `_${parts.slice(-segments).join('/')}`
}
