/**
 * 模型请求头的构造。
 *
 * 单独一个文件是因为有**两个**调用点：真实对话（`ai/stream.ts`）与设置里的
 * 「测试连接」（`config.ts`）。两处必须共用同一份实现，否则会出现"测试连接能通、
 * 真实对话不通"（或反过来）这种极难查的分歧——而它恰恰是自定义头最常踩的坑。
 */

/** 参与构造的最小配置面。刻意用结构类型，避免 `ai/` 反向依赖 `config.ts`。 */
export interface HeaderSourceConfig {
  apiKey?: string
  /** 用户自定义请求头：追加到默认头之上，**同名覆盖**（大小写不敏感） */
  headers?: Record<string, string>
}

/**
 * 构造请求头。
 *
 * 规则与理由：
 * - 默认 `content-type: application/json`——请求体永远是 JSON；
 * - `apiKey` 非空时才带 `authorization: Bearer …`：空值带一个 `Bearer ` 会让某些
 *   网关报"凭证格式错误"，而真正的原因是压根没配 key，错误信息会误导人；
 * - 自定义头**最后合并，且大小写不敏感地覆盖同名默认头**。HTTP 头名不区分大小写，
 *   若用户写 `Authorization` 而默认是 `authorization`，朴素展开会**同时发出两个**，
 *   服务器取哪个由实现决定——这正是"我配了自定义头却不生效"的典型成因。
 */
export function buildRequestHeaders(config: HeaderSourceConfig): Record<string, string> {
  const headers: Record<string, string> = { 'content-type': 'application/json' }
  const apiKey = config.apiKey?.trim()
  if (apiKey) {
    headers.authorization = `Bearer ${apiKey}`
  }

  for (const [rawName, rawValue] of Object.entries(config.headers ?? {})) {
    const name = rawName.trim()
    if (!name) continue
    // 先摘掉任何大小写形式的同名默认头，再按用户给的写法放进去
    const lowered = name.toLowerCase()
    for (const existing of Object.keys(headers)) {
      if (existing.toLowerCase() === lowered) delete headers[existing]
    }
    const value = String(rawValue ?? '').trim()
    if (value) headers[name] = value
  }

  return headers
}
