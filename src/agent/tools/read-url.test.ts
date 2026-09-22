import { describe, expect, test } from 'bun:test'
import { createReadUrlTool } from './builtins/read-url'

describe('read_url_content 工具测试', () => {
  const tool = createReadUrlTool()

  test('工具具有正确的名称、标签与描述', () => {
    expect(tool.name).toBe('read_url_content')
    expect(tool.label).toBe('读取网页内容')
    expect(tool.parameters).toBeDefined()
  })

  test('非法 URL 或空 URL 返回错误信息', async () => {
    const res1 = await tool.execute('c1', { url: '' })
    expect(res1.ok).toBe(false)
    expect(res1.output).toContain('未提供有效')

    const res2 = await tool.execute('c2', { url: 'ftp://example.com' })
    expect(res2.ok).toBe(false)
    expect(res2.output).toContain('非法 URL 协议')
  })
})
