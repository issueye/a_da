/**
 * 一个用到扩展 API 的真实例子：给 Agent 加一个联网搜索工具。
 *
 * 放在项目的 `.ada/extensions/` 下，打开这个项目时会自动加载（jiti 直接跑 .ts）。
 * 想让每个项目都有它，把文件复制到 `~/.a-da/extensions/` 就行。
 *
 * 刻意不 import 应用里的任何东西：扩展是可以被复制到别处的独立文件，一旦依赖
 * 仓库内的相对路径，复制出去就废了。
 *
 * 搜索引擎默认用 Bing，因为它不需要 API key、在国内也直连得到（DuckDuckGo 实测连不上，
 * 超时）。要换引擎，改 ENDPOINT 和 parseResults 两个地方。
 *
 * 需要走代理时不用改这里：Bun 的 fetch 认 `HTTPS_PROXY` / `HTTP_PROXY` 环境变量，
 * 应用和扩展的网络请求都会跟着走。比如启动前设 `HTTPS_PROXY=http://127.0.0.1:7897`。
 *
 * 它不在只读白名单里，所以「只读」模式下每次调用都要你点批准——一次搜索会把查询词
 * 发给第三方，这确实该问一声。
 */

const ENDPOINT = 'https://cn.bing.com/search'
/** 带一个普通浏览器的 UA：默认 UA 拿到的页面结构不一样。 */
const USER_AGENT =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36'
const MAX_RESULTS = 6
const TIMEOUT_MS = 20_000
const MAX_OUTPUT = 6000

/** 把一小段 HTML 变成纯文本：去标签、还原实体、压掉空白。 */
function plain(html) {
  const codePoint = (value, radix) => {
    const n = Number.parseInt(value, radix)
    try {
      return Number.isFinite(n) && n >= 0 && n <= 0x10ffff ? String.fromCodePoint(n) : ''
    } catch {
      return ''
    }
  }
  return (
    html
      .replace(/<[^>]*>/g, '')
      // 数字实体是通用写法，别给每个字符都写一条规则（Bing 的摘要里就有 &#0183;）。
      .replace(/&#x([0-9a-f]+);/gi, (_, hex) => codePoint(hex, 16))
      .replace(/&#(\d+);/g, (_, dec) => codePoint(dec, 10))
      .replace(/&nbsp;|&ensp;|&emsp;|&thinsp;/g, ' ')
      .replace(/&amp;/g, '&')
      .replace(/&lt;/g, '<')
      .replace(/&gt;/g, '>')
      .replace(/&quot;/g, '"')
      .replace(/&apos;/g, "'")
      .replace(/&hellip;/g, '…')
      .replace(/&mdash;/g, '—')
      .replace(/&ndash;/g, '–')
      .replace(/\s+/g, ' ')
      .trim()
  )
}

/**
 * 从结果页里取出标题、地址、摘要。
 *
 * 每条结果是一个 `<li class="b_algo">` 块，标题是块里的 `<h2><a href>`（直链，不是
 * 跳转），摘要是标题之后的第一个 `<p>`。页面结构变了这里就会返回空，工具会说实话
 * 而不是假装搜到了。
 */
export function parseResults(html) {
  const results = []
  for (const block of html.split(/<li class="b_algo/).slice(1)) {
    if (results.length >= MAX_RESULTS) break
    const link = /<h2[^>]*>\s*<a[^>]+href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/.exec(block)
    if (!link || !link[1] || !/^https?:\/\//.test(link[1])) continue
    const snippet = /<p[^>]*>([\s\S]*?)<\/p>/.exec(block.slice(link.index))
    results.push({
      title: plain(link[2] ?? ''),
      url: link[1],
      snippet: snippet ? plain(snippet[1] ?? '') : '',
    })
  }
  return results
}

export default function (api) {
  api.trace('web-search 扩展已加载：联网搜索可用')

  api.registerTool({
    name: 'web_search',
    label: '联网搜索',
    description:
      '用搜索引擎（Bing）查互联网。工作区里翻不到答案时用它：库的最新用法、报错信息、某个接口的现状。',
    parameters: {
      type: 'object',
      properties: {
        query: { type: 'string', description: '搜索词。' },
      },
      required: ['query'],
    },
    async execute(_callId, args, signal) {
      const query = String(args?.query ?? '').trim()
      if (!query) return { output: '缺少 query 参数。', ok: false }

      const timeout = AbortSignal.timeout(TIMEOUT_MS)
      try {
        const response = await fetch(`${ENDPOINT}?q=${encodeURIComponent(query)}`, {
          headers: { 'user-agent': USER_AGENT, 'accept-language': 'zh-CN,zh;q=0.9,en;q=0.8' },
          signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
        })
        if (!response.ok) {
          return { output: `搜索失败：HTTP ${response.status}`, ok: false }
        }

        const results = parseResults(await response.text())
        if (results.length === 0) {
          return {
            output: `没有从结果页里解析出条目（${query}）。搜索引擎的页面结构可能变了，改一下扩展里的 parseResults。`,
            ok: false,
          }
        }

        const text = results
          .map((result, index) => `${index + 1}. ${result.title}\n   ${result.url}\n   ${result.snippet}`)
          .join('\n\n')
        return {
          output: text.slice(0, MAX_OUTPUT),
          ok: true,
          details: { count: results.length, query },
        }
      } catch (error) {
        return { output: `搜索失败：${error?.message ?? error}`, ok: false }
      }
    },
  })
}
