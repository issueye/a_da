/**
 * SKILL 系统类型定义
 * 参考 ZCode / Claude Code 技能体系标准
 */

export type SkillScope = 'builtin' | 'workspace' | 'global' | 'plugin'

export interface SkillMetadata {
  name: string
  description: string
  version?: string
  author?: string
  tags?: string[]
  whenToUse?: string
  disallowedTools?: string[]
}

export interface SkillSummary {
  /** 唯一标识符：例如 workspace:code-reviewer, global:git-commit, plugin:web-search:crawler */
  id: string
  /** 技能英文短名称（用于命令/工具参数调用，例如 code-reviewer） */
  name: string
  /** 技能功能描述与使用场景说明（用于注入模型提示词） */
  description: string
  /** 技能指令正文（SKILL.md 正文 Markdown 内容） */
  body: string
  /** 技能根文件 SKILL.md 的绝对路径 */
  path: string
  /** 技能所在的根目录 */
  baseDirectory: string
  /** 作用域：工作区、用户全局、插件 */
  scope: SkillScope
  /** 是否启用 */
  enabled: boolean
  /** 若由插件引入，则记录插件名 */
  pluginName?: string
  /** 附加元数据 */
  metadata?: SkillMetadata
}

export interface SkillsPromptContext {
  /** 注入到大模型系统提示词中的可用技能摘要段 */
  prompt: string
  /** 当前已激活的技能名称列表 */
  activatedSkillNames: string[]
}

export interface SkillDiagnostic {
  severity: 'warning' | 'error'
  message: string
  path?: string
  skillName?: string
}
