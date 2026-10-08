/**
 * 归档门禁（verify-archive）
 *
 * 目的：保证 `archive/ts-legacy/` 是**真的归档**——不再被主干的任何代码/配置引用，
 * 且主干里不存在第二份 agent 引擎或 TS 宿主残留。
 *
 * 用法：`bun tools/verify-archive.ts`（或 `bun run verify:archive`）
 * 退出码：0 = 全绿；1 = 有断言失败
 *
 * 说明：本脚本是 M0 之前的最小实现，之后会并入 `cargo xtask verify-archive`
 * （见 docs/agent-base-plan.md §5.8）。
 */

import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative, sep } from 'node:path'

const ROOT = join(import.meta.dir, '..')
const ARCHIVE = 'archive'

/** 主干侧需要扫描的目录（归档/deps/产物除外） */
const SCAN_DIRS = ['agent_core', 'src-tauri', 'tauri-ui', 'ts_engine', 'tools']
/** 主干侧需要扫描的根文件 */
const SCAN_ROOT_FILES = ['Cargo.toml', 'package.json', 'Cargo.lock']
/** 只看这些扩展名的内容（.md 是文档，允许提到归档路径） */
const SCAN_EXTS = ['.rs', '.ts', '.tsx', '.js', '.jsx', '.json', '.toml', '.html', '.css', '.mjs', '.cjs']
const SKIP_DIRS = new Set(['node_modules', 'target', 'dist', 'tmp', ARCHIVE, '.git', '.ada', '.commandcode'])

const failures: string[] = []

function rel(p: string): string {
  return relative(ROOT, p).split(sep).join('/')
}

function walk(dir: string, out: string[] = []): string[] {
  if (!existsSync(dir)) return out
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue
      walk(full, out)
    } else if (entry.isFile()) {
      if (SKIP_DIRS.has(entry.name)) continue
      if (SCAN_EXTS.some((e) => entry.name.endsWith(e))) out.push(full)
    }
  }
  return out
}

function readAll(): { path: string; text: string }[] {
  const files: string[] = []
  for (const d of SCAN_DIRS) walk(join(ROOT, d), files)
  for (const f of SCAN_ROOT_FILES) {
    const p = join(ROOT, f)
    if (existsSync(p)) files.push(p)
  }
  return files
    .filter((p) => !p.endsWith('verify-archive.ts'))
    .map((p) => ({ path: rel(p), text: readFileSync(p, 'utf8') }))
}

function check(title: string, ok: boolean, detail: string[] = []) {
  if (ok) {
    console.log(`  ✔ ${title}`)
    return
  }
  console.log(`  ✘ ${title}`)
  for (const d of detail) console.log(`      ${d}`)
  failures.push(title)
}

const files = readAll()
console.log(`\nverify-archive：扫描 ${files.length} 个主干文件（跳过 archive/、node_modules、target、dist）\n`)

// 1. 主干不得引用归档
{
  const hits: string[] = []
  for (const f of files) {
    f.text.split('\n').forEach((line, i) => {
      // 只关心真的当路径用：import/require/字符串里出现 archive/
      if (/(from|require\(|import\(|["'`(])\s*['"`]?[^'"`\s]*archive[\\/]/.test(line)) {
        hits.push(`${f.path}:${i + 1}  ${line.trim().slice(0, 120)}`)
      }
    })
  }
  check('主干没有任何指向 archive/ 的引用', hits.length === 0, hits.slice(0, 20))
}

// 2. 只有一个 agent 引擎
{
  const tsLoop: string[] = []
  const rustLoop: string[] = []
  for (const f of files) {
    f.text.split('\n').forEach((line, i) => {
      if (f.path.endsWith('.ts') || f.path.endsWith('.tsx')) {
        if (/\brunAgentLoop\b/.test(line)) tsLoop.push(`${f.path}:${i + 1}`)
      }
      if (f.path.endsWith('.rs') && /fn\s+run_agent_loop\s*\(/.test(line)) rustLoop.push(`${f.path}:${i + 1}`)
    })
  }
  check('主干不存在 TS 侧多轮循环（runAgentLoop）', tsLoop.length === 0, tsLoop)
  check(
    'Rust 侧引擎入口恰好一处（fn run_agent_loop）',
    rustLoop.length === 1,
    rustLoop.length === 0 ? ['未找到 — 引擎入口丢失？'] : rustLoop.slice(1).map((x) => `重复定义: ${x}`),
  )
}

// 3. 无 TS 宿主残留
{
  const residues = [
    'A_DA_FORCE_LEGACY_HOST',
    'createInProcessClient',
    'hostEntryArgs',
    'createCommandDispatcher',
    'startHostServer',
    'createHostEmitter',
  ]
  const hits: string[] = []
  for (const f of files) {
    for (const r of residues) {
      if (f.text.includes(r)) hits.push(`${f.path}  含 ${r}`)
    }
  }
  check('主干不存在 TS 宿主残留符号', hits.length === 0, hits.slice(0, 20))
}

// 4. 构建入口不再指向归档
{
  const details: string[] = []
  const pkgPath = join(ROOT, 'package.json')
  const pkg = JSON.parse(readFileSync(pkgPath, 'utf8')) as { scripts?: Record<string, string> }
  for (const [name, cmd] of Object.entries(pkg.scripts ?? {})) {
    if (/app\.tsx|screenshot\.ts|scripts\//.test(cmd)) details.push(`package.json scripts.${name} = ${cmd}`)
  }
  let scriptsDirHasFiles = false
  if (existsSync(join(ROOT, 'scripts'))) {
    scriptsDirHasFiles = readdirSync(join(ROOT, 'scripts')).length > 0
  }
  if (scriptsDirHasFiles) {
    scriptsDirHasFiles = walk(join(ROOT, 'scripts')).length > 0
  }
  if (scriptsDirHasFiles) details.push('根 scripts/ 仍有文件')
  if (existsSync(join(ROOT, 'tsconfig.json'))) details.push('根 tsconfig.json 仍存在（会去解析已归档代码）')
  // bunfig.toml 允许存在，但只允许用于"把 archive 排除在 test 发现之外"：
  // 一旦它重新指向已归档的脚本（preload/app.tsx），或不再排除 archive，就算回归。
  const bunfigPath = join(ROOT, 'bunfig.toml')
  if (existsSync(bunfigPath)) {
    const bunfig = readFileSync(bunfigPath, 'utf8')
    if (/scripts\/|app\.tsx|src\//.test(bunfig)) details.push('bunfig.toml 仍指向已归档的脚本/入口')
    if (!bunfig.includes('archive/')) details.push('bunfig.toml 未把 archive/ 排除在 test 发现之外')
  }
  if (existsSync(join(ROOT, 'app.tsx'))) details.push('根 app.tsx 仍存在')
  if (existsSync(join(ROOT, 'src'))) details.push('根 src/ 仍存在')
  check('构建/测试入口不再指向归档', details.length === 0, details)
}

// 5. 死 shim 已删
{
  const details: string[] = []
  for (const p of ['agent_core/src/kernel', 'agent_core/src/compiler']) {
    if (existsSync(join(ROOT, p))) details.push(`${p} 仍存在`)
  }
  const lib = readFileSync(join(ROOT, 'agent_core/src/lib.rs'), 'utf8')
  for (const m of ['pub mod kernel;', 'pub mod compiler;']) {
    if (lib.includes(m)) details.push(`agent_core/src/lib.rs 仍有 ${m}`)
  }
  check('kernel/compiler 转发 shim 已删除', details.length === 0, details)
}

// 6. 归档本体存在（防止误删）
{
  const details: string[] = []
  for (const p of ['archive/README.md', 'archive/ts-legacy/src/agent', 'archive/ts-legacy/scripts']) {
    if (!existsSync(join(ROOT, p))) details.push(`${p} 缺失`)
  }
  check('归档本体完整', details.length === 0, details)
}

console.log('')
if (failures.length > 0) {
  console.error(`verify-archive 失败：${failures.length} 条断言未通过\n`)
  process.exit(1)
}
console.log('verify-archive 全绿：主干没有第二份实现，归档不再被引用。\n')
