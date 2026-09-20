/**
 * 仓库里那个 web_search 扩展。
 *
 * 走的是真实路径：jiti 编译 → 注册进 ToolRegistry → `runTool` 执行。只有网络是假的
 * ——一个用例不该去敲搜索引擎的门。
 *
 *   bun test
 */

import { afterAll, describe, expect, test } from 'bun:test'
import { join } from 'node:path'
import { ExtensionLoader } from './loader'
import { defaultToolRegistry, runTool } from '../tools'

const EXTENSIONS = join(import.meta.dir, '..', '..', '..', '.ada', 'extensions')

/** 一份缩小的 Bing 结果页：两条 `b_algo` 块，各带标题链接和摘要。 */
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

const realFetch = globalThis.fetch

/** 让扩展里的 fetch 拿到的就是这一份响应；Bun 的 fetch 类型还带 preconnect，转一手。 */
function stubFetch(body: string, status = 200): void {
  globalThis.fetch = (async () => new Response(body, { status })) as unknown as typeof fetch
}

afterAll(() => {
  globalThis.fetch = realFetch
  defaultToolRegistry.unregister('web_search')
})

describe('the web_search extension in this repo', () => {
  test('loads through the extension loader and registers a tool', async () => {
    const loader = new ExtensionLoader()
    const loaded = await loader.loadExtensionsFromDir(EXTENSIONS, process.cwd())
    expect(loaded).toContain('web_search')

    // 注册进来的工具会和内置工具一起发给模型——这正是扩展的意义所在。
    const names = defaultToolRegistry.getToolsForWorkspace(process.cwd()).map((tool) => tool.name)
    expect(names).toContain('web_search')
  })

  test('turns a result page into readable lines', async () => {
    stubFetch(RESULTS_PAGE)

    const result = await runTool(process.cwd(), { name: 'web_search', args: { query: 'bun test' } })

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

    const result = await runTool(process.cwd(), { name: 'web_search', args: { query: 'x' } })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('没有从结果页里解析出条目')
  })

  test('reports a missing query instead of searching for nothing', async () => {
    const result = await runTool(process.cwd(), { name: 'web_search', args: {} })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('缺少 query')
  })

  test('reports an HTTP failure', async () => {
    stubFetch('nope', 503)

    const result = await runTool(process.cwd(), { name: 'web_search', args: { query: 'x' } })
    expect(result.ok).toBe(false)
    expect(result.output).toContain('HTTP 503')
  })
})
