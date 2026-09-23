import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { ExtensionLoader } from './loader'
import { saveDisabledPlugins } from '../config'

describe('ExtensionLoader 复合插件包提示词 (Prompts) 归纳与生命周期', () => {
  let workspace: string
  let homeDir: string
  let oldHome: string | undefined
  let loader: ExtensionLoader

  beforeEach(async () => {
    workspace = await mkdtemp(join(tmpdir(), 'ada-ws-pkg-prompt-'))
    homeDir = await mkdtemp(join(tmpdir(), 'ada-home-pkg-prompt-'))
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
  })

  test('复合插件包目录包含 prompts/*.md 时被正确扫描并归纳到 PluginItem.prompts', async () => {
    const pkgDir = join(workspace, '.ada', 'extensions', 'dev-toolkit')
    const promptsDir = join(pkgDir, 'prompts')
    await mkdir(promptsDir, { recursive: true })

    // 在复合包内创建一个提示词文件
    const promptFile = join(promptsDir, 'commit.md')
    const content = `---
name: "git提交规范"
description: "根据当前 git diff 生成符合 Conventional Commits 规范的提交信息"
argument-hint: "[scope]"
isSystem: false
enabled: true
---

请检查暂存区改动并以 \${1:-chore} 作为默认范围生成提交信息。
`
    await writeFile(promptFile, content, 'utf8')

    // 扫描插件
    const plugins = await loader.scanPlugins(workspace)
    expect(plugins.length).toBe(1)

    const plugin = plugins[0]
    expect(plugin.name).toBe('dev-toolkit')
    expect(plugin.isPackage).toBe(true)
    expect(plugin.prompts).toBeDefined()
    expect(plugin.prompts.length).toBe(1)

    const promptItem = plugin.prompts[0]
    expect(promptItem.name).toBe('git提交规范')
    expect(promptItem.description).toContain('Conventional Commits')
    expect(promptItem.argumentHint).toBe('[scope]')
    expect(promptItem.scope).toBe('plugin')
    expect(promptItem.pluginName).toBe('dev-toolkit')
    expect(promptItem.pluginId).toBe('workspace:dev-toolkit')
    expect(promptItem.enabled).toBe(true)
  })

  test('停用复合插件包时，其内建提示词随之被标记为停用', async () => {
    const pkgDir = join(workspace, '.ada', 'extensions', 'audit-pack')
    const promptsDir = join(pkgDir, 'prompts')
    await mkdir(promptsDir, { recursive: true })

    await writeFile(
      join(promptsDir, 'audit.md'),
      '---\nname: "安全审计"\ndescription: "代码审计"\n---\n执行安全扫描',
      'utf8'
    )

    // 初始状态
    let plugins = await loader.scanPlugins(workspace)
    expect(plugins[0].enabled).toBe(true)
    expect(plugins[0].prompts[0].enabled).toBe(true)

    // 禁用插件
    await saveDisabledPlugins(['workspace:audit-pack'])

    plugins = await loader.scanPlugins(workspace)
    expect(plugins[0].enabled).toBe(false)
    expect(plugins[0].prompts[0].enabled).toBe(false)

    // 重新启用插件
    await saveDisabledPlugins([])
    plugins = await loader.scanPlugins(workspace)
    expect(plugins[0].enabled).toBe(true)
    expect(plugins[0].prompts[0].enabled).toBe(true)
  })
})
