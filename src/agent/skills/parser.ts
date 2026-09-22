/**
 * SKILL.md 解析器
 * 支持解析标准 YAML Frontmatter 与 Markdown 指令正文
 * 兼容 ZCode 与 Claude Code 技能格式
 */

import { basename, dirname } from 'node:path'
import type { SkillMetadata } from './types'

export interface ParsedSkill {
  metadata: SkillMetadata
  body: string
  hasFrontmatter: boolean
}

/**
 * 展开技能正文中的环境上下文变量
 * 例如：${ADA_SKILL_DIR}、${CLAUDE_SKILL_DIR}、${ZCODE_SKILL_DIR} 替换为技能所在绝对目录
 */
export function expandSkillVariables(content: string, baseDirectory: string): string {
  return content.replace(/\$\{(ADA_SKILL_DIR|CLAUDE_SKILL_DIR|ZCODE_SKILL_DIR)\}/g, baseDirectory)
}

/**
 * 解析原始 SKILL.md 文件内容
 * @param rawContent 文件内容
 * @param filePath 技能文件绝对路径（用于兜底推导技能名称）
 */
export function parseSkillMarkdown(rawContent: string, filePath: string): ParsedSkill {
  const fileName = basename(filePath)
  const isSkillMd = fileName.toLowerCase() === 'skill.md'
  const fallbackName = isSkillMd
    ? basename(dirname(filePath))
    : fileName.replace(/\.md$/i, '')

  const metadata: SkillMetadata = {
    name: fallbackName,
    description: '',
  }

  let body = rawContent.trim()
  let hasFrontmatter = false

  const fmMatch = rawContent.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/)
  if (fmMatch) {
    hasFrontmatter = true
    const yamlBlock = fmMatch[1]!
    body = fmMatch[2]!.trim()

    for (const rawLine of yamlBlock.split('\n')) {
      const line = rawLine.trim()
      if (!line || line.startsWith('#')) continue

      const colonIdx = line.indexOf(':')
      if (colonIdx > 0) {
        const key = line.slice(0, colonIdx).trim()
        let val = line.slice(colonIdx + 1).trim()
        // 去除外层引号
        val = val.replace(/^['"]|['"]$/g, '')

        if (key === 'name' && val) {
          metadata.name = val.trim()
        } else if (key === 'description') {
          metadata.description = val.trim()
        } else if (key === 'version') {
          metadata.version = val
        } else if (key === 'author') {
          metadata.author = val
        } else if (key === 'whenToUse' || key === 'when_to_use') {
          metadata.whenToUse = val
        } else if (key === 'compatibility') {
          metadata.compatibility = val
        } else if (key === 'license') {
          metadata.license = val
        } else if (
          key === 'disable-model-invocation' ||
          key === 'disable_model_invocation' ||
          key === 'disableModelInvocation'
        ) {
          metadata.disableModelInvocation =
            val.toLowerCase() === 'true' || val === '1' || val.toLowerCase() === 'yes'
        } else if (
          key === 'allowed-tools' ||
          key === 'allowed_tools' ||
          key === 'allowedTools'
        ) {
          const cleanVal = val.replace(/^\[|\]$/g, '')
          // 支持逗号或空格分隔
          metadata.allowedTools = cleanVal
            .split(/[,\s]+/)
            .map((t) => t.trim().replace(/^['"]|['"]$/g, ''))
            .filter(Boolean)
        } else if (key === 'tags') {
          // 处理 [tag1, tag2] 或 tag1, tag2
          const cleanVal = val.replace(/^\[|\]$/g, '')
          metadata.tags = cleanVal
            .split(',')
            .map((t) => t.trim().replace(/^['"]|['"]$/g, ''))
            .filter(Boolean)
        } else if (key === 'disallowedTools' || key === 'disallowed_tools') {
          const cleanVal = val.replace(/^\[|\]$/g, '')
          metadata.disallowedTools = cleanVal
            .split(',')
            .map((t) => t.trim().replace(/^['"]|['"]$/g, ''))
            .filter(Boolean)
        }
      }
    }
  }

  if (!metadata.description) {
    // 若未在 Frontmatter 中提供描述，则尝试从首段非标题文字提取作为简短描述
    const firstParagraph = body
      .split('\n')
      .map((l) => l.trim())
      .find((l) => l && !l.startsWith('#') && !l.startsWith('!'))
    if (firstParagraph) {
      metadata.description = firstParagraph.slice(0, 200)
    } else {
      metadata.description = `自定义技能: ${metadata.name}`
    }
  }

  return {
    metadata,
    body,
    hasFrontmatter,
  }
}
