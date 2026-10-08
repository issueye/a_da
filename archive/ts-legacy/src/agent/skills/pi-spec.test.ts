import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { escapeXml, SkillManager } from './manager'
import { parseSkillMarkdown } from './parser'

describe('pi Agent Skills 标准规范测试', () => {
  let testDir: string
  let manager: SkillManager

  beforeEach(async () => {
    testDir = await mkdtemp(join(tmpdir(), 'ada-pi-skills-test-'))
    manager = new SkillManager()
  })

  afterEach(async () => {
    await rm(testDir, { recursive: true, force: true }).catch(() => {})
  })

  test('escapeXml 能够正确转义特殊字符', () => {
    expect(escapeXml('<script>alert("hello & \'world\'")</script>')).toBe(
      '&lt;script&gt;alert(&quot;hello &amp; &apos;world&apos;&quot;)&lt;/script&gt;'
    )
  })

  test('单文件 .md 技能解析：自动以文件名推导名称并支持 pi Agent Skills 扩展字段', async () => {
    const rawContent = `---
description: 这是一个轻量级单文件格式的 Git 提交消息生成技能
disable-model-invocation: true
allowed-tools: [run_command, read_file]
compatibility: 需要 Git CLI 环境
---

# Git Commit 规范
按照 Conventional Commits 格式生成。
`
    const filePath = join(testDir, 'git-quick-commit.md')
    const parsed = parseSkillMarkdown(rawContent, filePath)

    expect(parsed.metadata.name).toBe('git-quick-commit')
    expect(parsed.metadata.description).toBe('这是一个轻量级单文件格式的 Git 提交消息生成技能')
    expect(parsed.metadata.disableModelInvocation).toBe(true)
    expect(parsed.metadata.allowedTools).toEqual(['run_command', 'read_file'])
    expect(parsed.metadata.compatibility).toBe('需要 Git CLI 环境')
  })

  test('双模扫描：支持单文件技能与目录技能并存，并验证 XML 提示词生成与 disableModelInvocation 过滤', async () => {
    const skillsDir = join(testDir, '.ada', 'skills')
    await mkdir(skillsDir, { recursive: true })

    // 1. 创建单文件技能 (普通自动感知)
    const singleSkillPath = join(skillsDir, 'api-tester.md')
    await writeFile(
      singleSkillPath,
      `---
name: api-tester
description: 接口自动化测试与契约校验
allowed-tools: read_url_content, run_command
---
## 执行步骤
校验 HTTP 状态码与响应体。
`,
      'utf8'
    )

    // 2. 创建单文件技能 (disable-model-invocation: true, 仅显式指令唤醒)
    const hiddenSkillPath = join(skillsDir, 'secret-deploy.md')
    await writeFile(
      hiddenSkillPath,
      `---
name: secret-deploy
description: 内部私有生产环境发布规范
disable-model-invocation: true
---
## 发布步骤
严格审计环境配置。
`,
      'utf8'
    )

    // 3. 扫描发现技能
    const scanned = await manager.scanSkills(testDir)
    const apiTester = scanned.find((s) => s.name === 'api-tester')
    const secretDeploy = scanned.find((s) => s.name === 'secret-deploy')

    expect(apiTester).toBeDefined()
    expect(apiTester?.isFileSkill).toBe(true)
    expect(apiTester?.allowedTools).toEqual(['read_url_content', 'run_command'])
    expect(apiTester?.disableModelInvocation).toBeFalsy()

    expect(secretDeploy).toBeDefined()
    expect(secretDeploy?.isFileSkill).toBe(true)
    expect(secretDeploy?.disableModelInvocation).toBe(true)

    // 4. 生成系统提示词 XML 检查
    const promptCtx = await manager.buildSkillsPrompt(testDir)
    expect(promptCtx.prompt).toContain('<available_skills>')
    expect(promptCtx.prompt).toContain('<name>api-tester</name>')
    expect(promptCtx.prompt).toContain('<allowed_tools>read_url_content, run_command</allowed_tools>')

    // 重点验证：disableModelInvocation === true 的技能被隐藏在系统提示词中
    expect(promptCtx.prompt).not.toContain('<name>secret-deploy</name>')
    expect(promptCtx.prompt).not.toContain('内部私有生产环境发布规范')

    // 5. 单文件技能删除测试：仅删除单文件自身，不破坏父目录
    await manager.deleteSkill(apiTester!.id, testDir)
    const afterDelete = await manager.scanSkills(testDir)
    expect(afterDelete.find((s) => s.name === 'api-tester')).toBeUndefined()
    expect(afterDelete.find((s) => s.name === 'secret-deploy')).toBeDefined()
  })
})
