/**
 * Skill 大模型内置工具
 * 参考 ZCode / Claude Code 设计
 * 允许模型在主会话中按需加载并执行具体专业技能规范
 */

import type { AgentTool, AgentToolResult } from '../core/types'
import { defaultSkillManager, SkillManager } from './manager'

export function createSkillTool(manager: SkillManager = defaultSkillManager, workspaceRoot?: string): AgentTool {
  return {
    name: 'Skill',
    description: `在对话中按需加载并执行指定的专业技能规范（Skills）。
当用户请求执行特定领域任务，或通过 \`/<skill_name>\` 提及某技能时，请调用此工具加载其具体操作步骤。
参数：
- \`skill\`: 技能名称（不包含开头的斜杠，例如 'code-review'）
- \`args\`: 可选字符串参数`,
    parameters: {
      type: 'object',
      properties: {
        skill: {
          type: 'string',
          description: '要加载的技能名称（例如 code-review、sql-optimizer）',
        },
        args: {
          type: 'string',
          description: '传递给技能的可选参数',
        },
      },
      required: ['skill'],
    },
    async execute(_callId: string, args: Record<string, unknown>): Promise<AgentToolResult> {
      const skillName = typeof args?.skill === 'string' ? (args.skill as string).trim() : ''
      if (!skillName) {
        return {
          output: '请提供要加载的技能名称。',
          ok: false,
        }
      }

      const currentWorkspace = (args?.workspace as string | undefined) || workspaceRoot
      const loaded = await manager.loadSkillContent(skillName, currentWorkspace)

      if (!loaded) {
        const available = await manager.getEnabledSkills(currentWorkspace)
        const names = available.map((s) => s.name).join(', ')
        return {
          output: `未找到名为 "${skillName}" 的已启用技能。当前可用的技能有：${names || '暂无可用技能'}`,
          ok: false,
        }
      }

      const output = [
        `<skill_content name="${loaded.name}">`,
        `# Skill: ${loaded.name}`,
        '',
        loaded.content,
        '',
        `Base directory for this skill: ${loaded.baseDirectory}`,
        `File path: ${loaded.path}`,
        'Relative paths in this skill are relative to this base directory.',
        '</skill_content>',
      ].join('\n')

      return {
        output,
        ok: true,
      }
    },
  }
}
