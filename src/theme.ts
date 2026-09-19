/**
 * Design tokens for the a_da agent window.
 *
 * Light surface, one accent for the agent's warnings, and graphite for the
 * send button. Every value here is a plain string so the whole theme can be
 * swapped from one object.
 */

const platform = typeof process !== 'undefined' ? process.platform : 'darwin'

export const IS_WINDOWS = platform === 'win32'
export const IS_MACOS = platform === 'darwin'

export const FONT_SANS = IS_WINDOWS
  ? 'Segoe UI'
  : IS_MACOS
    ? 'Helvetica Neue'
    : 'Sans'

export const FONT_MONO = IS_WINDOWS ? 'Cascadia Mono' : 'Menlo'

export const C = {
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

  success: '#2F855A',
  danger: '#C0453C',
} as const

/** Row heights, paddings and text sizes the whole window is built from. */
export const M = {
  /** Narrow caption strip: the window buttons are the tallest thing in it. */
  titleBar: 36,
  barButton: 24,
  tab: 24,
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

/** Theme for the native `<markdown>` / `<code>` elements. */
export const DOC_THEME = {
  appearance: 'light' as const,
  text: C.text,
  textMuted: C.secondary,
  textFaint: C.tertiary,
  textDim: C.secondary,
  border: C.border,
  bg: C.canvas,
  accent: C.link,
  caret: C.link,
  fontSans: FONT_SANS,
  codeText: '#9A3E2E',
  codeWash: '#0000000D',
  metrics: {
    mdTextSize: 13.5,
    mdLineHeight: 21,
    mdBlockGap: 12,
    mdHeadingSizes: [18, 15.5, 13.5, 13.5],
    mdHeadingLineHeights: [26, 22, 21, 21],
    codeTextSize: 12,
    codeLineHeight: 18,
    diffLineHeight: 18,
    diffFileHeaderHeight: 30,
  },
}

export const EDITOR_THEME = {
  ...DOC_THEME,
  caret: '#2F6FEB',
}

/** `C:\Users\me\code\a_da\dist` becomes `_a_da/dist`, like the sidebar in the mock. */
export function shortPath(path: string, segments = 2): string {
  const parts = path.split(/[\\/]/).filter(Boolean)
  if (parts.length <= segments) return parts.join('/')
  return `_${parts.slice(-segments).join('/')}`
}
