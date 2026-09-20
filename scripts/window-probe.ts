/**
 * List every visible top-level window a process owns: class name and size.
 *
 *   bun scripts/window-probe.ts <pid> [pid...]
 *
 * The app's own `findAppWindow` only answers "is there *a* window", which is not
 * enough when the question is *which* window appeared: a console window answers
 * it just as well as a dialog. This is the magnifier for that — it is how the
 * stray console beside the directory picker was found (`ConsoleWindowClass`
 * sitting next to the `#32770` dialog of the same powershell process).
 */

import { FFIType, JSCallback, dlopen } from 'bun:ffi'

const user32 = dlopen('user32.dll', {
  EnumWindows: { args: [FFIType.function, FFIType.i64], returns: FFIType.bool },
  GetWindowThreadProcessId: { args: [FFIType.i64, FFIType.ptr], returns: FFIType.u32 },
  IsWindowVisible: { args: [FFIType.i64], returns: FFIType.bool },
  GetClassNameW: { args: [FFIType.i64, FFIType.ptr, FFIType.i32], returns: FFIType.i32 },
  GetWindowRect: { args: [FFIType.i64, FFIType.ptr], returns: FFIType.bool },
})

const wanted = new Set(process.argv.slice(2).map(Number))
const owner = new Uint32Array(1)
const classBuf = new Uint16Array(256)
const rect = new Int32Array(4)
const found: string[] = []

const callback = new JSCallback(
  (hwnd: number) => {
    user32.symbols.GetWindowThreadProcessId(hwnd, owner)
    const pid = owner[0] ?? 0
    if (!wanted.has(pid) || !user32.symbols.IsWindowVisible(hwnd)) return true
    classBuf.fill(0)
    const len = Number(user32.symbols.GetClassNameW?.(hwnd, classBuf, 256) ?? 0)
    const cls = len > 0 ? String.fromCharCode(...classBuf.slice(0, len)) : ''
    user32.symbols.GetWindowRect(hwnd, rect)
    const w = (rect[2] ?? 0) - (rect[0] ?? 0)
    const h = (rect[3] ?? 0) - (rect[1] ?? 0)
    found.push(`pid=${pid} class=${cls} ${w}x${h} at (${rect[0]},${rect[1]})`)
    return true
  },
  { args: [FFIType.i64, FFIType.i64], returns: FFIType.bool }
)

// `ptr` is only null for a closed callback; this one is alive right here.
user32.symbols.EnumWindows(callback.ptr!, 0)
callback.close()
console.log(found.length ? found.join('\n') : '(没有找到窗口)')
