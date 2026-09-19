/**
 * Windows window chrome for a frameless GPUI window.
 *
 * GPUI opens a borderless window on Windows, so the title row is ours to draw
 * and the OS never sees a caption.
 *
 * Dragging is done here, not by Windows. The usual recipes do not work under
 * gpui: `WM_NCLBUTTONDOWN` + `HTCAPTION` is handled by gpui's own window
 * procedure and its input dispatch consumes it, and `WM_SYSCOMMAND` / `SC_MOVE`
 * never enters a move loop either (measured with `GetGUIThreadInfo`: the UI
 * thread stays out of `GUI_INMOVESIZE`). So the move is computed instead, from
 * the grab offset: the cursor's screen position minus where the window was when
 * the press happened stays constant for the whole gesture, so
 * `cursor - offset` is the window's new origin.
 *
 * Screen coordinates and not the pointer event's: `SetWindowPos` is applied
 * asynchronously by the owning thread, so an event's client coordinates can be
 * measured against a window position that has not caught up yet — dragging by
 * those lands on exactly half the distance on Windows. The cursor does not lie.
 * Synthetic input never moves the real cursor, so that case falls back to the
 * event coordinates, which is also what a touch digitizer needs.
 * The trade is Aero Snap, which only the system loop can do.
 *
 * `ShowWindow` covers minimize and maximize. Nothing here runs on macOS or
 * Linux: there the platform owns the traffic lights.
 *
 * Everything is addressed by process id, so `controlWindow(otherPid)` can drive
 * a window this process did not create — that is what `scripts/`
 * `window-controls.ts` uses to check these calls against a real window. Every
 * call is guarded: on a machine without user32 the app still runs, only without
 * window buttons.
 */

import { FFIType, JSCallback, dlopen } from 'bun:ffi'

const WM_CLOSE = 0x0010
const SW_MAXIMIZE = 3
const SW_MINIMIZE = 6
const SW_RESTORE = 9
const SWP_NOSIZE = 0x0001
const SWP_NOZORDER = 0x0004
const SWP_NOACTIVATE = 0x0010
const VK_LBUTTON = 0x01

type Symbols = Record<string, (...args: any[]) => number | boolean>
type Lib = { symbols: Symbols }

let loaded = false
let lib: Lib | null = null

function load(): Lib | null {
  if (loaded) return lib
  loaded = true
  if (process.platform !== 'win32') return null
  try {
    lib = dlopen('user32.dll', {
      EnumWindows: { args: [FFIType.function, FFIType.i64], returns: FFIType.bool },
      GetWindowThreadProcessId: { args: [FFIType.i64, FFIType.ptr], returns: FFIType.u32 },
      IsWindowVisible: { args: [FFIType.i64], returns: FFIType.bool },
      IsZoomed: { args: [FFIType.i64], returns: FFIType.bool },
      IsIconic: { args: [FFIType.i64], returns: FFIType.bool },
      GetWindowRect: { args: [FFIType.i64, FFIType.ptr], returns: FFIType.bool },
      GetCursorPos: { args: [FFIType.ptr], returns: FFIType.bool },
      GetAsyncKeyState: { args: [FFIType.i32], returns: FFIType.i16 },
      GetDpiForWindow: { args: [FFIType.i64], returns: FFIType.u32 },
      ShowWindow: { args: [FFIType.i64, FFIType.i32], returns: FFIType.bool },
      SetWindowPos: {
        args: [
          FFIType.i64,
          FFIType.i64,
          FFIType.i32,
          FFIType.i32,
          FFIType.i32,
          FFIType.i32,
          FFIType.u32,
        ],
        returns: FFIType.bool,
      },
      ReleaseCapture: { args: [], returns: FFIType.bool },
      SendMessageW: {
        args: [FFIType.i64, FFIType.u32, FFIType.i64, FFIType.i64],
        returns: FFIType.i64,
      },
    }) as unknown as Lib
  } catch {
    lib = null
  }
  return lib
}

const windows = new Map<number, number>()

/** The first visible top-level window owned by `pid` is that process's GPUI window. */
export function findAppWindow(pid: number = process.pid): number | null {
  const cached = windows.get(pid)
  if (cached) return cached
  const api = load()
  if (!api) return null
  const owner = new Uint32Array(1)
  const holder: { hwnd: number | null } = { hwnd: null }
  const callback = new JSCallback(
    (candidate: number) => {
      api.symbols.GetWindowThreadProcessId(candidate, owner)
      if (owner[0] === pid && api.symbols.IsWindowVisible(candidate)) {
        holder.hwnd = candidate
        return false
      }
      return true
    },
    { args: [FFIType.i64, FFIType.i64], returns: FFIType.bool },
  )
  try {
    api.symbols.EnumWindows(callback.ptr, 0)
  } catch {
    holder.hwnd = null
  } finally {
    callback.close()
  }
  if (holder.hwnd) windows.set(pid, holder.hwnd)
  return holder.hwnd
}

export interface WindowState {
  x: number
  y: number
  width: number
  height: number
  minimized: boolean
  maximized: boolean
}

/**
 * The real OS state of a window, which is what the drawn buttons have to move.
 * Layout inside the window cannot see any of this.
 */
export function readWindowState(pid: number = process.pid): WindowState | null {
  const api = load()
  const hwnd = findAppWindow(pid)
  if (!api || !hwnd) return null
  const rect = new Int32Array(4) // left, top, right, bottom
  try {
    api.symbols.GetWindowRect(hwnd, rect)
  } catch {
    return null
  }
  return {
    x: rect[0]!,
    y: rect[1]!,
    width: rect[2]! - rect[0]!,
    height: rect[3]! - rect[1]!,
    minimized: Boolean(api.symbols.IsIconic(hwnd)),
    maximized: Boolean(api.symbols.IsZoomed(hwnd)),
  }
}

export interface Point {
  x: number
  y: number
}

export interface DragResult {
  ok: boolean
  detail: string
}

export interface WindowControls {
  /** The app draws its own window buttons only where the OS has none. */
  custom: boolean
  /**
   * Start a move from a press at `at`, in the window's own client coordinates.
   * The result is returned rather than swallowed, because a window that will not
   * drag is impossible to diagnose from the outside.
   */
  beginDrag(at: Point): DragResult
  /**
   * Follow the pointer, in the same client coordinate space as `beginDrag`.
   * Returns the position it applied, which is what the drag probe reads.
   */
  moveDrag(at: Point): string
  endDrag(): void
  minimize(): void
  toggleMaximize(): void
  restore(): void
  close(): void
}

interface ActiveDrag {
  hwnd: number
  originX: number
  originY: number
  pressX: number
  pressY: number
  /** Cursor screen position when the press happened. */
  cursorX: number
  cursorY: number
  /** Where the grab point sits inside the window, which the window keeps. */
  grabX: number
  grabY: number
  /** Client coordinates are logical points; `SetWindowPos` wants physical pixels. */
  scale: number
  /**
   * Whether to trust the physical button to end the gesture. A real drag starts
   * with the button down; synthetic input never does, and the real cursor can
   * move for unrelated reasons while it runs, so policing that gesture with the
   * button would cut it short.
   */
  watchButton: boolean
}

/** One window is dragged at a time, per process. */
let activeDrag: ActiveDrag | null = null

function windowOrigin(api: Lib, hwnd: number): Point | null {
  const rect = new Int32Array(4)
  try {
    if (!api.symbols.GetWindowRect(hwnd, rect)) return null
  } catch {
    return null
  }
  return { x: rect[0]!, y: rect[1]! }
}

function cursorPos(api: Lib): Point | null {
  const point = new Int32Array(2)
  try {
    if (!api.symbols.GetCursorPos(point)) return null
  } catch {
    return null
  }
  return { x: point[0]!, y: point[1]! }
}

function leftButtonDown(api: Lib): boolean {
  try {
    const state = api.symbols.GetAsyncKeyState(VK_LBUTTON)
    // The high bit is "currently down"; the value arrives sign-extended.
    return typeof state === 'number' && (state & 0x8000) !== 0
  } catch {
    return false
  }
}

function dpiScale(api: Lib, hwnd: number): number {
  try {
    const dpi = api.symbols.GetDpiForWindow(hwnd)
    return typeof dpi === 'number' && dpi > 0 ? dpi / 96 : 1
  } catch {
    return 1
  }
}

/** Controls for one process's window. The app uses `windowControls` below. */
export function controlWindow(pid: number = process.pid): WindowControls {
  const run = (action: (api: Lib, hwnd: number) => void) => {
    const api = load()
    const hwnd = findAppWindow(pid)
    if (!api || !hwnd) return
    try {
      action(api, hwnd)
    } catch {
      /* window chrome is a nicety; never let it break the click that started it */
    }
  }

  return {
    custom: process.platform === 'win32',

    beginDrag(at: Point): DragResult {
      const api = load()
      if (!api) return { ok: false, detail: 'user32 不可用' }
      const hwnd = findAppWindow(pid)
      if (!hwnd) return { ok: false, detail: `找不到进程 ${pid} 的顶层窗口` }
      if (api.symbols.IsZoomed(hwnd)) return { ok: false, detail: '窗口已最大化，先还原' }
      const origin = windowOrigin(api, hwnd)
      const cursor = cursorPos(api)
      if (!origin || !cursor) return { ok: false, detail: '读不到窗口位置或光标' }
      activeDrag = {
        hwnd,
        originX: origin.x,
        originY: origin.y,
        pressX: at.x,
        pressY: at.y,
        cursorX: cursor.x,
        cursorY: cursor.y,
        grabX: cursor.x - origin.x,
        grabY: cursor.y - origin.y,
        scale: dpiScale(api, hwnd),
        watchButton: leftButtonDown(api),
      }
      return { ok: true, detail: `hwnd=${hwnd}` }
    },

    moveDrag(at: Point): string {
      const drag = activeDrag
      if (!drag) return 'no drag'
      const api = load()
      if (!api) return 'no user32'
      const cursor = cursorPos(api)
      // Which is the truth depends on how the gesture started. With a real
      // button held, the cursor is the drag, and the events are only a way to
      // hear about it. Without one — synthetic input, a touch digitizer — the
      // cursor is unrelated to the gesture and the event coordinates are all
      // there is. Letting the cursor win in both cases made any unrelated cursor
      // drift hijack a synthetic drag.
      const useCursor = drag.watchButton && cursor != null
      if (drag.watchButton && !leftButtonDown(api)) {
        // The physical button is up, so the gesture is over even if its up event
        // was lost. Doing this only for a gesture that started with the button
        // down is what keeps the window from sticking to the pointer afterwards.
        activeDrag = null
        return 'released'
      }
      const x = useCursor
        ? cursor.x - drag.grabX
        : drag.originX + Math.round((at.x - drag.pressX) * drag.scale)
      const y = useCursor
        ? cursor.y - drag.grabY
        : drag.originY + Math.round((at.y - drag.pressY) * drag.scale)
      try {
        api.symbols.SetWindowPos(
          drag.hwnd,
          0,
          x,
          y,
          0,
          0,
          SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
      } catch {
        activeDrag = null
        return 'SetWindowPos threw'
      }
      return `${x},${y}${useCursor ? ' cursor' : ' event'}`
    },

    endDrag(): void {
      activeDrag = null
    },

    minimize() {
      run((api, hwnd) => {
        api.symbols.ShowWindow(hwnd, SW_MINIMIZE)
      })
    },

    toggleMaximize() {
      run((api, hwnd) => {
        api.symbols.ShowWindow(hwnd, api.symbols.IsZoomed(hwnd) ? SW_RESTORE : SW_MAXIMIZE)
      })
    },

    restore() {
      run((api, hwnd) => {
        api.symbols.ShowWindow(hwnd, SW_RESTORE)
      })
    },

    close() {
      activeDrag = null
      const api = load()
      const hwnd = findAppWindow(pid)
      if (api && hwnd) {
        try {
          api.symbols.SendMessageW(hwnd, WM_CLOSE, 0, 0)
          // GPUI quits when its last window closes. If the close was ignored, do
          // not leave the user with a window that cannot be dismissed.
          if (pid === process.pid) setTimeout(() => process.exit(0), 600)
          return
        } catch {
          /* fall through to exit */
        }
      }
      if (pid === process.pid) process.exit(0)
    },
  }
}

export const windowControls: WindowControls = controlWindow()
