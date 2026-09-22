/**
 * 网页内容抓取与提取工具 (read_url_content)
 * 参考 ZCode 架构设计
 * 为 Vibe Coding 提供快速查阅第三方库、API 规范与技术文档能力
 */

import type { AgentTool, AgentToolResult } from '../../core/types'

const MAX_URL_CONTENT_LENGTH = 35_000
const FETCH_TIMEOUT_MS = 15_000

function htmlToMarkdown(html: string): string {
  let cleaned = html
    // 移除无用标签及内容
    .replace(/<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>/gi, '')
    .replace(/<style\b[^<]*(?:(?!<\/style>)<[^<]*)*<\/style>/gi, '')
    .replace(/<nav\b[^<]*(?:(?!<\/nav>)<[^<]*)*<\/nav>/gi, '')
    .replace(/<footer\b[^<]*(?:(?!<\/footer>)<[^<]*)*<\/footer>/gi, '')
    .replace(/<!--[\s\S]*?-->/g, '')

  // 转换常用标签为 Markdown
  cleaned = cleaned
    .replace(/<h1[^>]*>([\s\S]*?)<\/h1>/gi, '\n# $1\n')
    .replace(/<h2[^>]*>([\s\S]*?)<\/h2>/gi, '\n## $1\n')
    .replace(/<h3[^>]*>([\s\S]*?)<\/h3>/gi, '\n### $1\n')
    .replace(/<h4[^>]*>([\s\S]*?)<\/h4>/gi, '\n#### $1\n')
    .replace(/<li[^>]*>([\s\S]*?)<\/li>/gi, '\n- $1')
    .replace(/<pre[^>]*><code[^>]*>([\s\S]*?)<\/code><\/pre>/gi, '\n```\n$1\n```\n')
    .replace(/<code[^>]*>([\s\S]*?)<\/code>/gi, '`$1`')
    .replace(/<p[^>]*>([\s\S]*?)<\/p>/gi, '\n$1\n')
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<a[^>]+href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/gi, '[$2]($1)')
    // 移除所有其余剩余 HTML 标签
    .replace(/<[^>]+>/g, '')
    // HTML 实体解码
    .replace(/&nbsp;/g, ' ')
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")

  // 清洗多余空行与两端空白
  const lines = cleaned
    .split('\n')
    .map((l) => l.trim())
    .filter((l, idx, arr) => !(l === '' && arr[idx - 1] === ''))
  return lines.join('\n').trim()
}

export function createReadUrlTool(): AgentTool<{ url: string }> {
  return {
    name: 'read_url_content',
    label: '读取网页内容',
    description: '抓取指定公网 URL 的网页内容并提取为纯文本/Markdown 格式。适用于查阅技术文档、开源库 API、博客或报错排查资料。',
    parameters: {
      type: 'object',
      properties: {
        url: {
          type: 'string',
          description: '需要读取的 HTTP 或 HTTPS 网页完整链接',
        },
      },
      required: ['url'],
    },
    async execute(_callId: string, args: { url: string }): Promise<AgentToolResult> {
      const url = (args?.url || '').trim()
      if (!url) {
        return {
          output: '未提供有效的 URL 地址。',
          ok: false,
        }
      }

      if (!/^https?:\/\//i.test(url)) {
        return {
          output: `非法 URL 协议: ${url}。仅支持 http:// 或 https:// 链接。`,
          ok: false,
        }
      }

      try {
        const controller = new AbortController()
        const timeout = setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS)

        const res = await fetch(url, {
          signal: controller.signal,
          headers: {
            'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 a-da/1.0',
            'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,text/plain;q=0.8,*/*;q=0.7',
          },
        })
        clearTimeout(timeout)

        if (!res.ok) {
          return {
            output: `网页抓取失败：HTTP ${res.status} ${res.statusText}`,
            ok: false,
          }
        }

        const rawText = await res.text()
        const contentType = res.headers.get('content-type') || ''

        let content = ''
        if (contentType.includes('application/json') || contentType.includes('text/plain')) {
          content = rawText
        } else {
          content = htmlToMarkdown(rawText)
        }

        if (content.length > MAX_URL_CONTENT_LENGTH) {
          content = `${content.slice(0, MAX_URL_CONTENT_LENGTH)}\n\n[内容已截断，原内容超过 ${MAX_URL_CONTENT_LENGTH} 字符]`
        }

        return {
          output: content || '网页内容为空。',
          ok: true,
          details: { url, status: res.status, length: content.length },
        }
      } catch (err) {
        return {
          output: `请求网页异常: ${(err as Error).message}`,
          ok: false,
        }
      }
    },
  }
}
