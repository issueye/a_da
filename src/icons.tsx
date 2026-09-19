/**
 * The window's icon set.
 *
 * Every icon is a raw SVG string, so `bun build --compile` embeds it in the
 * binary and the app ships no loose asset files. The paths use `#000`, which
 * GPUI's monochrome icon renderer replaces with the element's `style.color`.
 */

const wrap = (body: string, size = 24) =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${size} ${size}" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`

export const ICONS = {
  sidebar: wrap('<rect width="18" height="18" x="3" y="3" rx="3"/><path d="M9 3v18"/>'),
  search: wrap('<circle cx="11" cy="11" r="7"/><path d="m20 20-3.6-3.6"/>'),
  thread: wrap(
    '<path d="M21 15a2 2 0 0 1-2 2H8l-5 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>',
  ),
  minus: wrap('<path d="M5 12h14"/>'),
  square: wrap('<rect width="13" height="13" x="5.5" y="5.5" rx="2"/>'),
  close: wrap('<path d="M18 6 6 18"/><path d="m6 6 12 12"/>'),
  plus: wrap('<path d="M5 12h14"/><path d="M12 5v14"/>'),
  chevronDown: wrap('<path d="m6 9 6 6 6-6"/>'),
  chevronRight: wrap('<path d="m9 6 6 6-6 6"/>'),
  arrowUp: wrap('<path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>'),
  send: wrap(
    '<path d="M14.5 21.7a.5.5 0 0 0 .94-.03l6.5-19a.5.5 0 0 0-.63-.63l-19 6.5a.5.5 0 0 0-.03.94l7.93 3.18a2 2 0 0 1 1.11 1.11z"/><path d="M21.85 2.15 10.91 13.09"/>',
  ),
  folder: wrap(
    '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
  ),
  file: wrap(
    '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M16 13H8"/><path d="M16 17H8"/>',
  ),
  terminal: wrap('<path d="m4 17 6-6-6-6"/><path d="M12 19h8"/>'),
  shield: wrap(
    '<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>',
  ),
  refresh: wrap('<path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 4v5h-5"/>'),
  bug: wrap(
    '<path d="m8 2 1.9 1.9"/><path d="M14.1 3.9 16 2"/><path d="M9 7.1V6a3 3 0 1 1 6 0v1.1"/><path d="M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6"/><path d="M12 11v6"/><path d="M6.5 9C4.6 8.8 3 7.1 3 5"/><path d="M6 13H2"/><path d="M6 17l-3 2"/><path d="M17.5 9c1.9-.2 3.5-1.9 3.5-4"/><path d="M18 13h4"/><path d="M18 17l3 2"/>',
  ),
  brain: wrap(
    '<path d="M12 5a3 3 0 0 0-6 0 3 3 0 0 0-1 5.8V14a5 5 0 0 0 5 5h2z"/><path d="M12 5a3 3 0 0 1 6 0 3 3 0 0 1 1 5.8V14a5 5 0 0 1-5 5h-2z"/><path d="M12 5v14"/>',
  ),
  plug: wrap(
    '<path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z"/>',
  ),
  settings: wrap(
    '<path d="M12.2 2h-.4a2 2 0 0 0-2 2v.2a2 2 0 0 1-1 1.7l-.4.2a2 2 0 0 1-2 0l-.2-.1a2 2 0 0 0-2.7.7l-.2.4a2 2 0 0 0 .7 2.7l.2.1a2 2 0 0 1 1 1.7v.4a2 2 0 0 1-1 1.7l-.2.1a2 2 0 0 0-.7 2.7l.2.4a2 2 0 0 0 2.7.7l.2-.1a2 2 0 0 1 2 0l.4.2a2 2 0 0 1 1 1.7v.2a2 2 0 0 0 2 2h.4a2 2 0 0 0 2-2v-.2a2 2 0 0 1 1-1.7l.4-.2a2 2 0 0 1 2 0l.2.1a2 2 0 0 0 2.7-.7l.2-.4a2 2 0 0 0-.7-2.7l-.2-.1a2 2 0 0 1-1-1.7v-.4a2 2 0 0 1 1-1.7l.2-.1a2 2 0 0 0 .7-2.7l-.2-.4a2 2 0 0 0-2.7-.7l-.2.1a2 2 0 0 1-2 0l-.4-.2a2 2 0 0 1-1-1.7V4a2 2 0 0 0-2-2Z"/><circle cx="12" cy="12" r="3"/>',
  ),
  key: wrap(
    '<path d="M15.5 7.5 19 4"/><path d="m18 6 2 2"/><circle cx="9.5" cy="14.5" r="5.5"/>',
  ),
  server: wrap(
    '<rect width="18" height="7" x="3" y="3" rx="2"/><rect width="18" height="7" x="3" y="14" rx="2"/><path d="M7 6.5h.01"/><path d="M7 17.5h.01"/>',
  ),
  check: wrap('<path d="M20 6 9 17l-5-5"/>'),
  x: wrap('<path d="M18 6 6 18"/><path d="m6 6 12 12"/>'),
  spinner: wrap('<path d="M21 12a9 9 0 1 1-6.2-8.6"/>'),
  dot: wrap('<circle cx="12" cy="12" r="4"/>'),
  copy: wrap(
    '<rect width="14" height="14" x="8" y="8" rx="2"/><path d="M4 16a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2"/>',
  ),
} as const

export type IconName = keyof typeof ICONS
