/**
 * Ship a single executable.
 *
 * `bun build --compile` embeds the JavaScript, the React reconciler and the
 * `@gpuix/native` addon, so the result runs on a machine with no Bun and no
 * Node install. On Windows `hideConsole` matters: without it the app opens with
 * a console window behind it.
 *
 *   bun run build              writes dist/a-da (dist/a-da.exe on Windows)
 */

import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import path from 'node:path'

const root = path.join(import.meta.dir, '..')
const outfile = process.env.A_DA_OUT ?? path.join(root, 'dist', 'a-da')
// `bun scripts/make-icon.tsx` writes this from assets/logo.svg.
const icon = path.join(root, 'assets', 'logo.ico')

mkdirSync(path.dirname(outfile), { recursive: true })

/**
 * Patch the PE header of the compiled Windows executable to mark it as
 * IMAGE_SUBSYSTEM_WINDOWS_GUI (subsystem 2) rather than IMAGE_SUBSYSTEM_WINDOWS_CUI (subsystem 3).
 *
 * Bun's `compile.windows.hideConsole` option has an upstream issue where the emitted PE binary
 * still retains Subsystem 3 (console), causing Windows to pop open a command prompt console.
 * Patching offset 0x44 in the PE OptionalHeader permanently prevents Windows from allocating a console.
 */
function patchWindowsGuiSubsystem(targetPath: string): void {
  if (process.platform !== 'win32' || !existsSync(targetPath)) return
  try {
    const buf = readFileSync(targetPath)
    if (buf.length < 0x40) return
    const peOffset = buf.readUInt32LE(0x3C)
    if (peOffset + 24 + 68 + 2 > buf.length) return
    if (buf.toString('ascii', peOffset, peOffset + 4) !== 'PE\0\0') return
    const magic = buf.readUInt16LE(peOffset + 24)
    if (magic !== 0x10b && magic !== 0x20b) return
    const subsystemOffset = peOffset + 24 + 68
    const current = buf.readUInt16LE(subsystemOffset)
    if (current !== 2) {
      buf.writeUInt16LE(2, subsystemOffset)
      writeFileSync(targetPath, buf)
      console.log(`[build] patched ${path.relative(root, targetPath)} PE Subsystem to WINDOWS_GUI (2)`)
    }
  } catch (error) {
    console.warn(`[build] failed to patch Windows GUI subsystem:`, error)
  }
}

async function build() {
  return Bun.build({
    entrypoints: [path.join(root, 'app.tsx')],
    minify: false,
    compile: {
      outfile,
      windows:
        process.platform === 'win32'
          ? {
              hideConsole: true,
              title: 'a_da',
              publisher: 'a_da',
              version: '0.1.0',
              description: 'A local coding agent rendered with GPUIX',
              ...(existsSync(icon) ? { icon } : {}),
            }
          : undefined,
    },
  })
}

// Windows keeps a just-exited executable locked for a moment, which makes Bun
// fail with EPERM while it moves the new binary into place.
let result = await build()
for (let attempt = 0; attempt < 3 && !result.success; attempt++) {
  const locked = result.logs.some((message) => message.message.includes('EPERM'))
  if (!locked) break
  await new Promise((resolve) => setTimeout(resolve, 500))
  result = await build()
}

if (!result.success) {
  for (const message of result.logs) console.error(message)
  process.exit(1)
}

const writtenPath = result.outputs[0]?.path ?? (process.platform === 'win32' ? `${outfile}.exe` : outfile)
console.log(`[build] wrote ${path.relative(root, writtenPath)}`)

// 直接将单一二进制文件的 PE Subsystem 修改为 WINDOWS_GUI (2)，
// 确保 Windows 原生以纯 GUI 程序启动，零控制台黑框、无需外挂任何启动器或多余二进制文件。
patchWindowsGuiSubsystem(writtenPath)

// 清理历史残留的多余核心二进制
if (process.platform === 'win32') {
  const legacyCore = path.join(root, 'dist', 'a-da-core.exe')
  if (existsSync(legacyCore)) {
    try { rmSync(legacyCore, { force: true }) } catch {}
  }
}
