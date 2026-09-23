/**
 * 内置辅助 Coding 插件：项目工程与依赖诊断 (project-inspector)
 */

import { existsSync } from 'node:fs'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import type { BuiltinPluginPackage } from './types'

const execFileAsync = promisify(execFile)

async function checkCmd(cmd: string): Promise<string | null> {
  try {
    const { stdout } = await execFileAsync(cmd, ['--version'], { windowsHide: true })
    return stdout.trim().split('\n')[0] || 'available'
  } catch {
    return null
  }
}

export const projectInspectorPlugin: BuiltinPluginPackage = {
  id: 'project-inspector',
  name: '项目工程与依赖诊断 (project-inspector)',
  description: '自动检测工作区的技术栈类型、包管理工具、可用 scripts 指令与关键依赖，一键生成诊断报告。',
  tools: [
    (workspace: string) => ({
      name: 'inspect_project',
      description: '探测当前项目的工程配置、技术栈、依赖包以及可执行的 npm/cargo/bun 命令，帮助迅速理解工程结构。',
      parameters: {
        type: 'object',
        properties: {},
      },
      async execute() {
        const root = workspace || process.cwd()
        const sections: string[] = [`# 项目工程与环境诊断报告: ${root}`]

        // 1. 检查 Node.js / Bun 生态 (package.json)
        const pkgJsonPath = join(root, 'package.json')
        if (existsSync(pkgJsonPath)) {
          try {
            const raw = await readFile(pkgJsonPath, 'utf8')
            const pkg = JSON.parse(raw)
            sections.push(`## Node / TypeScript 生态配置`)
            sections.push(`- **包名**: ${pkg.name || '(unnamed)'} (v${pkg.version || '0.0.0'})`)
            if (pkg.scripts && Object.keys(pkg.scripts).length > 0) {
              sections.push(`- **可用 Scripts 指令**:`)
              for (const [k, v] of Object.entries(pkg.scripts)) {
                sections.push(`  - \`${k}\`: ${v}`)
              }
            }
            const deps = Object.keys(pkg.dependencies || {})
            const devDeps = Object.keys(pkg.devDependencies || {})
            sections.push(`- **生产依赖**: ${deps.length} 个 ${deps.slice(0, 15).join(', ')}${deps.length > 15 ? '...' : ''}`)
            sections.push(`- **开发依赖**: ${devDeps.length} 个 ${devDeps.slice(0, 15).join(', ')}${devDeps.length > 15 ? '...' : ''}`)
          } catch (e: any) {
            sections.push(`读取 package.json 失败: ${e.message}`)
          }
        }

        // 2. 检查 Rust 生态 (Cargo.toml)
        const cargoPath = join(root, 'Cargo.toml')
        if (existsSync(cargoPath)) {
          try {
            const raw = await readFile(cargoPath, 'utf8')
            sections.push(`## Rust / Cargo 生态配置`)
            const pkgName = raw.match(/name\s*=\s*"([^"]+)"/)?.[1]
            const version = raw.match(/version\s*=\s*"([^"]+)"/)?.[1]
            if (pkgName) sections.push(`- **Crate 包名**: ${pkgName} (v${version || '0.1.0'})`)
            const isWorkspace = raw.includes('[workspace]')
            if (isWorkspace) sections.push(`- **工程模式**: Cargo Workspace 多包工作区`)
          } catch {}
        }

        // 3. 检查 Python 生态 (pyproject.toml / requirements.txt)
        const pyproj = join(root, 'pyproject.toml')
        const reqTxt = join(root, 'requirements.txt')
        if (existsSync(pyproj) || existsSync(reqTxt)) {
          sections.push(`## Python 生态配置`)
          if (existsSync(pyproj)) sections.push(`- 发现 pyproject.toml`)
          if (existsSync(reqTxt)) sections.push(`- 发现 requirements.txt`)
        }

        // 4. 运行时与 CLI 工具链版本探测
        const toolChecks = await Promise.all([
          checkCmd('bun').then((v) => ({ name: 'Bun', v })),
          checkCmd('node').then((v) => ({ name: 'Node.js', v })),
          checkCmd('cargo').then((v) => ({ name: 'Cargo', v })),
          checkCmd('git').then((v) => ({ name: 'Git', v })),
        ])

        sections.push(`## 本地环境工具链探测`)
        for (const t of toolChecks) {
          sections.push(`- **${t.name}**: ${t.v ? `已就绪 (${t.v})` : '未检测到或未加入 PATH'}`)
        }

        return { output: sections.join('\n\n'), ok: true }
      },
    }),
  ],
  skills: [
    {
      name: 'project-setup',
      description: '项目环境排查与技术栈诊断工作流。',
      content: `---
name: project-setup
description: 项目环境排查与技术栈诊断工作流。
whenToUse: 当首次进入新代码库或排查依赖构建问题时使用。
---

# 项目环境排查法则

1. **先探测后执行**：先调用 \`inspect_project\` 了解项目包管理器与 scripts 定义；
2. **保持包管理一致**：优先使用项目声明的包管理器（如优先使用 bun 或 pnpm）；
3. **针对性构建排错**：根据项目声明的 scripts 运行构建或校验。
`,
    },
  ],
  prompts: [
    {
      name: 'diagnose',
      description: '探测当前工程的技术栈、可用命令与本地环境',
      content: `请调用 inspect_project 工具全面诊断当前工程的技术栈配置，并从包管理、可用 scripts、依赖结构以及工具链状态给出整体概括与下一步开发建议。`,
    },
  ],
}
