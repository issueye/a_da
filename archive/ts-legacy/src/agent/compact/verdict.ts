/**
 * 把压缩钩子的判定应用到选择方案上。
 *
 * 单独成模块而不是塞在 store.ts 里：它是纯函数（进 selection、出 selection），
 * 可以脱离 store 单独测——而"追加保留的消息有没有真的被挪出待总结"这种事，
 * 靠读代码看不出对错，只能靠测试。
 *
 * 按**对象引用**匹配要保留的消息：插件拿到的是同一批消息对象，用身份判断最精确，
 * 按内容或时间戳匹配都可能误伤另一条同内容的消息。
 */

import type { AgentMessage } from '../core/types'
import type { BeforeCompactionResult } from '../core/events'
import type { CompactSelection } from './types'

export function applyCompactionVerdict(
  selection: CompactSelection,
  verdict: BeforeCompactionResult
): CompactSelection {
  const next = verdict.selection ?? selection
  if (!Array.isArray(next.messagesToSummarize) || !Array.isArray(next.preservedMessages)) {
    // 插件递回的结构不可用：宁可退回原方案，也不能让压缩读到 undefined
    return selection
  }

  const keep = verdict.keepMessages
  if (!keep || keep.length === 0) return next

  const keepSet = new Set<AgentMessage>(keep)
  const kept = next.messagesToSummarize.filter((message) => keepSet.has(message))
  if (kept.length === 0) return next

  return {
    ...next,
    messagesToSummarize: next.messagesToSummarize.filter((message) => !keepSet.has(message)),
    // 被留下的消息本来在待总结区（时间上更早），所以并到保留区**最前面**才不乱序
    preservedMessages: [...kept, ...next.preservedMessages],
  }
}
