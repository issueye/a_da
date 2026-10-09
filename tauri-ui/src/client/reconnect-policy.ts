// 重连退避策略（W6-T6）——**纯函数模块，无导入、无副作用**。
//
// 为什么单独一个文件：它需要能被独立加载与验证。
// 放在 `ws-client.ts` 里的话，任何 `import` 都会执行该模块的顶层代码
// （创建客户端实例 → 发起连接），于是"只想算一下退避"的脚本会挂住进程。

/** 首次重连的等待时间。 */
export const RECONNECT_BASE_MS = 500

/** 退避上限：无论连续失败多少次，等待时间都不会超过它。 */
export const RECONNECT_MAX_MS = 30_000

/** 抖动比例（±20%）。 */
export const RECONNECT_JITTER_RATIO = 0.2

/**
 * 第 `attempt` 次重连应等待多久（毫秒）。
 *
 * 形态：`base * 2^attempt`，**封顶** `MAX`，再叠加 ±`JITTER_RATIO` 的抖动。
 *
 * - **封顶**是必须的：纯指数增长在十几次失败后就会变成"几小时后才重连"，
 *   用户会以为应用死了；
 * - **抖动**是为了**避免惊群**：多个客户端同时断线时若退避完全一致，
 *   它们会在同一毫秒一起重连，把刚恢复的服务再打挂。
 *
 * @param attempt 连续失败次数（从 0 开始）
 * @param random  随机源（默认 `Math.random`），注入以便测试
 */
export function reconnectDelayMs(
  attempt: number,
  random: () => number = Math.random
): number {
  const exponent = Math.max(0, Math.floor(attempt))
  // `2 ** 40` 仍可表示，但为了让封顶生效、也避免超大指数，先夹住指数
  const safeExponent = Math.min(exponent, 30)
  const raw = RECONNECT_BASE_MS * 2 ** safeExponent
  const capped = Math.min(RECONNECT_MAX_MS, raw)
  const jitter = capped * RECONNECT_JITTER_RATIO * (random() * 2 - 1)
  return Math.max(0, Math.round(capped + jitter))
}
