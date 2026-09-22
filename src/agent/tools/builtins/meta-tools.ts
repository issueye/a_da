/**
 * 工具 CRUD 元开发管理工具 (manage_tool)
 * 在 create 模式下专属激活
 * 赋能智能体通过编写 TypeScript 扩展插件，自发创建、读取、更新、删除与列出自定义 Agent 工具
 */

import { existsSync } from 'node:fs'
import { readFile, writeFile } from 'node:fs/promises'
import type { AgentTool, AgentToolResult } from '../../core/types'
import { defaultExtensionLoader, ExtensionLoader } from '../loader'

export interface ManageToolArgs {
  action: 'create' | 'read' | 'update' | 'delete' | 'list'
  name?: string
  description?: string
  scope?: 'workspace' | 'global'
  code?: string
  workspace?: string
}

export function createManageTool(
  loader: ExtensionLoader = defaultExtensionLoader,
  workspaceRoot?: string
): AgentTool<ManageToolArgs> {
  return {
    name: 'manage_tool',
    label: '工具管理',
    description: `在 Create（创造）模式下管理与自扩展自定义 Agent 工具插件（基于 TypeScript 动态热加载）。
支持操作：
- create: 编写并创建全新自定义工具插件，系统自动编译、注册并热生效；
- read: 查看已有自定义工具的源码实现、参数描述与状态；
- update: 覆写并重新加载自定义工具源码；
- delete: 删除自定义工具插件并注销该工具；
- list: 列出当前所有已发现的自定义扩展工具。`,
    parameters: {
      type: 'object',
      properties: {
        action: {
          type: 'string',
          enum: ['create', 'read', 'update', 'delete', 'list'],
          description: '要执行的工具 CRUD 操作',
        },
        name: {
          type: 'string',
          description: '工具英文标识名称（如 git_stats, db_query）',
        },
        description: {
          type: 'string',
          description: '工具的功能描述与使用场景说明',
        },
        scope: {
          type: 'string',
          enum: ['workspace', 'global'],
          description: '作用域：workspace（项目内 .ada/extensions）或 global（全局 ~/.ada/extensions）',
        },
        code: {
          type: 'string',
          description: '自定义工具的完整 TypeScript 插件源码。需默认导出一个函数：export default function(context) { context.registerTool({ name, description, parameters, execute }) }',
        },
      },
      required: ['action'],
    },
    async execute(_callId: string, args: ManageToolArgs): Promise<AgentToolResult> {
      const action = args?.action
      const currentWorkspace = (args?.workspace as string | undefined) || workspaceRoot || process.cwd()

      if (!action) {
        return {
          output: '必须指定 action 操作类型 (create, read, update, delete, list)。',
          ok: false,
        }
      }

      try {
        switch (action) {
          case 'create': {
            const name = (args.name || '').trim()
            if (!name) {
              return { output: '创建工具失败：必须提供工具英文名称 (name)。', ok: false }
            }

            const scope = args.scope === 'global' ? 'global' : 'workspace'
            const filePath = await loader.createPluginTemplate(
              currentWorkspace,
              scope,
              name,
              args.code?.trim() || undefined
            )

            // 触发热加载
            await loader.autoLoadExtensions(currentWorkspace)

            return {
              output: `成功创建自定义工具插件 "${name}"！\n作用域: ${scope}\n文件路径: ${filePath}\n系统已完成热编译与注册，大模型现在可以直接调度该工具。`,
              ok: true,
              details: { name, scope, filePath },
            }
          }

          case 'read': {
            const name = (args.name || '').trim()
            if (!name) {
              return { output: '查看工具失败：必须提供工具名称 (name)。', ok: false }
            }

            const plugins = await loader.scanPlugins(currentWorkspace)
            const target = plugins.find(
              (p) =>
                p.name.toLowerCase() === name.toLowerCase() ||
                p.id.toLowerCase().includes(name.toLowerCase()) ||
                p.tools.some((t) => t.name.toLowerCase() === name.toLowerCase())
            )

            if (!target) {
              return {
                output: `未找到名为 "${name}" 的自定义工具插件。可通过 action: 'list' 查看所有已安装插件。`,
                ok: false,
              }
            }

            let sourceCode = ''
            if (existsSync(target.filePath)) {
              sourceCode = await readFile(target.filePath, 'utf8')
            }

            const output = [
              `# 自定义工具插件: ${target.name}`,
              `- 作用域: ${target.scope}`,
              `- 文件路径: ${target.filePath}`,
              `- 启用状态: ${target.enabled ? '已启用' : '已停用'}`,
              `- 导出工具数: ${target.tools.length} 个 (${target.tools.map((t) => t.name).join(', ')})`,
              target.error ? `- 编译警告/错误: ${target.error}` : '',
              '',
              '## TypeScript 插件源码',
              '```typescript',
              sourceCode,
              '```',
            ]
              .filter(Boolean)
              .join('\n')

            return {
              output,
              ok: true,
              details: target,
            }
          }

          case 'update': {
            const name = (args.name || '').trim()
            const code = (args.code || '').trim()
            if (!name) {
              return { output: '更新工具失败：必须提供工具名称 (name)。', ok: false }
            }
            if (!code) {
              return { output: '更新工具失败：必须提供新的 TypeScript 源码 (code)。', ok: false }
            }

            const plugins = await loader.scanPlugins(currentWorkspace)
            const target = plugins.find(
              (p) =>
                p.name.toLowerCase() === name.toLowerCase() ||
                p.id.toLowerCase().includes(name.toLowerCase()) ||
                p.tools.some((t) => t.name.toLowerCase() === name.toLowerCase())
            )

            if (!target) {
              return {
                output: `更新失败：未找到名为 "${name}" 的自定义插件。`,
                ok: false,
              }
            }

            await writeFile(target.filePath, code, 'utf8')
            await loader.autoLoadExtensions(currentWorkspace)

            return {
              output: `成功更新工具插件 "${name}" 源码！系统已完成重新热重载。`,
              ok: true,
              details: { name, filePath: target.filePath },
            }
          }

          case 'delete': {
            const name = (args.name || '').trim()
            if (!name) {
              return { output: '删除工具失败：必须提供工具名称 (name)。', ok: false }
            }

            const plugins = await loader.scanPlugins(currentWorkspace)
            const target = plugins.find(
              (p) =>
                p.name.toLowerCase() === name.toLowerCase() ||
                p.id.toLowerCase().includes(name.toLowerCase()) ||
                p.tools.some((t) => t.name.toLowerCase() === name.toLowerCase())
            )

            if (!target) {
              return {
                output: `删除失败：未找到名为 "${name}" 的自定义插件。`,
                ok: false,
              }
            }

            const ok = await loader.deletePlugin(target.filePath, currentWorkspace)
            if (!ok) {
              return { output: `删除插件文件失败: ${target.filePath}`, ok: false }
            }

            return {
              output: `成功删除工具插件 "${name}" 并注销相关工具。`,
              ok: true,
              details: { name },
            }
          }

          case 'list': {
            const plugins = await loader.scanPlugins(currentWorkspace)
            if (plugins.length === 0) {
              return {
                output: '当前工作区与全局用户目录尚未安装任何自定义扩展工具插件。',
                ok: true,
                details: [],
              }
            }

            const lines = [
              `当前已安装 ${plugins.length} 个工具扩展插件：`,
              ...plugins.map((p, idx) => {
                const toolsStr = p.tools.map((t) => t.name).join(', ') || '无导出工具'
                const stateStr = p.enabled ? '✓ 启用' : '✗ 停用'
                return `${idx + 1}. [${p.scope.toUpperCase()}] ${p.name} (${stateStr}) - 工具: [${toolsStr}] (文件: ${p.fileName})`
              }),
            ]

            return {
              output: lines.join('\n'),
              ok: true,
              details: plugins.map((p) => ({
                id: p.id,
                name: p.name,
                scope: p.scope,
                enabled: p.enabled,
                tools: p.tools.map((t) => t.name),
              })),
            }
          }

          default:
            return {
              output: `不支持的操作 action: ${(action as any)}。`,
              ok: false,
            }
        }
      } catch (err) {
        return {
          output: `工具管理操作异常: ${(err as Error).message}`,
          ok: false,
        }
      }
    },
  }
}
