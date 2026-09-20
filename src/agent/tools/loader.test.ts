import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { defaultExtensionLoader, ExtensionLoader } from './loader'
import { defaultToolRegistry, runTool } from '../tools'

let tempWorkspace = ''

beforeAll(async () => {
  tempWorkspace = await mkdtemp(join(tmpdir(), 'a-da-extension-test-'))
  const extDir = join(tempWorkspace, '.ada', 'extensions')
  await mkdir(extDir, { recursive: true })

  // 写入一个自定义 TypeScript 扩展工具
  const toolTs = `
export default function(workspace) {
  return {
    name: 'custom_math',
    description: '自定义数学计算工具',
    parameters: {
      type: 'object',
      properties: {
        a: { type: 'number' },
        b: { type: 'number' }
      }
    },
    async execute(callId, args) {
      const sum = (args.a || 0) + (args.b || 0)
      return {
        output: "计算结果: " + sum,
        ok: true,
        details: { sum }
      }
    }
  }
}
`
  await writeFile(join(extDir, 'math.ts'), toolTs, 'utf-8')
})

afterAll(async () => {
  if (tempWorkspace) await rm(tempWorkspace, { recursive: true, force: true })
})

describe('ExtensionLoader (jiti)', () => {
  test('dynamically loads typescript tools from .ada/extensions', async () => {
    const loader = new ExtensionLoader()
    const loaded = await loader.autoLoadExtensions(tempWorkspace)

    expect(loaded).toContain('custom_math')

    // 验证可以通过 runTool 调用该扩展工具
    const res = await runTool(tempWorkspace, {
      name: 'custom_math',
      args: { a: 12, b: 30 },
    })

    expect(res.ok).toBe(true)
    expect(res.output).toContain('计算结果: 42')
  })

  test('extension receives ExtensionContext and can emit trace points', async () => {
    const extDir = join(tempWorkspace, '.ada', 'extensions')
    const tracePlugin = `
export default function(api) {
  api.trace('插件已成功初始化并接入点位');
  api.registerTool({
    name: 'plugin_tool',
    description: '插件工具',
    parameters: { type: 'object' },
    async execute() {
      api.trace('插件工具正在执行');
      return { output: 'ok', ok: true };
    }
  });
}
`
    await writeFile(join(extDir, 'plugin.ts'), tracePlugin, 'utf-8')

    const traceLogs: string[] = []
    const loader = new ExtensionLoader()
    loader.bindHost((msg) => traceLogs.push(msg))

    const loaded = await loader.autoLoadExtensions(tempWorkspace)
    expect(loaded).toContain('plugin_tool')
    expect(traceLogs.some((log) => log.includes('插件已成功初始化并接入点位'))).toBe(true)

    // 执行插件工具并验证内部打点
    await runTool(tempWorkspace, { name: 'plugin_tool', args: {} })
    expect(traceLogs.some((log) => log.includes('插件工具正在执行'))).toBe(true)
  })
})
