/**
 * 提示词模板参数替换与动态展开引擎
 * 参考 @earendil-works/pi-coding-agent core/prompt-templates.ts 工业级实现
 */

import type { PromptItem } from './types'

/**
 * 解析带有引号的命令行参数（支持 bash 风格双引号与单引号）
 */
export function parseCommandArgs(argsString: string): string[] {
  const args: string[] = []
  let current = ''
  let inQuote: string | null = null

  for (let i = 0; i < argsString.length; i++) {
    const char = argsString[i]!

    if (inQuote) {
      if (char === inQuote) {
        inQuote = null
      } else {
        current += char
      }
    } else if (char === '"' || char === "'") {
      inQuote = char
    } else if (/\s/.test(char)) {
      if (current) {
        args.push(current)
        current = ''
      }
    } else {
      current += char
    }
  }

  if (current) {
    args.push(current)
  }

  return args
}

/**
 * 替换提示词模板中的参数占位符
 * 支持：
 * - $1, $2, ... 位置参数
 * - $@ 与 $ARGUMENTS 全参数
 * - ${N:-default} 位置参数缺失或为空时的默认值
 * - ${@:-default} 与 ${ARGUMENTS:-default} 全参数默认值
 * - ${@:N} 从第 N 个参数开始的所有参数（1-indexed）
 * - ${@:N:L} 从第 N 个参数开始取 L 个参数
 */
export function substituteArgs(content: string, args: string[]): string {
  const allArgs = args.join(' ')

  return content.replace(
    /\$\{(\d+|ARGUMENTS|@):-([^}]*)\}|\$\{@:(\d+)(?::(\d+))?(?::-([^}]*))?\}|\$(ARGUMENTS|@|\d+)/g,
    (_match, defaultTarget, defaultValue, sliceStart, sliceLength, sliceDefault, simple) => {
      if (defaultTarget) {
        const value =
          defaultTarget === '@' || defaultTarget === 'ARGUMENTS'
            ? allArgs
            : args[parseInt(defaultTarget, 10) - 1]
        return value ? value : defaultValue
      }

      if (sliceStart) {
        let start = parseInt(sliceStart, 10) - 1
        if (start < 0) start = 0

        let result = ''
        if (sliceLength) {
          const length = parseInt(sliceLength, 10)
          result = args.slice(start, start + length).join(' ')
        } else {
          result = args.slice(start).join(' ')
        }
        if (!result && sliceDefault) {
          return sliceDefault
        }
        return result
      }

      if (simple === 'ARGUMENTS' || simple === '@') {
        return allArgs
      }

      const index = parseInt(simple, 10) - 1
      return args[index] ?? ''
    }
  )
}

/**
 * 如果输入文本以 / 开头且匹配可用提示词模板，自动展开其正文与参数
 * 返回展开后的文本；若未匹配或未启用则原样返回
 */
export function expandPromptTemplate(text: string, templates: PromptItem[]): string {
  const trimmed = text.trim()
  if (!trimmed.startsWith('/')) return text

  const match = trimmed.match(/^\/([^\s]+)(?:\s+([\s\S]*))?$/)
  if (!match) return text

  const templateName = match[1]!.toLowerCase()
  const argsString = match[2] ?? ''

  // 匹配模板（仅匹配已启用的提示词）
  const template = templates.find((t) => {
    if (!t.enabled) return false
    // 允许通过文件名、name 或 id 匹配
    const matchName = t.name.toLowerCase()
    const matchId = t.id.toLowerCase()
    return (
      matchName === templateName ||
      matchId === templateName ||
      matchId.endsWith(`_${templateName}`) ||
      matchId.endsWith(`:${templateName}`)
    )
  })

  if (template) {
    const args = parseCommandArgs(argsString)
    return substituteArgs(template.content, args)
  }

  return text
}
