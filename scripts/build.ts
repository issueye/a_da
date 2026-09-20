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

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
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
 * still retains Subsystem 3 (console), causing Windows to always pop open a command prompt console.
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

const coreOutfile =
  process.platform === 'win32'
    ? (process.env.A_DA_OUT ? `${process.env.A_DA_OUT}-core` : path.join(root, 'dist', 'a-da-core'))
    : (process.env.A_DA_OUT ?? path.join(root, 'dist', 'a-da'))

async function build() {
  return Bun.build({
    entrypoints: [path.join(root, 'app.tsx')],
    minify: false,
    compile: {
      outfile: coreOutfile,
      windows:
        process.platform === 'win32'
          ? {
              hideConsole: false,
              title: 'a_da core',
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

const writtenPath = result.outputs[0]?.path ?? (process.platform === 'win32' ? `${coreOutfile}.exe` : coreOutfile)
console.log(`[build] wrote ${path.relative(root, writtenPath)}`)

// Bun 编出来的 core 带的是控制台子系统，双击会先弹一个黑框再进图形界面。
// 直接改 PE 头，让 Windows 一开始就不为它分配控制台。
patchWindowsGuiSubsystem(writtenPath)

// 在 Windows 环境下，编译原生 Subsystem 2 (winexe) 纯 GUI 启动器：
// 用户双击 a-da.exe 时，0 毫秒无黑框，通过 CREATE_NO_WINDOW 静默唤起核心引擎，完美呈现图形界面
if (process.platform === 'win32') {
  const cscPaths = [
    'C:\\Windows\\Microsoft.NET\\Framework64\\v4.0.30319\\csc.exe',
    'C:\\Windows\\Microsoft.NET\\Framework\\v4.0.30319\\csc.exe',
  ]
  const csc = cscPaths.find((p) => existsSync(p))
  const launcherSource = path.join(root, 'scripts', 'launcher.cs')
  const launcherOut = path.join(root, 'dist', 'a-da.exe')

  if (csc && existsSync(launcherSource)) {
    const cmd = [
      csc,
      '/target:winexe',
      '/nologo',
      '/optimize+',
      ...(existsSync(icon) ? [`/win32icon:${icon}`] : []),
      `/out:${launcherOut}`,
      launcherSource,
    ]
    const proc = Bun.spawnSync({ cmd })
    if (proc.exitCode === 0) {
      console.log(`[build] compiled native GUI launcher: ${path.relative(root, launcherOut)}`)
    } else {
      console.warn(`[build] warning: csc compilation exited with ${proc.exitCode}: ${proc.stderr?.toString()}`)
    }
  }
}
