/**
 * 内置辅助 Coding 插件：代码大纲与符号极速导航 (code-outline)
 */

import { existsSync } from 'node:fs'
import { readFile, stat } from 'node:fs/promises'
import { extname, isAbsolute, join } from 'node:path'
import type { BuiltinPluginPackage } from './types'

interface SymbolItem {
  kind: 'class' | 'interface' | 'type' | 'function' | 'method' | 'struct' | 'enum' | 'trait' | 'impl' | 'key'
  name: string
  line: number
  signature?: string
}

/** 基于轻量正则提取常见语言的顶层与主要符号大纲 */
function extractSymbols(content: string, ext: string): SymbolItem[] {
  const lines = content.split('\n')
  const symbols: SymbolItem[] = []
  const lowerExt = ext.toLowerCase()

  for (let i = 0; i < lines.length; i++) {
    const lineNum = i + 1
    const rawLine = lines[i] || ''
    const trimmed = rawLine.trim()

    // 忽略空行与简单单行注释
    if (!trimmed || trimmed.startsWith('//') || trimmed.startsWith('#') || trimmed.startsWith('/*') || trimmed.startsWith('*')) {
      continue
    }

    // 1. TypeScript / JavaScript (.ts, .tsx, .js, .jsx)
    if (['.ts', '.tsx', '.js', '.jsx', '.mjs', '.cjs'].includes(lowerExt)) {
      const classMatch = trimmed.match(/^(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([A-Za-z0-9_$]+)/)
      if (classMatch) {
        symbols.push({ kind: 'class', name: classMatch[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
      const ifaceMatch = trimmed.match(/^(?:export\s+)?interface\s+([A-Za-z0-9_$]+)/)
      if (ifaceMatch) {
        symbols.push({ kind: 'interface', name: ifaceMatch[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
      const typeMatch = trimmed.match(/^(?:export\s+)?type\s+([A-Za-z0-9_$]+)/)
      if (typeMatch) {
        symbols.push({ kind: 'type', name: typeMatch[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
      const fnMatch = trimmed.match(/^(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*([A-Za-z0-9_$]+)?\s*\(/)
      if (fnMatch) {
        symbols.push({ kind: 'function', name: fnMatch[1] || '(anonymous)', line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
      const constFnMatch = trimmed.match(/^(?:export\s+)?const\s+([A-Za-z0-9_$]+)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z0-9_$]+)\s*=>/)
      if (constFnMatch) {
        symbols.push({ kind: 'function', name: constFnMatch[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
    }

    // 2. Rust (.rs)
    if (lowerExt === '.rs') {
      const structMatch = trimmed.match(/^(?:pub(?:\([^)]+\))?\s+)?struct\s+([A-Za-z0-9_]+)/)
      if (structMatch) {
        symbols.push({ kind: 'struct', name: structMatch[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
      const enumMatch = trimmed.match(/^(?:pub(?:\([^)]+\))?\s+)?enum\s+([A-Za-z0-9_]+)/)
      if (enumMatch) {
        symbols.push({ kind: 'enum', name: enumMatch[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
      const traitMatch = trimmed.match(/^(?:pub(?:\([^)]+\))?\s+)?trait\s+([A-Za-z0-9_]+)/)
      if (traitMatch) {
        symbols.push({ kind: 'trait', name: traitMatch[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
      const implMatch = trimmed.match(/^impl(?:<[^>]+>)?\s+(?:[A-Za-z0-9_:]+\s+for\s+)?([A-Za-z0-9_:]+)/)
      if (implMatch) {
        symbols.push({ kind: 'impl', name: implMatch[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
      const fnMatch = trimmed.match(/^(?:pub(?:\([^)]+\))?\s+)?(?:async\s+)?(?:const\s+)?fn\s+([A-Za-z0-9_]+)/)
      if (fnMatch) {
        symbols.push({ kind: 'function', name: fnMatch[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
    }

    // 3. Python (.py)
    if (lowerExt === '.py') {
      const pyClass = trimmed.match(/^class\s+([A-Za-z0-9_]+)/)
      if (pyClass) {
        symbols.push({ kind: 'class', name: pyClass[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
      const pyDef = trimmed.match(/^(?:async\s+)?def\s+([A-Za-z0-9_]+)\s*\(/)
      if (pyDef) {
        symbols.push({ kind: rawLine.startsWith(' ') ? 'method' : 'function', name: pyDef[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
    }

    // 4. Go (.go)
    if (lowerExt === '.go') {
      const goType = trimmed.match(/^type\s+([A-Za-z0-9_]+)\s+(struct|interface)/)
      if (goType) {
        symbols.push({ kind: goType[2] === 'struct' ? 'struct' : 'interface', name: goType[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
      const goFunc = trimmed.match(/^func\s+(?:\([^)]+\)\s+)?([A-Za-z0-9_]+)\s*\(/)
      if (goFunc) {
        symbols.push({ kind: 'function', name: goFunc[1]!, line: lineNum, signature: trimmed.slice(0, 80) })
        continue
      }
    }

    // 5. JSON 顶层键
    if (lowerExt === '.json' && (rawLine.startsWith('  "') || rawLine.startsWith('\t"'))) {
      const jsonKey = rawLine.match(/^[\s\t]*"([^"]+)"\s*:/)
      if (jsonKey) {
        symbols.push({ kind: 'key', name: jsonKey[1]!, line: lineNum })
      }
    }
  }

  return symbols
}

export const codeOutlinePlugin: BuiltinPluginPackage = {
  id: 'code-outline',
  name: '代码大纲与符号导航 (code-outline)',
  description: '快速提取源码文件的符号大纲（类、函数、类型定义及所在行号），省去大文件全量读取消耗，极速定位代码。',
  tools: [
    (workspace: string) => ({
      name: 'get_outline',
      description: '快速提取指定源码文件的结构大纲（包含类、接口、函数、结构体、类型及行号）。优先于全量 read_file 调用，避免消耗过多 Token。',
      parameters: {
        type: 'object',
        properties: {
          path: {
            type: 'string',
            description: '源码文件的相对或绝对路径，如 src/agent/store.ts',
          },
        },
        required: ['path'],
      },
      async execute(_callId, args: { path: string }) {
        const root = workspace || process.cwd()
        const rawPath = String(args?.path || '').trim()
        if (!rawPath) return { output: '请提供目标文件路径。', ok: false }

        const targetFile = isAbsolute(rawPath) ? rawPath : join(root, rawPath)
        if (!existsSync(targetFile)) {
          return { output: `目标文件不存在: ${rawPath}`, ok: false }
        }

        try {
          const st = await stat(targetFile)
          if (st.isDirectory()) {
            return { output: `目标路径是一个目录而非文件: ${rawPath}`, ok: false }
          }

          const content = await readFile(targetFile, 'utf8')
          const lines = content.split('\n')
          const symbols = extractSymbols(content, extname(targetFile))

          const formattedSymbols = symbols.map((s) => {
            const sig = s.signature ? ` | ${s.signature}` : ''
            return `L${s.line.toString().padEnd(4, ' ')} [${s.kind}] ${s.name}${sig}`
          })

          const report = [
            `# 文件符号大纲: ${rawPath}`,
            `- 总行数: ${lines.length} 行`,
            `- 文件大小: ${Math.round(st.size / 1024)} KB`,
            `- 提取到主要符号: ${symbols.length} 个`,
            '',
            formattedSymbols.length > 0
              ? formattedSymbols.join('\n')
              : '（未识别到顶层符号定义，建议直接使用 read_file 查看）',
          ].join('\n')

          return { output: report, ok: true }
        } catch (err: any) {
          return { output: `解析文件大纲失败: ${err.message}`, ok: false }
        }
      },
    }),
  ],
  skills: [
    {
      name: 'code-navigation',
      description: '大文件代码快速导航策略：先查符号大纲，再精准定向读取与修改。',
      content: `---
name: code-navigation
description: 大文件代码快速导航策略：先查符号大纲，再精准定向读取与修改。
whenToUse: 当面对未知文件或超过 300 行的大型源文件时优先使用。
---

# 大文件精准高效导航法则

1. **先大纲后局部**：针对未知或大型文件，禁止盲目全量读取；先调用 \`get_outline\` 提取类、接口与关键方法行号；
2. **定向切片读取**：根据符号行号，使用 \`read_file\` 仅读取目标函数所在的几十行上下文；
3. **保持高命中**：这样能够大幅节省上下文 Token 预算，保障思考与回复的极速流畅。
`,
    },
  ],
  prompts: [
    {
      name: 'outline',
      description: '获取指定文件的符号与函数大纲',
      argumentHint: '<file-path>',
      content: `请调用 get_outline 工具提取 \${1:?必须指定文件路径} 的符号大纲，并简要概述该文件包含的核心功能模块与对外暴露的接口。`,
    },
  ],
}
