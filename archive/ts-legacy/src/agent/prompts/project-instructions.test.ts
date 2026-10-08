/**
 * 项目说明注入（AGENTS.md / CLAUDE.md）与 /init 内置提示词的测试。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { defaultPromptManager } from './manager'
import { BUILTIN_PROMPTS } from './builtins'

let ws = ''

beforeAll(async () => {
  ws = await mkdtemp(join(tmpdir(), 'a-da-agents-'))
})

afterAll(async () => {
  if (ws) await rm(ws, { recursive: true, force: true })
})

describe('readProjectInstructions', () => {
  test('AGENTS.md 优先于 CLAUDE.md', async () => {
    await writeFile(join(ws, 'AGENTS.md'), '# agents 规则\n用 bun test 跑测试。')
    await writeFile(join(ws, 'CLAUDE.md'), '# claude 规则\n')
    const found = await defaultPromptManager.readProjectInstructions(ws)
    expect(found).not.toBeNull()
    expect(found!.file).toBe('AGENTS.md')
    expect(found!.content).toContain('bun test')
  })

  test('没有 AGENTS.md 时回退 CLAUDE.md；都没有返回 null', async () => {
    await rm(join(ws, 'AGENTS.md'), { force: true })
    const found = await defaultPromptManager.readProjectInstructions(ws)
    expect(found!.file).toBe('CLAUDE.md')

    await rm(join(ws, 'CLAUDE.md'), { force: true })
    expect(await defaultPromptManager.readProjectInstructions(ws)).toBeNull()
  })

  test('超长文件被截断并标注', async () => {
    await writeFile(join(ws, 'AGENTS.md'), `x`.repeat(40_000))
    const found = await defaultPromptManager.readProjectInstructions(ws)
    expect(found!.content.length).toBeLessThan(41_000)
    expect(found!.content).toContain('已截断')
    await rm(join(ws, 'AGENTS.md'), { force: true })
  })
})

describe('getCompositeSystemPrompt 注入项目说明', () => {
  test('合成的系统提示词包含项目说明段', async () => {
    await writeFile(join(ws, 'AGENTS.md'), '本项目用中文提交信息。')
    const composite = await defaultPromptManager.getCompositeSystemPrompt(ws, 'code')
    expect(composite).toContain('【项目说明（来自 AGENTS.md）】')
    expect(composite).toContain('本项目用中文提交信息。')
    expect(composite).toContain('【协作模式：Code 编码模式')
    // 移除后不再注入
    await rm(join(ws, 'AGENTS.md'), { force: true })
    const without = await defaultPromptManager.getCompositeSystemPrompt(ws, 'code')
    expect(without).not.toContain('本项目用中文提交信息。')
  })
})

describe('/init 内置提示词', () => {
  test('存在于内置清单，默认启用且非系统段', () => {
    const init = BUILTIN_PROMPTS.find((p) => p.id === 'builtin-init-project-instructions')
    expect(init).toBeDefined()
    expect(init!.enabled).toBe(true)
    expect(init!.isSystem).toBe(false)
    expect(init!.content).toContain('AGENTS.md')

    // 会出现在扫描结果里（斜杠菜单可见）
  })

  test('scanPrompts 能列出它，斜杠菜单可发现', async () => {
    const prompts = await defaultPromptManager.scanPrompts(ws)
    const init = prompts.find((p) => p.id === 'builtin-init-project-instructions')
    expect(init).toBeDefined()
    expect(init!.enabled).toBe(true)
  })
})
