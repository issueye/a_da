/**
 * Link this app to the local GPUIX checkout.
 *
 * `@gpuix/react` and `@gpuix/native` are built from `../gpuix`, and the app has
 * to share that checkout's single React copy: the reconciler installs the hook
 * dispatcher on the React module it imported, so a second copy in this project
 * would make every `useState` fail.
 *
 * `bun install` cannot do this, because `@gpuix/react` depends on
 * `@gpuix/native` through the `workspace:` protocol that only resolves inside
 * the gpuix repository. So the three links are created here, as directory
 * junctions, which need no administrator rights on Windows.
 *
 * Run `bun run link` once after cloning, and again after moving either folder.
 */

import { cpSync, existsSync, lstatSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'

const here = dirname(import.meta.dir)
const gpuix = resolve(here, '..', 'gpuix')

const links: [string, string][] = [
  ['node_modules/@gpuix/react', join(gpuix, 'packages/react')],
  ['node_modules/@gpuix/native', join(gpuix, 'packages/native')],
  ['node_modules/react', join(gpuix, 'node_modules/react')],
  ['node_modules/react-reconciler', join(gpuix, 'node_modules/react-reconciler')],
]

let linked = 0
for (const [from, target] of links) {
  const path = join(here, from)
  if (!existsSync(target)) {
    console.error(`找不到 ${target}，请先在 ../gpuix 运行 bun install 与 bun run build`)
    process.exit(1)
  }
  const current = existsSync(path) ? lstatSync(path) : null
  if (current?.isSymbolicLink() && resolve(path).toLowerCase() === target.toLowerCase()) {
    continue
  }
  if (current) {
    // A real directory from a previous `bun install`, or a stale link: the app
    // must resolve React and the GPUIX packages from the gpuix checkout.
    rmSync(path, { recursive: true, force: true })
  }
  mkdirSync(dirname(path), { recursive: true })
  try {
    symlinkSync(target, path, 'junction')
  } catch {
    // On file systems that don't support junctions/symlinks (like exFAT on Windows), fall back to copy.
    cpSync(target, path, { recursive: true })
  }
  linked += 1
}

const dist = join(gpuix, 'packages/react/dist/index.js')
if (!existsSync(dist)) {
  console.error(`@gpuix/react 还没编译：请先在 ../gpuix/packages/react 运行 bun run build`)
  process.exit(1)
}

// The native addon is loaded from the linked package; copy nothing, just check.
const nativeIndex = join(gpuix, 'packages/native/index.js')
if (!existsSync(nativeIndex)) {
  console.error('@gpuix/native 缺少 index.js，请先在 ../gpuix/packages/native 运行 bun run build')
  process.exit(1)
}

// 确保 @gpuix/native 的 exports.import 指向 index.cjs，使 bun build --compile 能静态识别 require 并将 .node 二进制内嵌打包
const nativePkgPath = join(gpuix, 'packages/native/package.json')
if (existsSync(nativePkgPath)) {
  try {
    const pkg = JSON.parse(readFileSync(nativePkgPath, 'utf-8'))
    if (pkg.exports?.['.']?.import === './index.js') {
      pkg.exports['.'].import = './index.cjs'
      pkg.exports['.'].default = './index.cjs'
      writeFileSync(nativePkgPath, JSON.stringify(pkg, null, 2) + '\n')
    }
  } catch {}
}

console.log(`已连接 ${links.length} 个本地包（新建 ${linked} 个）`)
