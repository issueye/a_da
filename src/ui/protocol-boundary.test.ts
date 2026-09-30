/**
 * 守门测试：**UI 只能通过 `ui/client` 访问 agent 侧**（拆分计划 M0-5、M2-7）。
 *
 * 为什么需要它：拆分的最大风险不是"改不动"，而是**半拆状态长期存在**——有的组件走了
 * 客户端接口、有的还直接抓着 store 单例或某个管理器，而这种"看起来拆了"最难查。
 * 所以这条线用测试钉住，而不是靠自觉。两道：
 *
 * 1. **M0**：UI 不得 import `agent/store`、不得出现 `store.` 用法（清单已清空）；
 * 2. **M2**：UI 不得从 `agent/**` import **实现符号**——只允许纯函数与纯常量
 *    （无状态、不读磁盘、不写配置）。判据是**按符号**而不是按模块：像 `agent/prompts`
 *    这种模块同时导出管理器与纯函数，按模块放行等于把管理器也放进去。
 *
 * 注意：`src/ui/client/**` 是允许 import 实现模块的（它就是适配器），所以扫描时排除它。
 */

import { describe, expect, test } from 'bun:test'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

const root = join(import.meta.dir, '..', '..')
const uiDir = join(root, 'src', 'ui')

/**
 * 待迁移清单。**M0 已完成（2026-09-30）：这里必须是空的**——留空数组是有意的，
 * 它同时是"UI 侧对 store 的依赖已清零"的验收标记；一旦有人新开一个直接 import
 * agent/store 的组件，第一条断言立刻红。
 */
const PENDING: string[] = []

/**
 * 允许 UI 直接 import 的 agent 侧符号：**纯函数与纯常量**。
 *
 * 判据（新加一个要能说服人）：给定同样输入永远同样输出、不读盘、不写配置、没有实例状态。
 * 这类东西搬去客户端层只是搬运，留着也不会让拆分退步；而管理器、配置读写、加载器一律不行。
 */
const ALLOWED_UI_SYMBOLS = new Set([
  // 纯计算
  'computeThreadStats',
  'patchStats',
  'computeContextBreakdown',
  'getModelContextWindow',
  'expandPromptTemplate',
  'parseHookTimeout',
  'envOverrides',
  'formatHeadersText',
  'parseHeadersText',
  /** 工具调用的展示摘要（名字 + 参数 → 一行文本），纯格式化 */
  'describeTool',
  // 纯展示常量与映射
  'CAPABILITY_SWITCHES',
  'describePluginRestrictions',
  'SUBAGENT_HEX_COLORS',
  'getSubagentColor',
  'PUBLIC_WORKSPACE_LABEL',
  'workspaceLabel',
  'isPublicWorkspace',
])

function listUiSources(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) {
      if (entry === 'client') continue // 适配器，允许 import 实现模块
      listUiSources(full, out)
      continue
    }
    if (!/\.(ts|tsx)$/.test(entry)) continue
    if (/\.test\.(ts|tsx)$/.test(entry)) continue
    out.push(full)
  }
  return out
}

/** 一个文件是否还在直接依赖 store：导入它、或用 `store.xxx`。 */
function coupledToStore(source: string): boolean {
  const importsStore = /from '(\.\.\/)*agent\/store'/.test(source)
  const usesStore = /\bstore\./.test(source)
  return importsStore || usesStore
}

/**
 * 列出文件里从 `agent/**` 导入的**值**符号（跳过纯类型导入——类型在运行时不存在）。
 *
 * 多行 import 也要认（`[^}]` 能跨行匹配）；`import type { … }` 与内联 `type X` 都跳过；
 * 默认导入一律算实现依赖（`agent/**` 的默认导出都是单例管理器这类东西）。
 */
function valueImportsFromAgent(source: string): string[] {
  const found: string[] = []
  for (const match of source.matchAll(/import\s+(type\s+)?\{([^}]*)\}\s+from\s+'([^']*agent\/[^']+)'/g)) {
    if (match[1]) continue
    for (const raw of match[2]!.split(',')) {
      const name = raw.trim()
      if (!name || name.startsWith('type ')) continue
      found.push(name.replace(/\s+as\s+.*$/, ''))
    }
  }
  for (const match of source.matchAll(/import\s+([A-Za-z_$][\w$]*)\s+(?:,\s*\{[^}]*\}\s+)?from\s+'([^']*agent\/[^']+)'/g)) {
    found.push(match[1]!)
  }
  return found
}

describe('M0 边界：UI 只认 ui/client', () => {
  test('清单外的 UI 源码不得 import agent/store，也不得出现 store. 用法', () => {
    const offenders = listUiSources(uiDir)
      .map((full) => ({ rel: relative(root, full).replace(/\\/g, '/'), source: readFileSync(full, 'utf8') }))
      .filter((file) => coupledToStore(file.source) && !PENDING.includes(file.rel))
      .map((file) => file.rel)

    expect(offenders).toEqual([])
  })

  test('待迁移清单里不能有已经迁干净的文件（清单必须同步删行）', () => {
    const stale = PENDING.filter((rel) => {
      const source = readFileSync(join(root, rel), 'utf8')
      return !coupledToStore(source)
    })

    expect(stale).toEqual([])
  })

  test('协议层不得依赖任何实现模块（只允许 `agent/**/types` 这类纯类型模块）', () => {
    const protocolDir = join(root, 'src', 'shared', 'protocol')
    const bad: string[] = []

    for (const full of listUiSources(protocolDir)) {
      const rel = relative(root, full).replace(/\\/g, '/')
      const source = readFileSync(full, 'utf8')
      for (const match of source.matchAll(/from '([^']+)'/g)) {
        const spec = match[1]!
        if (!spec.includes('agent/')) continue
        // 允许 `.../agent/types`、`.../agent/<域>/types`；其余一律算实现模块
        if (/(^|\/)types$/.test(spec)) continue
        bad.push(`${rel} → ${spec}`)
      }
    }

    expect(bad).toEqual([])
  })

  test('M2 边界：UI 不得 import agent 侧的实现符号（只允许纯函数与纯常量）', () => {
    const offenders: string[] = []

    for (const full of listUiSources(uiDir)) {
      const rel = relative(root, full).replace(/\\/g, '/')
      const source = readFileSync(full, 'utf8')
      for (const name of valueImportsFromAgent(source)) {
        if (!ALLOWED_UI_SYMBOLS.has(name)) offenders.push(`${rel} → ${name}`)
      }
    }

    expect(offenders).toEqual([])
  })
})
