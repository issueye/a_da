/**
 * SKILL CRUD 元开发管理工具 (manage_skill)
 * 在 create 模式下专属激活
 * 赋能智能体通过自然语言自发创建、读取、更新、删除与列出技能规范 (SKILL.md)
 */

import type { AgentTool, AgentToolResult } from '../core/types'
import { defaultSkillManager, SkillManager } from './manager'

export interface ManageSkillArgs {
  action: 'create' | 'read' | 'update' | 'delete' | 'list'
  name?: string
  description?: string
  whenToUse?: string
  scope?: 'workspace' | 'global'
  body?: string
  workspace?: string
}

export function createManageSkillTool(
  manager: SkillManager = defaultSkillManager,
  workspaceRoot?: string
): AgentTool<ManageSkillArgs> {
  return {
    name: 'manage_skill',
    label: '技能管理',
    description: `在 Create（创造）模式下管理与自扩展技能规范（SKILL.md）。
支持操作：
- create: 创建全新技能规范，自动生成标准 YAML Frontmatter 与 Markdown 指令；
- read: 查看指定技能的完整元数据与正文；
- update: 修改已有技能的描述、触发时机或正文；
- delete: 删除指定工作区或全局自定义技能；
- list: 列出当前已发现的全部技能及其启停状态。`,
    parameters: {
      type: 'object',
      properties: {
        action: {
          type: 'string',
          enum: ['create', 'read', 'update', 'delete', 'list'],
          description: '要执行的 CRUD 操作',
        },
        name: {
          type: 'string',
          description: '技能标识短名称（仅支持小写字母、数字与连字符，如 git-commit）',
        },
        description: {
          type: 'string',
          description: '技能的功能简介与价值说明',
        },
        whenToUse: {
          type: 'string',
          description: '何时应触发调用此技能的场景描述',
        },
        scope: {
          type: 'string',
          enum: ['workspace', 'global'],
          description: '保存作用域：workspace（工作区 .ada/skills）或 global（全局 ~/.ada/skills）',
        },
        body: {
          type: 'string',
          description: '技能的 Markdown 正文指令步骤',
        },
      },
      required: ['action'],
    },
    async execute(_callId: string, args: ManageSkillArgs): Promise<AgentToolResult> {
      const action = args?.action
      const currentWorkspace = (args?.workspace as string | undefined) || workspaceRoot

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
            const desc = (args.description || '').trim()
            if (!name) {
              return { output: '创建技能失败：必须提供技能名称 (name)。', ok: false }
            }
            if (!desc) {
              return { output: '创建技能失败：必须提供技能功能描述 (description)。', ok: false }
            }

            const scope = args.scope === 'global' ? 'global' : 'workspace'
            const filePath = await manager.createSkillTemplate({
              name,
              description: desc,
              scope,
              workspaceRoot: currentWorkspace,
              body: args.body?.trim() || undefined,
            })

            return {
              output: `成功创建技能 "${name}"！\n作用域: ${scope}\n文件路径: ${filePath}\n系统已完成热重载并纳入可用技能库。`,
              ok: true,
              details: { name, scope, filePath },
            }
          }

          case 'read': {
            const name = (args.name || '').trim()
            if (!name) {
              return { output: '查看技能失败：必须提供技能名称 (name)。', ok: false }
            }
            const all = await manager.scanSkills(currentWorkspace)
            const target = all.find(
              (s) => s.name.toLowerCase() === name.toLowerCase() || s.id === name
            )
            if (!target) {
              return {
                output: `未找到名为 "${name}" 的技能规范。可通过 action: 'list' 查询所有可用技能。`,
                ok: false,
              }
            }

            const output = [
              `# 技能信息: ${target.name}`,
              `- 作用域: ${target.scope}`,
              `- 启用状态: ${target.enabled ? '已启用' : '已停用'}`,
              `- 描述: ${target.description}`,
              target.metadata?.whenToUse ? `- 适用时机: ${target.metadata.whenToUse}` : '',
              `- 文件路径: ${target.path}`,
              '',
              '## Markdown 指令正文',
              target.body,
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
            if (!name) {
              return { output: '更新技能失败：必须提供技能名称 (name)。', ok: false }
            }

            const filePath = await manager.updateSkill(
              name,
              {
                description: args.description,
                body: args.body,
                whenToUse: args.whenToUse,
              },
              currentWorkspace
            )

            return {
              output: `成功更新技能 "${name}"！\n更新文件: ${filePath}`,
              ok: true,
              details: { name, filePath },
            }
          }

          case 'delete': {
            const name = (args.name || '').trim()
            if (!name) {
              return { output: '删除技能失败：必须提供技能名称 (name)。', ok: false }
            }

            await manager.deleteSkill(name, currentWorkspace)
            return {
              output: `成功删除技能 "${name}" 并从系统技能库中注销。`,
              ok: true,
              details: { name },
            }
          }

          case 'list': {
            const all = await manager.scanSkills(currentWorkspace)
            if (all.length === 0) {
              return { output: '当前尚未发现任何技能规范。', ok: true, details: [] }
            }

            const lines = [
              `当前已发现 ${all.length} 个技能规范：`,
              ...all.map((s, idx) => {
                const scopeTag = `[${s.scope.toUpperCase()}]`
                const stateTag = s.enabled ? '✓' : '✗'
                return `${idx + 1}. ${scopeTag} ${s.name} (${stateTag}) - ${s.description}`
              }),
            ]

            return {
              output: lines.join('\n'),
              ok: true,
              details: all.map((s) => ({
                id: s.id,
                name: s.name,
                scope: s.scope,
                enabled: s.enabled,
                description: s.description,
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
          output: `技能管理操作失败: ${(err as Error).message}`,
          ok: false,
        }
      }
    },
  }
}
