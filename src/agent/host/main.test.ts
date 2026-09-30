/**
 * 主机角色的 argv 与就绪行（M3-4 / 协议 §1.8）。
 *
 * 纯逻辑：写与读必须成对——`hostEntryArgs` 造参数、`parseHostArgs` 读参数，
 * 一侧改了另一侧没改，症状是"主机起来了但某个开关没生效"这种难查的静默失效。
 */

import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { hostEntryArgs, parseHostArgs, parseReadyLine, readyLine } from './main'

describe('主机 argv：写与读成对', () => {
  test('往返一致（含可选参数）', () => {
    const options = { port: 0, token: 'abc123', parentPid: 4242 }
    expect(parseHostArgs(hostEntryArgs(options))).toEqual(options)

    const minimal = { port: 51234, token: 'xyz' }
    expect(parseHostArgs(hostEntryArgs(minimal))).toEqual(minimal)
  })

  test('没有令牌就不给起（本机回环也不能无令牌开放）', () => {
    expect(() => parseHostArgs(['--host', '--port', '0'])).toThrow(/--token/)
  })

  test('端口与父进程 pid 的取值不合法时明说，而不是当成默认值', () => {
    expect(() => parseHostArgs(['--host', '--token', 't', '--port', 'abc'])).toThrow(/--port/)
    expect(() => parseHostArgs(['--host', '--token', 't', '--port', '70000'])).toThrow(/--port/)
    expect(() => parseHostArgs(['--host', '--token', 't', '--parent-pid', '-1'])).toThrow(/--parent-pid/)
  })

  test('不认识的参数直接报错（免得静默忽略掉一个拼错的开关）', () => {
    expect(() => parseHostArgs(['--host', '--token', 't', '--pora', '1'])).toThrow(/不认识/)
  })
})

describe('就绪行', () => {
  test('能从混杂输出里扫出来，坏行返回 null 而不是猜', () => {
    const line = readyLine(51234)
    const parsed = parseReadyLine(`一些无关输出\n${line}\n`)
    expect(parsed?.port).toBe(51234)
    expect(parsed?.pid).toBe(process.pid)

    expect(parseReadyLine('随便一行')).toBeNull()
    expect(parseReadyLine('A_DA_HOST_READY {不是 json}')).toBeNull()
    expect(parseReadyLine('A_DA_HOST_READY {"ready":false,"port":1}')).toBeNull()
  })
})

/**
 * 协议 §1.8 第 4 条：**host 分支绝不能碰到渲染层**。
 *
 * 这条只能靠结构来保证：ESM 的静态 import 会先于任何分支判断执行，所以 `app.tsx` 里
 * 只要留下一句静态的 `import './src/platform/init'`，`--host` 进程就会连原生 addon
 * 一起加载（最坏情况是多出一个空窗口）。用测试盯住这个形状别被改回去。
 */
describe('入口分流（协议 §1.8 第 4 条）', () => {
  const repoRoot = join(import.meta.dir, '..', '..', '..')
  const entry = readFileSync(join(repoRoot, 'app.tsx'), 'utf8')
  const uiMain = readFileSync(join(repoRoot, 'src', 'ui', 'main.tsx'), 'utf8')

  test('app.tsx 按 argv 分流，两个分支都用动态 import', () => {
    expect(entry).toContain("process.argv.includes('--host')")
    expect(entry).toContain("await import('./src/agent/host/main')")
    expect(entry).toContain("await import('./src/ui/main')")
  })

  test('app.tsx 不静态 import 渲染层（否则 --host 会白加载原生 addon）', () => {
    const staticImports = [...entry.matchAll(/^\s*import\s[^;]*from\s+'([^']+)'/gm)].map((m) => m[1]!)
    expect(staticImports.filter((spec) => spec.includes('platform/init'))).toEqual([])
    expect(staticImports.filter((spec) => spec.includes('ui/'))).toEqual([])
    expect(staticImports.filter((spec) => spec.includes('@gpuix/'))).toEqual([])
  })

  test('渲染层的启动顺序没在搬家中丢掉：平台引导仍然最优先', () => {
    // 搬进 src/ui/main.tsx 之后，"平台引导在最前"这条不能丢：
    // 第一条 import 语句必须是它（注释里出现"import"这个词不算）
    const firstImportStatement = uiMain.match(/^\s*import\s[^\n]*/m)?.[0] ?? ''
    expect(firstImportStatement).toContain('platform/init')
    // 界面确实被挂载（参数怎么写不管，M3-6 起会带上 client）
    expect(uiMain).toContain('render(<AgentWindow')
  })
})
