import type { PluginDiagnostic } from '../types'

/**
 * 插件自述的**诊断**（P4 · SP-74）。
 *
 * # 为什么需要它
 *
 * `PluginItem.diagnostics` 从 P0 起就已经被扫描端如实填好了
 * （"执行面尚未接线 / 已停用 / 工具清单是文本扫描推断的"），但界面上**一处都没渲染**——
 * 于是插件卡永远显示不出任何问题，而"有壳无用"正是这套埋点要根治的误读。
 *
 * # 三条口径
 *
 * 1. **一条都不省**：`info` 也显示。这个字段的意义就是"这个插件现在到底怎么样"，
 *    只显示 warn/error 会让"正常"与"没被检查过"长得一样（AGENTS §2.2 缺席可见）。
 * 2. **空数组不占位置**：没有诊断就不渲染任何东西（内置插件通常为空）。
 * 3. **`hint` 要显示**：它是"该去看哪里"的指路（比如"执行面排期见某文档"），
 *    藏起来等于让人自己去猜。
 */
export function PluginDiagnostics({ diagnostics }: { diagnostics?: PluginDiagnostic[] }) {
  if (!diagnostics || diagnostics.length === 0) return null

  const styleOf = (level: PluginDiagnostic['level']) => {
    switch (level) {
      case 'error':
        return 'border-rose-300 dark:border-rose-500/30 bg-rose-50 dark:bg-rose-950/20 text-rose-700 dark:text-rose-300'
      case 'warn':
        return 'border-amber-300 dark:border-amber-500/30 bg-amber-50 dark:bg-amber-950/20 text-amber-700 dark:text-amber-300'
      default:
        return 'border-zinc-200 dark:border-zinc-700 bg-zinc-50 dark:bg-zinc-900/40 text-zinc-600 dark:text-zinc-400'
    }
  }

  return (
    <div className="pt-1 space-y-1">
      {diagnostics.map((d, i) => (
        <div
          key={`${d.level}-${i}`}
          className={`text-[10px] leading-relaxed px-2 py-1 rounded border ${styleOf(d.level)}`}
        >
          <span className="font-semibold uppercase mr-1">{d.level}</span>
          <span>{d.message}</span>
          {d.hint && <span className="block opacity-80 mt-0.5">→ {d.hint}</span>}
        </div>
      ))}
    </div>
  )
}
