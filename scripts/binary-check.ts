/**
 * Prove the compiled binary is a real app: start `dist/a-da.exe`, wait for the
 * first frame, screenshot it, and close it.
 *
 *   bun run build && bun scripts/binary-check.ts
 */

import { existsSync, mkdirSync, rmSync } from 'node:fs'
import path from 'node:path'
import { launch } from '@gpuix/react/automation'

const root = path.join(import.meta.dir, '..')
const coreBinary = path.join(root, 'dist', 'a-da-core.exe')
const binary = process.platform === 'win32' && existsSync(coreBinary)
  ? coreBinary
  : path.join(root, 'dist', process.platform === 'win32' ? 'a-da.exe' : 'a-da')
mkdirSync(path.join(root, 'tmp'), { recursive: true })

// 隔离数据目录：验证的是「干净启动画出欢迎页」，不能被真实会话恢复干扰
const CHECK_HOME = path.join(root, 'tmp', 'binary-home')
rmSync(CHECK_HOME, { recursive: true, force: true })

const app = await launch({
  command: binary,
  args: [],
  cwd: root,
  env: { GPUIX_BACKGROUND: '1', A_DA_HOME: CHECK_HOME },
})
await app.getByTestId('welcome').waitFor({ timeoutMs: 60_000 })
await app.clock.pause()
await app.screenshot({ path: path.join(root, 'tmp', 'binary.png') })
await app.clock.resume()
await app.close()

console.log(`[binary-check] ${path.relative(root, binary)} launched and painted`)
