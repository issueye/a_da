/**
 * 将 UI 界面全量打包为紧凑的单文件 Bundle
 *
 * 脱离 Bun 运行时的关键步骤：
 * 将 60 个 React TSX 组件、状态树与适配层打包为单个独立的 JavaScript 产物，
 * 供轻量级嵌入式引擎（Hermes / QuickJS / 原生宿主）直接加载执行。
 */
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import ts from 'typescript'

const root = path.join(import.meta.dir, '..')
const outfile = path.join(root, 'dist', 'ui.cjs')

mkdirSync(path.dirname(outfile), { recursive: true })

const stubPlugin = {
  name: 'stub-modules-for-hermes',
  setup(build: any) {
    build.onResolve({ filter: /host-bootstrap$/ }, () => ({
      path: path.join(root, 'src', 'ui', 'client', 'host-bootstrap-stub.ts'),
    }))
    build.onResolve({ filter: /agent\/(core|tools|host|session|checkpoint|skills|decision|subagents$|prompts$|compact$)/ }, () => ({
      path: path.join(root, 'src', 'ui', 'client', 'empty-stub.ts'),
    }))
    build.onResolve({ filter: /agent\/store$/ }, () => ({
      path: path.join(root, 'src', 'ui', 'client', 'store-stub.ts'),
    }))
    build.onResolve({ filter: /in-process$/ }, () => ({
      path: path.join(root, 'src', 'ui', 'client', 'in-process-stub.ts'),
    }))
    build.onResolve({ filter: /^bun:ffi$/ }, () => ({
      path: path.join(root, 'src', 'ui', 'client', 'ffi-stub.ts'),
    }))
  },
}

const result = await Bun.build({
  entrypoints: [path.join(root, 'src', 'ui', 'main.tsx')],
  target: 'node',
  format: 'cjs',
  minify: false, // 由后续 TypeScript 统一降级与安全输出
  external: ['@gpuix/native', 'fsevents'],
  plugins: [stubPlugin],
  naming: 'ui.cjs',
  outdir: path.join(root, 'dist'),
})

if (!result.success) {
  console.error('[bundle-ui] 打包失败:')
  for (const log of result.logs) console.error(log)
  process.exit(1)
}

if (existsSync(outfile)) {
  console.log('[bundle-ui] 正在通过 TypeScript 对 Hermes 引擎语法进行兼容性降级 (ES5/ES6)...')
  const rawCode = readFileSync(outfile, 'utf8')
  const transpiled = ts.transpileModule(rawCode, {
    compilerOptions: {
      target: ts.ScriptTarget.ES5,
      module: ts.ModuleKind.CommonJS,
      downlevelIteration: true,
      importHelpers: false,
    },
  })
  writeFileSync(outfile, transpiled.outputText, 'utf8')

  const sizeKb = (statSync(outfile).size / 1024).toFixed(1)
  console.log(`[bundle-ui] 🎉 成功生成轻量 UI Bundle: dist/ui.cjs (${sizeKb} KB)`)
}
