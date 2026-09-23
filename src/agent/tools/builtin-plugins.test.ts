import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { BUILTIN_PLUGINS } from './builtin-plugins'
import { ExtensionLoader } from './loader'
import { defaultPromptManager } from '../prompts'
import { defaultSkillManager } from '../skills'
import { defaultToolRegistry } from './registry'
import { saveDisabledPlugins } from '../config'

describe('系统官方内置辅助 Coding 插件系统', () => {
  let workspace: string
  let homeDir: string
  let oldHome: string | undefined
  let loader: ExtensionLoader

  beforeEach(async () => {
    workspace = await mkdtemp(join(tmpdir(), 'ada-ws-builtin-plg-'))
    homeDir = await mkdtemp(join(tmpdir(), 'ada-home-builtin-plg-'))
    oldHome = process.env.A_DA_HOME
    process.env.A_DA_HOME = homeDir
    loader = new ExtensionLoader()
  })

  afterEach(async () => {
    if (oldHome !== undefined) {
      process.env.A_DA_HOME = oldHome
    } else {
      delete process.env.A_DA_HOME
    }
    await rm(workspace, { recursive: true, force: true })
    await rm(homeDir, { recursive: true, force: true })
    await saveDisabledPlugins([])
  })

  test('内置插件清单包含 4 大精选辅助插件且具备三位一体数据模型', () => {
    expect(BUILTIN_PLUGINS.length).toBeGreaterThanOrEqual(4)

    const ids = BUILTIN_PLUGINS.map((p) => p.id)
    expect(ids).toContain('git-tools')
    expect(ids).toContain('code-outline')
    expect(ids).toContain('project-inspector')
    expect(ids).toContain('test-runner')

    for (const plugin of BUILTIN_PLUGINS) {
      expect(plugin.name).toBeDefined()
      expect(plugin.tools.length).toBeGreaterThan(0)
      expect(plugin.skills?.length).toBeGreaterThan(0)
      expect(plugin.prompts?.length).toBeGreaterThan(0)
    }
  })

  test('code-outline 插件能准确提取源码中的类、接口与函数符号大纲', async () => {
    const outlinePlugin = BUILTIN_PLUGINS.find((p) => p.id === 'code-outline')!
    const rawTool = outlinePlugin.tools[0]!
    const getOutlineTool = typeof rawTool === 'function' ? rawTool(workspace) : rawTool

    const testFile = join(workspace, 'example.ts')
    const tsCode = `
export interface UserConfig {
  name: string
  age: number
}

export type ThemeMode = 'dark' | 'light'

export class Calculator {
  add(a: number, b: number): number {
    return a + b
  }
}

export async function processData(items: string[]): Promise<void> {
  // do something
}

export const helper = () => {}
`
    await writeFile(testFile, tsCode, 'utf8')

    const result = await getOutlineTool.execute(
      'call-1',
      { path: 'example.ts' }
    )

    expect(result.ok).toBe(true)
    expect(result.output).toContain('文件符号大纲: example.ts')
    expect(result.output).toContain('[interface] UserConfig')
    expect(result.output).toContain('[type] ThemeMode')
    expect(result.output).toContain('[class] Calculator')
    expect(result.output).toContain('[function] processData')
    expect(result.output).toContain('[function] helper')
  })

  test('project-inspector 插件能自动探测 package.json 的技术栈与 scripts 指令', async () => {
    const inspectorPlugin = BUILTIN_PLUGINS.find((p) => p.id === 'project-inspector')!
    const rawInspectTool = inspectorPlugin.tools[0]!
    const inspectTool = typeof rawInspectTool === 'function' ? rawInspectTool(workspace) : rawInspectTool

    const pkgJson = join(workspace, 'package.json')
    await writeFile(
      pkgJson,
      JSON.stringify({
        name: 'sample-app',
        version: '1.2.3',
        scripts: {
          build: 'tsc && vite build',
          test: 'bun test',
        },
        dependencies: {
          react: '^19.0.0',
        },
      }),
      'utf8'
    )

    const result = await inspectTool.execute('call-2', {})

    expect(result.ok).toBe(true)
    expect(result.output).toContain('sample-app')
    expect(result.output).toContain('1.2.3')
    expect(result.output).toContain('`build`: tsc && vite build')
    expect(result.output).toContain('`test`: bun test')
    expect(result.output).toContain('react')
  })

  test('ExtensionLoader 扫描内置插件并在 autoLoadExtensions 中自动注册工具', async () => {
    const plugins = await loader.scanPlugins(workspace)
    const builtinPlugins = plugins.filter((p) => p.scope === 'builtin')
    expect(builtinPlugins.length).toBeGreaterThanOrEqual(4)

    const gitTools = builtinPlugins.find((p) => p.id === 'builtin:git-tools')
    expect(gitTools).toBeDefined()
    expect(gitTools?.enabled).toBe(true)
    expect(gitTools?.tools.some((t) => t.name === 'git_status')).toBe(true)

    // 执行自动加载
    const loaded = await loader.autoLoadExtensions(workspace)
    expect(loaded).toContain('git_status')
    expect(loaded).toContain('get_outline')
    expect(loaded).toContain('inspect_project')
    expect(loaded).toContain('run_test_focused')

    // 禁用 git-tools
    await loader.togglePlugin('builtin:git-tools', false, workspace)

    const reloadedPlugins = await loader.scanPlugins(workspace)
    const disabledGit = reloadedPlugins.find((p) => p.id === 'builtin:git-tools')
    expect(disabledGit?.enabled).toBe(false)

    // 重新自动加载，已禁用的工具不再注册
    const afterDisabledLoaded = await loader.autoLoadExtensions(workspace)
    expect(afterDisabledLoaded).not.toContain('git_status')
    expect(afterDisabledLoaded).toContain('get_outline')
  })

  test('defaultPromptManager 和 defaultSkillManager 能够成功归纳内置插件提供的提示词和技能', async () => {
    const allPrompts = await defaultPromptManager.scanPrompts(workspace)
    const outlinePrompt = allPrompts.find((p) => p.name === 'outline')
    expect(outlinePrompt).toBeDefined()
    expect(outlinePrompt?.pluginName).toContain('code-outline')
    expect(outlinePrompt?.argumentHint).toBe('<file-path>')

    const allSkills = await defaultSkillManager.scanSkills(workspace)
    const gitSkill = allSkills.find((s) => s.name === 'git-workflow')
    expect(gitSkill).toBeDefined()
    expect(gitSkill?.pluginName).toContain('git-tools')
  })
})
