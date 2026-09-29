/**
 * ponytail 插件的行为验收。
 *
 * 这个插件不提供工具，全部价值都在「技能正文能否被读到」「斜杠命令能否展开」
 * 「默认档位是否真的注入」——这三件事都能"看起来装上了，其实没生效"，所以逐条断言副作用：
 *
 * 1. **技能 frontmatter 必须是单行 description**：本仓库的解析器是手写的简化版，
 *    不支持 YAML 的 `>` / `|` 块标量，而**上游 SKILL.md 用的正是 `>`**。照抄上游写法，
 *    解析出来会是什么？描述变成 `'>'`，技能列表里那一条就成了空话，而且不报错。
 * 2. **斜杠命令展开后不许残留占位符**：模板里写了不支持的占位语法（例如 `${1:x}`——
 *    本仓库只认 `$1`、`${1:-默认值}`、`${@:N}`）就会把 `${1:x}` 原文发给模型。
 * 3. **默认档位必须是不注入**：内置插件是预装给所有人的，默认值等于替所有人改系统提示词。
 *    所以默认那条断言的是"什么都没发生"，配了档位才断言注入内容与档位一致。
 */

import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { cleanupTempDir } from '../../../../scripts/test-preload'
import { readPluginCapabilities } from '../../config'
import type { BeforeSystemPromptContext } from '../../core/events'
import { composePluginHooks } from '../../plugins/hook-runtime'
import { findLoadedPlugin } from '../../plugins/registry'
import { defaultPromptManager } from '../../prompts'
import { expandPromptTemplate } from '../../prompts/template'
import { defaultSkillManager } from '../../skills'
import { parseSkillMarkdown } from '../../skills/parser'
import { defaultExtensionLoader } from '../loader'
import { BUILTIN_PLUGINS } from './index'
import {
  createPonytailHooks,
  parsePonytailMode,
  ponytailPlugin,
  readPonytailConfig,
  PONYTAIL_PLUGIN_ID,
} from './ponytail'

let home = ''
let workspace = ''
let oldHome: string | undefined
let oldConfig: string | undefined

beforeEach(async () => {
  home = await mkdtemp(join(tmpdir(), 'ada-ponytail-home-'))
  workspace = await mkdtemp(join(tmpdir(), 'ada-ponytail-ws-'))
  oldHome = process.env.A_DA_HOME
  oldConfig = process.env.A_DA_CONFIG
  process.env.A_DA_HOME = home
  // 指到自己的临时配置：别的测试文件会临时改 A_DA_CONFIG，不锁住就会读到别人的文件
  process.env.A_DA_CONFIG = join(home, 'config.json')
})

afterEach(async () => {
  if (oldHome === undefined) delete process.env.A_DA_HOME
  else process.env.A_DA_HOME = oldHome
  if (oldConfig === undefined) delete process.env.A_DA_CONFIG
  else process.env.A_DA_CONFIG = oldConfig
  delete process.env.A_DA_PLUGIN_PONYTAIL_DEFAULT_MODE
  await cleanupTempDir(home)
  await cleanupTempDir(workspace)
})

/** 写一份插件配置到 `pluginConfig['ponytail']`。 */
async function configure(patch: Record<string, unknown>): Promise<void> {
  await writeFile(
    join(home, 'config.json'),
    JSON.stringify({ pluginConfig: { [PONYTAIL_PLUGIN_ID]: patch } }),
    'utf8',
  )
}

/** 造一个系统提示词点位上下文，trace 进数组以便断言"说出来了"。 */
function promptContext(): BeforeSystemPromptContext & { traces: string[] } {
  const traces: string[] = []
  return {
    kind: 'main',
    threadId: 'thread-1',
    workspace,
    systemPrompt: '原有的系统提示词',
    mode: 'code',
    trace: (message: string) => traces.push(message),
    traces,
  }
}

describe('ponytail 的插件登记与规格', () => {
  test('已登记进 BUILTIN_PLUGINS（没登记就等于没装）', () => {
    const registered = BUILTIN_PLUGINS.find((plugin) => plugin.id === PONYTAIL_PLUGIN_ID)
    expect(registered).toBe(ponytailPlugin)
  })

  test('纯纪律插件不提供工具，且技能与斜杠命令一一同名', () => {
    // 空数组而非省略：契约要求 tools 必填，好让加载器免去无谓的可空判断
    expect(ponytailPlugin.tools).toHaveLength(0)

    const skills = (ponytailPlugin.skills ?? []).map((skill) => skill.name).sort()
    const prompts = (ponytailPlugin.prompts ?? []).map((prompt) => prompt.name).sort()
    expect(skills).toEqual([
      'ponytail',
      'ponytail-audit',
      'ponytail-debt',
      'ponytail-gain',
      'ponytail-help',
      'ponytail-review',
    ])
    // 上游的六个斜杠命令与六个技能同名：改名只会改一处，这条把它钉住
    expect(prompts).toEqual(skills)
  })
})

describe('ponytail 技能正文：能被解析器读成描述，而不是一个 ">"', () => {
  test('每个技能的 frontmatter 都能解析出与声明一致的单行描述', () => {
    for (const skill of ponytailPlugin.skills ?? []) {
      const parsed = parseSkillMarkdown(skill.content, `(builtin):${PONYTAIL_PLUGIN_ID}/${skill.name}`)

      expect({ skill: skill.name, name: parsed.metadata.name }).toEqual({
        skill: skill.name,
        name: skill.name,
      })
      expect(parsed.metadata.description).toBe(skill.description)
      expect(parsed.metadata.description).not.toBe('>')
      expect(parsed.metadata.description).not.toContain('\n')
      expect(parsed.metadata.description.length).toBeGreaterThan(10)
      // 模型按 whenToUse 判断要不要加载，缺了它自动触发就只剩描述可看
      expect((parsed.metadata.whenToUse ?? '').length).toBeGreaterThan(10)
      expect(parsed.hasFrontmatter).toBe(true)
      expect(parsed.body.length).toBeGreaterThan(100)
    }
  })

  test('技能库里能查到这六个技能，且归属 ponytail 插件', async () => {
    const skills = await defaultSkillManager.scanSkills(workspace)
    for (const expected of ['ponytail', 'ponytail-review', 'ponytail-audit', 'ponytail-debt', 'ponytail-gain', 'ponytail-help']) {
      const found = skills.find((skill) => skill.name === expected)
      expect({ name: expected, found: Boolean(found) }).toEqual({ name: expected, found: true })
      expect(found?.pluginId).toBe(`builtin:${PONYTAIL_PLUGIN_ID}`)
      expect(found?.scope).toBe('plugin')
      expect(found?.enabled).toBe(true)
    }
  })
})

describe('ponytail 斜杠命令：六个都展开得动，且不残留占位符', () => {
  test('命令在提示词清单里可见可用', async () => {
    const prompts = await defaultPromptManager.scanPrompts(workspace)
    const mine = prompts.filter((prompt) => prompt.pluginId === `builtin:${PONYTAIL_PLUGIN_ID}`)
    expect(mine.map((prompt) => prompt.name).sort()).toEqual([
      'ponytail',
      'ponytail-audit',
      'ponytail-debt',
      'ponytail-gain',
      'ponytail-help',
      'ponytail-review',
    ])
    for (const prompt of mine) expect(prompt.enabled).toBe(true)
  })

  test('展开后的正文没有残留占位符，参数落在该在的地方', async () => {
    const prompts = await defaultPromptManager.scanPrompts(workspace)
    const cases: Array<{ input: string; contains: string[] }> = [
      // 档位是第 1 个参数，任务文本从第 2 个参数起
      { input: '/ponytail ultra 给登录加个缓存', contains: ['ultra', '给登录加个缓存'] },
      // 不给档位时用默认值 full，而不是把 ${1:-full} 发给模型
      { input: '/ponytail 给登录加个缓存', contains: ['full', '给登录加个缓存'] },
      { input: '/ponytail-review', contains: ['net: -<N> lines possible.'] },
      { input: '/ponytail-audit src/agent', contains: ['src/agent', 'Lean already. Ship.'] },
      { input: '/ponytail-debt', contains: ['ponytail:', 'No ponytail: debt. Clean ledger.'] },
      { input: '/ponytail-gain', contains: ['benchmark median', '3–6× faster'] },
      { input: '/ponytail-help', contains: ['默认档位', 'A_DA_PLUGIN_PONYTAIL_DEFAULT_MODE'] },
    ]

    for (const item of cases) {
      const expanded = expandPromptTemplate(item.input, prompts)
      expect({ input: item.input, expanded: expanded !== item.input }).toEqual({
        input: item.input,
        expanded: true,
      })
      expect(expanded).not.toContain('${')
      for (const needle of item.contains) {
        expect({ input: item.input, needle, hit: expanded.includes(needle) }).toEqual({
          input: item.input,
          needle,
          hit: true,
        })
      }
    }
  })
})

describe('默认档位：默认不注入，配了才注入', () => {
  test('档位解析：只认 off / lite / full / ultra，其余回落到 off', () => {
    expect(parsePonytailMode('ULTRA')).toBe('ultra')
    expect(parsePonytailMode(' lite ')).toBe('lite')
    expect(parsePonytailMode(undefined)).toBe('off')
    expect(parsePonytailMode('fast')).toBe('off')
    expect(parsePonytailMode(true)).toBe('off')
  })

  test('没有配置时：不注入，也不写 trace（默认不动别人的系统提示词）', async () => {
    const context = promptContext()
    const verdict = await createPonytailHooks().beforeSystemPrompt!(context)

    expect(verdict).toBeUndefined()
    expect(context.traces).toEqual([])
    expect(await readPonytailConfig()).toEqual({ defaultMode: 'off' })
  })

  test('认不出的档位同样不注入（不猜用户想要哪一档）', async () => {
    await configure({ defaultMode: '偶尔懒一下' })
    const context = promptContext()
    expect(await createPonytailHooks().beforeSystemPrompt!(context)).toBeUndefined()
    expect(context.traces).toEqual([])
  })

  test('配了 ultra：追加内容含档位与它那一档的差别，并写出一条 trace', async () => {
    await configure({ defaultMode: 'ultra' })
    const context = promptContext()
    const verdict = await createPonytailHooks().beforeSystemPrompt!(context)

    expect(verdict?.replace).toBeUndefined()
    expect(verdict?.append).toContain('档位 ultra')
    expect(verdict?.append).toContain('YAGNI 极端派')
    expect(verdict?.append).toContain('ponytail: <上限是什么>, <什么时候该升级>')
    // 「默认注入每轮都生效」与「用户口头喊停」的冲突要交代清楚，否则模型下一轮又被拉回来
    expect(verdict?.append).toContain('defaultMode 设为 off')
    expect(context.traces).toHaveLength(1)
    expect(context.traces[0]).toContain('ultra')
  })

  test('环境变量优先于配置文件（且名字与速查卡里写的一致）', async () => {
    await configure({ defaultMode: 'full' })
    process.env.A_DA_PLUGIN_PONYTAIL_DEFAULT_MODE = 'lite'

    expect(await readPonytailConfig()).toEqual({ defaultMode: 'lite' })
    const verdict = await createPonytailHooks().beforeSystemPrompt!(promptContext())
    expect(verdict?.append).toContain('档位 lite')
  })
})

/**
 * 「声明了却没人调用」是本项目最难发现的一类缺陷（AGENTS.md §15）。上面那组测的是
 * 钩子函数本身；这一组测**运行时链路**：加载器把本插件带上了没有、钩子运行层有没有
 * 把它合进主循环要用的那份钩子表。少任何一环，插件都是"装了、没生效"，且不报错。
 */
describe('运行时链路：加载器与钩子运行层真的带上了这个插件', () => {
  test('加载后状态为 ready，且它的 beforeSystemPrompt 出现在合成的钩子表里', async () => {
    await defaultExtensionLoader.autoLoadExtensions(workspace)

    const plugin = findLoadedPlugin(workspace, `builtin:${PONYTAIL_PLUGIN_ID}`)
    expect(plugin).toBeDefined()
    expect(plugin?.status).toBe('ready')
    expect(typeof plugin?.contributions.hooks?.beforeSystemPrompt).toBe('function')

    const hooks = composePluginHooks({
      kind: 'main',
      workspace,
      capabilities: await readPluginCapabilities(workspace),
    })
    expect(typeof hooks.beforeSystemPrompt).toBe('function')

    // 默认档位下不表态（返回 undefined），配了档位才出现在追加内容里
    const context = promptContext()
    expect(await hooks.beforeSystemPrompt!(context)).toBeUndefined()

    await configure({ defaultMode: 'ultra' })
    const verdict = await hooks.beforeSystemPrompt!(promptContext())
    expect(verdict?.append).toContain('【Ponytail 偷懒模式（档位 ultra）】')
  })
})
