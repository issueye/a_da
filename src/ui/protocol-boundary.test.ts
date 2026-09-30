/**
 * 守门测试：**UI 只能通过 `ui/client` 访问 agent 侧**（拆分计划 M0-5）。
 *
 * 为什么需要它：拆分的最大风险不是"改不动"，而是**半拆状态长期存在**——有的组件走了
 * 客户端接口、有的还直接抓着 store 单例，而这种"看起来拆了"最难查。所以这条线用测试钉住，
 * 而不是靠自觉。
 *
 * 用法（迁移进行中）：`PENDING` 是**待迁移清单**，每迁完一个文件就删一行。
 * 测试两头都管：
 * - 清单外的文件一旦出现 `agent/store` 导入或 `store.` 用法 → **红**（拦住回退与新增）；
 * - 清单里的文件一旦已经迁干净 → 也**红**（清单本身会腐烂，必须同步删掉）。
 *
 * **M0 完成时 `PENDING` 必须是空数组。**
 *
 * 注意：`src/ui/client/**` 是允许 import store 的（它是 M0 的适配器），所以扫描时排除它。
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

function listUiSources(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) {
      if (entry === 'client') continue // 适配器，允许 import store
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
})
