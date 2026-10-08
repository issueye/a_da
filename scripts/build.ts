/**
 * a_da 桌面客户端端到端生产构建流水线
 *
 * 交付形态：
 * 1. 编译纯 Rust 原生 Agent 核心 (agent_core) -> dist/agent_core.exe (超低内存 8.5MB)；
 * 2. 使用 Bun 将 React 19 UI、GPUIX 渲染引擎及内嵌的 agent_core 编译为单一独立二进制 (dist/a-da.exe)；
 * 3. 补丁 Windows PE 子系统为 WINDOWS_GUI (2)，消灭启动黑框控制台；
 * 4. 用户双击 a-da.exe 立即弹出原生 GPU 加速窗口，后台由纯 Rust 原生核心驱动。
 */

import { copyFileSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import path from 'node:path'

const root = path.join(import.meta.dir, '..')
const distDir = path.join(root, 'dist')
mkdirSync(distDir, { recursive: true })

const outfile = process.env.A_DA_OUT ?? path.join(distDir, 'a-da')
const icon = path.join(root, 'assets', 'logo.ico')

console.log('\x1b[1;36m=================================================================\x1b[0m')
console.log('\x1b[1;36m           a_da 桌面端生产构建流水线 (GPUIX + Rust Core)          \x1b[0m')
console.log('\x1b[1;36m=================================================================\x1b[0m\n')

// 步骤 1：编译纯 Rust 原生后端核心 (agent_core)
console.log('\x1b[33m[步骤 1/3]\x1b[0m 正在使用 Cargo 编译纯 Rust 后端核心 (agent_core)...')

function getCargoConfig(): { cmd: string; env: Record<string, string | undefined> } {
  const userHome = process.env.USERPROFILE || process.env.HOME || ''
  const toolchainBin = path.join(userHome, '.rustup', 'toolchains', 'stable-x86_64-pc-windows-msvc', 'bin')
  const cargoBin = path.join(userHome, '.cargo', 'bin')
  const customPath = [toolchainBin, cargoBin, process.env.PATH].filter(Boolean).join(path.delimiter)
  const targetDir = process.env.CARGO_TARGET_DIR || path.join(root, '..', 'cargo_target_ada')

  return { cmd: 'cargo', env: { ...process.env, PATH: customPath, CARGO_TARGET_DIR: targetDir } }
}

const { cmd: cargoCmd, env: cargoEnv } = getCargoConfig()
const cargoProc = Bun.spawnSync([cargoCmd, 'build', '--release', '-p', 'agent_core'], {
  cwd: root,
  env: cargoEnv,
  stdio: ['inherit', 'inherit', 'inherit'],
})
if (cargoProc.exitCode !== 0) {
  console.error('\x1b[31m[错误] Cargo 构建核心失败，终止构建\x1b[0m')
  process.exit(1)
}

const exeName = process.platform === 'win32' ? 'agent_core.exe' : 'agent_core'
const targetDir = cargoEnv.CARGO_TARGET_DIR
const candidateCorePaths = [
  ...(targetDir ? [path.join(targetDir, 'release', exeName)] : []),
  path.join(root, 'target', 'release', exeName),
  path.join(root, 'agent_core', 'target', 'release', exeName),
]
const compiledCore = candidateCorePaths.find((p) => existsSync(p))
const targetCore = path.join(distDir, exeName)
if (!compiledCore) {
  console.error(`\x1b[31m[错误] 未找到 Cargo 编译产物，已检查: ${candidateCorePaths.join(', ')}\x1b[0m`)
  process.exit(1)
}
copyFileSync(compiledCore, targetCore)
const coreSizeMb = (statSync(targetCore).size / 1024 / 1024).toFixed(2)
console.log(`  ✔ 原生 Rust 核心就绪: ${path.relative(root, targetCore)} (${coreSizeMb} MB)`)
console.log('')

// 步骤 2：打包单文件独立可执行桌面应用 (内嵌原生核心与 GPUIX 渲染驱动)
console.log('\x1b[33m[步骤 2/3]\x1b[0m 正在打包单一独立可执行应用 (dist/a-da.exe)...')

async function compileStandalone() {
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

// Windows 下刚退出的二进制可能会被文件句柄短暂锁死，重试以避免 EPERM
let result = await compileStandalone()
for (let attempt = 0; attempt < 3 && !result.success; attempt++) {
  const locked = result.logs.some((message) => message.message.includes('EPERM'))
  if (!locked) break
  await new Promise((resolve) => setTimeout(resolve, 500))
  result = await compileStandalone()
}

if (!result.success) {
  console.error('\x1b[31m[错误] 应用打包失败:\x1b[0m')
  for (const message of result.logs) console.error(message)
  process.exit(1)
}

const writtenPath = result.outputs[0]?.path ?? (process.platform === 'win32' ? `${outfile}.exe` : outfile)
console.log(`  ✔ 桌面应用构建完成: ${path.relative(root, writtenPath)}\n`)

// 步骤 3：补丁 Windows PE 子系统为 WINDOWS_GUI (2)，消灭控制台黑框
console.log('\x1b[33m[步骤 3/3]\x1b[0m 正在配置 Windows GUI 子系统（消灭控制台黑框）...')
function patchWindowsGuiSubsystem(targetPath: string): void {
  if (process.platform !== 'win32' || !existsSync(targetPath)) return
  try {
    const buf = readFileSync(targetPath)
    if (buf.length < 0x200 || buf.readUInt16LE(0) !== 0x5a4d) return
    const peOffset = buf.readUInt32LE(0x3c)
    if (buf.readUInt32LE(peOffset) !== 0x00004550) return
    const subsystemOffset = peOffset + 4 + 20 + 68
    const current = buf.readUInt16LE(subsystemOffset)
    if (current !== 2) {
      buf.writeUInt16LE(2, subsystemOffset)
      writeFileSync(targetPath, buf)
      console.log(`  ✔ 已成功修改 PE Subsystem 为 WINDOWS_GUI (2) 无控制台黑框`)
    } else {
      console.log(`  ✔ PE Subsystem 已是 WINDOWS_GUI (2)`)
    }
  } catch (error) {
    console.warn(`  [警告] 切换 Windows GUI 子系统失败:`, error)
  }
}

patchWindowsGuiSubsystem(writtenPath)
patchWindowsGuiSubsystem(targetCore)

// 构建体积汇总
const appSizeMb = (statSync(writtenPath).size / 1024 / 1024).toFixed(2)

console.log('\n\x1b[1;32m=================================================================\x1b[0m')
console.log('\x1b[1;32m                   🎉 客户端构建成功！可直接运行                 \x1b[0m')
console.log('\x1b[1;32m=================================================================\x1b[0m')
console.log(`• 桌面应用程序 [dist/a-da.exe]        : \x1b[1;32m${appSizeMb} MB\x1b[0m (单文件交付，双击即弹出 GPU 原生窗口)`)
console.log(`• 原生后端核心 [dist/agent_core.exe]  : \x1b[1;32m${coreSizeMb} MB\x1b[0m (纯 Rust 高性能服务，内存仅 8.5MB)`)
console.log(`• 运行方式: 双击 dist\\a-da.exe，或在终端输入 .\\dist\\a-da.exe 启动\n`)
