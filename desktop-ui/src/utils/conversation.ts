import type { Thread } from '../types'

/**
 * 这条会话是不是**还没开始**（决定"模式能不能选"）。
 *
 * 🔴 判据必须与**服务端**拒改模式的判据同源（`thread.setMode` 那两个字段）：
 * `items` 与 `messages` **都为空**。两处各写一份的话，会出现"滑块能点、点了报错"
 * 或者"该能选的时候不给选"——而这两种都只有用户先撞上才会被发现。
 *
 * 为什么"已经有内容就不能改模式"是硬规则：模式决定系统提示词与工具集，
 * 中途换掉等于把同一段对话的一半交给另一个智能体去解释，事后没人说得清哪句话是谁说的。
 */
export const isFreshConversation = (thread?: Thread): boolean =>
  !thread || (thread.items.length === 0 && (thread.messages?.length ?? 0) === 0)
