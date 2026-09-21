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
})
