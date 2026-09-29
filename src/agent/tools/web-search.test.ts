/**
 * web_search 扩展这条链路：jiti 编译 → 注册进 ToolRegistry → `runTool` 执行。
 *
 * 夹具是**测试自带的**，写在临时工作区的 `.ada/extensions/` 下，不依赖仓库里任何
 * 实际插件文件——早先这里指向仓库内的 `.ada/extensions/web-search.ts`，那个文件
 * 与用户全局 `~/.a-da/extensions/` 的那份重复（会两处各加载一次），已删除。
 * 测试只关心链路与解析行为，所以夹具保持最小：能触发这五条断言即可，不是那份实现的副本。
 *
 * 只有网络是假的——一个用例不该去敲搜索引擎的门。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { ExtensionLoader } from './loader'
import { defaultToolRegistry, runTool } from '../tools'

/** 一份缩小的 Bing 结果页：三条 `b_algo` 块，其中一条是相对链接（应被丢弃）。 */
const RESULTS_PAGE = `
<ol id="b_results">
<li class="b_algo" data-id iid=SERP.1>
  <link rel="stylesheet" href="/rp/x.br.css"/>
  <h2><a href="https://bun.sh/docs/test">Bun <strong>test</strong> runner</a></h2>
  <p class="b_lineclamp">Run tests with <strong>bun test</strong> &amp; watch mode.</p>
</li>
<li class="b_algo" data-id iid=SERP.2>
  <h2><a href="/rp/relative-should-be-skipped">relative link</a></h2>
  <p>Should not survive.</p>
</li>
<li class="b_algo" data-id iid=SERP.3>
  <h2><a href="https://example.com/plain">A plain result</a></h2>
  <p>Nothing special here.</p>
</li>
</ol>
`

/**
 * 最小夹具：只保留这五条用例需要的形状（缺参数 / HTTP 失败 / 解析不出条目 / 正常解析）。
 * 与真实扩展同样导出**声明式描述符**，走的是同一条加载路径。
 *
 * 用 `String.raw` 而不是普通模板字面量：夹具正文里的 `\s`、`\/`、`\n` 都是**要写进
 * 目标 .ts 源码**的字符，不能被当前这个字面量先吃掉（普通模板字面量里 `\s` 会变成 `s`、
 * `\/` 会变成 `/` —— 后者会把正则提前截断，生成一个语法错的插件，jiti 编译失败、
 * 插件被静默跳过，症状是「夹具明明写了却加载不出来」）。
 */
const FIXTURE = String.raw`
const MAX_OUTPUT = 6000

/** 把一小段 HTML 变成纯文本：去标签、还原常见实体、压掉空白。 */
function plain(html) {
  return html
    .replace(/<[^>]*>/g, '')
    .replace(/&nbsp;/g, ' ')
    .replace(/&amp;/g, '&')
    .replace(/\s+/g, ' ')
    .trim()
}

/** 取出 title / url / snippet；相对链接直接跳过，页面结构变了就返回空。 */
function parseResults(html) {
  const results = []
  for (const block of html.split(/<li class="b_algo/).slice(1)) {
    const link = /<h2[^>]*>\s*<a[^>]+href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/.exec(block)
    if (!link || !/^https?:\/\//.test(link[1])) continue
    const snippet = /<p[^>]*>([\s\S]*?)<\/p>/.exec(block.slice(link.index))
    results.push({
      title: plain(link[2] || ''),
      url: link[1],
      snippet: snippet ? plain(snippet[1] || '') : '',
    })
  }
  return results
}

export default {
  name: '联网搜索 (web-search)',
  description: '用 Bing 搜索互联网',
  tools: [
    {
      name: 'web_search',
      label: '联网搜索',
      description: '用搜索引擎（Bing）查互联网。',
      parameters: {
        type: 'object',
        properties: { query: { type: 'string', description: '搜索词。' } },
        required: ['query'],
      },
      async execute(_callId, args) {
        const query = String(args?.query ?? '').trim()
        if (!query) return { output: '缺少 query 参数。', ok: false }

        try {
          const response = await fetch('https://cn.bing.com/search?q=' + encodeURIComponent(query))
          if (!response.ok) {
            return { output: '搜索失败：HTTP ' + response.status, ok: false }
          }
          const results = parseResults(await response.text())
          if (results.length === 0) {
            return {
              output: '没有从结果页里解析出条目（' + query + '）。搜索引擎的页面结构可能变了。',
              ok: false,
            }
          }
          const text = results
            .map((r, i) => (i + 1) + '. ' + r.title + '\n   ' + r.url + '\n   ' + r.snippet)
            .join('\n\n')
          return { output: text.slice(0, MAX_OUTPUT), ok: true, details: { count: results.length } }
        } catch (error) {
          return { output: '搜索失败：' + (error?.message ?? error), ok: false }
        }
      },
    },
  ],
}
`

let workspace = ''
const realFetch = globalThis.fetch

/** 让夹具里的 fetch 拿到的就是这一份响应；Bun 的 fetch 类型还带 preconnect，转一手。 */
function stubFetch(body: string, status = 200): void {
  globalThis.fetch = (async () => new Response(body, { status })) as unknown as typeof fetch
}

beforeAll(async () => {
  workspace = await mkdtemp(join(tmpdir(), 'a-da-web-search-'))
  const extDir = join(workspace, '.ada', 'extensions')
  await mkdir(extDir, { recursive: true })
  await writeFile(join(extDir, 'web-search.ts'), FIXTURE, 'utf-8')
})

afterAll(async () => {
  globalThis.fetch = realFetch
  defaultToolRegistry.unregister('web_search')
  if (workspace) await rm(workspace, { recursive: true, force: true })
})

describe('web_search 扩展（测试自带夹具）', () => {
  test('loads through the extension loader and registers a tool', async () => {
    const loader = new ExtensionLoader()
    const loaded = await loader.loadExtensionsFromDir(
      join(workspace, '.ada', 'extensions'),
      workspace
    )
    expect(loaded).toContain('web_search')

    // 注册进来的工具会和内置工具一起发给模型——这正是扩展的意义所在。
    const names = defaultToolRegistry.getToolsForWorkspace(workspace).map((tool) => tool.name)
    expect(names).toContain('web_search')
  })

  test('turns a result page into readable lines', async () => {
    stubFetch(RESULTS_PAGE)

    const result = await runTool(workspace, { name: 'web_search', args: { query: 'bun test' } })

    expect(result.ok).toBe(true)
    // 标签和实体要还原成可读文字，相对链接（Bing 页面的样式表之类）要丢掉。
    expect(result.output).toContain('https://bun.sh/docs/test')
    expect(result.output).toContain('Bun test runner')
    expect(result.output).toContain('Run tests with bun test & watch mode.')
    expect(result.output).toContain('https://example.com/plain')
    expect(result.output).not.toContain('relative link')
    expect(result.output).not.toContain('<strong>')
  })

  test('says so when the page yields nothing, instead of pretending it searched', async () => {
    stubFetch('<html>nothing here</html>')

    const result = await runTool(workspace, { name: 'web_search', args: { query: 'x' } })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('没有从结果页里解析出条目')
  })

  test('reports a missing query instead of searching for nothing', async () => {
    const result = await runTool(workspace, { name: 'web_search', args: {} })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('缺少 query')
  })

  test('reports an HTTP failure', async () => {
    stubFetch('nope', 503)

    const result = await runTool(workspace, { name: 'web_search', args: { query: 'x' } })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('HTTP 503')
  })
})
