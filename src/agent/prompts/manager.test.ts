import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { PromptManager } from './manager'

describe('PromptManager 提示词管理核心逻辑', () => {
  let workspace: string
  let homeDir: string
  let oldHome: string | undefined
  let manager: PromptManager

  beforeEach(async () => {
    workspace = await mkdtemp(join(tmpdir(), 'ada-ws-prompt-'))
    homeDir = await mkdtemp(join(tmpdir(), 'ada-home-prompt-'))
    oldHome = process.env.A_DA_HOME
    process.env.A_DA_HOME = homeDir
    manager = new PromptManager()
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

  test('初始扫描应包含默认内置提示词且中文编码规范默认启用', async () => {
    const list = await manager.scanPrompts(workspace)
    expect(list.length).toBeGreaterThanOrEqual(5)

    const chineseStandard = list.find((p) => p.id === 'builtin-chinese-coding-standards')
    expect(chineseStandard).toBeDefined()
    expect(chineseStandard?.enabled).toBe(true)
    expect(chineseStandard?.isSystem).toBe(true)
  })

  test('支持创建工作区提示词并成功写入 Markdown 文件', async () => {
    const created = await manager.createPrompt(workspace, {
      name: '项目特定规范',
      description: '本项目自定义代码与测试规范',
      content: '禁止使用 any 类型，所有异步函数必须包含 try-catch。',
      scope: 'workspace',
      isSystem: true,
      enabled: true,
    })

    expect(created.name).toBe('项目特定规范')
    expect(created.scope).toBe('workspace')
    expect(created.isSystem).toBe(true)
    expect(created.enabled).toBe(true)
    expect(created.filePath).toBeDefined()

    // 重新扫描验证持久化
    const all = await manager.scanPrompts(workspace)
    const found = all.find((p) => p.id === created.id)
    expect(found).toBeDefined()
    expect(found?.content).toContain('禁止使用 any 类型')
  })

  test('支持创建全局提示词', async () => {
    const created = await manager.createPrompt(workspace, {
      name: '全局架构规范',
      description: '所有项目通用的架构设计指引',
      content: '保持三层架构分层，核心业务领域与协议解耦。',
      scope: 'global',
      isSystem: false,
      enabled: true,
    })

    expect(created.scope).toBe('global')
    expect(created.filePath?.startsWith(homeDir)).toBe(true)

    const all = await manager.scanPrompts(workspace)
    expect(all.some((p) => p.id === created.id)).toBe(true)
  })

  test('支持切换启停状态并持久化', async () => {
    // 切换内置提示词启停
    const ok1 = await manager.togglePrompt('builtin-chinese-coding-standards', false, workspace)
    expect(ok1).toBe(true)

    let all = await manager.scanPrompts(workspace)
    let standard = all.find((p) => p.id === 'builtin-chinese-coding-standards')
    expect(standard?.enabled).toBe(false)

    // 创建自定义提示词并切换状态
    const item = await manager.createPrompt(workspace, {
      name: '临时规则',
      content: '规则正文',
      scope: 'workspace',
      enabled: true,
    })

    const ok2 = await manager.togglePrompt(item.id, false, workspace)
    expect(ok2).toBe(true)

    all = await manager.scanPrompts(workspace)
    const reloaded = all.find((p) => p.id === item.id)
    expect(reloaded?.enabled).toBe(false)
  })

  test('支持更新已有提示词并保存到文件', async () => {
    const item = await manager.createPrompt(workspace, {
      name: '原名称',
      description: '原描述',
      content: '原正文内容',
      scope: 'workspace',
      isSystem: false,
    })

    item.name = '已更新名称'
    item.description = '已更新描述'
    item.content = '已修改的正文内容'
    item.isSystem = true

    const success = await manager.updatePrompt(item)
    expect(success).toBe(true)

    const all = await manager.scanPrompts(workspace)
    const updated = all.find((p) => p.id === item.id)
    expect(updated?.name).toBe('已更新名称')
    expect(updated?.content).toBe('已修改的正文内容')
    expect(updated?.isSystem).toBe(true)
  })

  test('支持删除提示词文件', async () => {
    const item = await manager.createPrompt(workspace, {
      name: '待删除提示词',
      content: '稍后删除',
      scope: 'workspace',
    })

    expect(item.filePath).toBeDefined()
    const deleted = await manager.deletePrompt(item.filePath!)
    expect(deleted).toBe(true)

    const all = await manager.scanPrompts(workspace)
    expect(all.some((p) => p.id === item.id)).toBe(false)
  })

  test('getCompositeSystemPrompt 正确合成启用的系统提示词', async () => {
    // 默认内置中文编码规范已启用
    const initialPrompt = await manager.getCompositeSystemPrompt(workspace)
    expect(initialPrompt).toContain('中文专业编码规范')

    // 新增一个启用的系统提示词
    await manager.createPrompt(workspace, {
      name: '自定义安全规范',
      content: '必须对所有来自外网的 URL 参数做白名单校验。',
      scope: 'workspace',
      isSystem: true,
      enabled: true,
    })

    // 新增一个未启用的系统提示词（不应被合成）
    await manager.createPrompt(workspace, {
      name: '未启用规范',
      content: '这条不应出现',
      scope: 'workspace',
      isSystem: true,
      enabled: false,
    })

    const composite = await manager.getCompositeSystemPrompt(workspace)
    expect(composite).toContain('中文专业编码规范')
    expect(composite).toContain('自定义安全规范')
    expect(composite).toContain('必须对所有来自外网的 URL 参数做白名单校验')
    expect(composite).not.toContain('这条不应出现')
  })

  test('支持扫描插件包目录中的提示词并解析 argument-hint', async () => {
    const { mkdir, writeFile } = await import('node:fs/promises')
    const pluginPromptsDir = join(workspace, '.ada', 'extensions', 'git-pack', 'prompts')
    await mkdir(pluginPromptsDir, { recursive: true })

    const promptFile = join(pluginPromptsDir, 'pr-review.md')
    const mdContent = `---
name: "代码评审"
description: "自动化 PR 审查提示词"
argument-hint: "<pr-url> [branch]"
isSystem: false
enabled: true
---

请对 \${1:-当前分支} 与 \${2:-main} 进行详细审查。
`
    await writeFile(promptFile, mdContent, 'utf8')

    const list = await manager.scanPrompts(workspace)
    const found = list.find((p) => p.name === '代码评审')
    expect(found).toBeDefined()
    expect(found?.scope).toBe('plugin')
    expect(found?.pluginName).toBe('git-pack')
    expect(found?.pluginId).toBe('workspace:git-pack')
    expect(found?.argumentHint).toBe('<pr-url> [branch]')
    expect(found?.enabled).toBe(true)

    // 测试 findPrompt 查找已启用的提示词
    const matched = await manager.findPrompt('代码评审', workspace)
    expect(matched).toBeDefined()
    expect(matched?.id).toBe(found?.id)

    // 通过 slug 文件名查找
    const matchedSlug = await manager.findPrompt('pr-review', workspace)
    expect(matchedSlug).toBeDefined()
    expect(matchedSlug?.name).toBe('代码评审')
  })

  test('插件提示词在所属插件停用时自动随之联动停用', async () => {
    const { mkdir, writeFile } = await import('node:fs/promises')
    const pluginPromptsDir = join(workspace, '.ada', 'extensions', 'helper-pack', 'prompts')
    await mkdir(pluginPromptsDir, { recursive: true })
    await writeFile(
      join(pluginPromptsDir, 'helper.md'),
      '---\nname: "辅助助手"\ndescription: "帮助"\n---\n正文',
      'utf8'
    )

    // 初始状态应为启用
    let list = await manager.scanPrompts(workspace)
    let item = list.find((p) => p.name === '辅助助手')
    expect(item?.enabled).toBe(true)

    // 禁用插件
    const { saveDisabledPlugins } = await import('../config')
    await saveDisabledPlugins(['workspace:helper-pack'])

    list = await manager.scanPrompts(workspace)
    item = list.find((p) => p.name === '辅助助手')
    expect(item?.enabled).toBe(false)

    // 查找提示词时不应匹配已停用的提示词
    const matchDisabled = await manager.findPrompt('辅助助手', workspace)
    expect(matchDisabled).toBeUndefined()
  })

  test('findPrompt 严格遵循作用域优先级：workspace > global > plugin > builtin', async () => {
    const { mkdir, writeFile } = await import('node:fs/promises')
    // 1. 在插件中创建同名提示词 "review"
    const pluginDir = join(workspace, '.ada', 'extensions', 'pack', 'prompts')
    await mkdir(pluginDir, { recursive: true })
    await writeFile(
      join(pluginDir, 'review.md'),
      '---\nname: "review"\ndescription: "插件版本"\n---\n来自插件',
      'utf8'
    )

    let prompt = await manager.findPrompt('review', workspace)
    expect(prompt?.scope).toBe('plugin')
    expect(prompt?.content).toContain('来自插件')

    // 2. 在全局创建同名提示词 "review"
    await manager.createPrompt(workspace, {
      name: 'review',
      content: '来自全局',
      scope: 'global',
    })

    prompt = await manager.findPrompt('review', workspace)
    expect(prompt?.scope).toBe('global')
    expect(prompt?.content).toContain('来自全局')

    // 3. 在工作区创建同名提示词 "review"
    await manager.createPrompt(workspace, {
      name: 'review',
      content: '来自工作区',
      scope: 'workspace',
    })

    prompt = await manager.findPrompt('review', workspace)
    expect(prompt?.scope).toBe('workspace')
    expect(prompt?.content).toContain('来自工作区')
  })
})
