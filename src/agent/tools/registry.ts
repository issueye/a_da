/**
 * 工具注册中心
 * 参考 @earendil-works/pi-coding-agent 统一工具管理
 */

import type { AgentTool } from '../core/types'
import { createBashTool } from './builtins/bash'
import { createEditTool } from './builtins/edit'
import { createListTool } from './builtins/list'
import { createReadTool } from './builtins/read'
import { createSearchTool } from './builtins/search'
import { createWriteTool } from './builtins/write'
import { createTodoTool } from './builtins/todo'
import {
  createSubagentTool,
  createCheckSubagentTool,
  createSendSubagentMessageTool,
  createResumeSubagentTool,
  createAwaitSubagentsTool,
} from './builtins/subagent'
import { createSkillTool, createManageSkillTool } from '../skills'
import { createReadUrlTool } from './builtins/read-url'
import { createManageTool } from './builtins/meta-tools'
import { createRunBackgroundTool, createCheckTaskTool, createKillTaskTool } from './builtins/background'
import { createFindSymbolTool } from './builtins/symbols'

export class ToolRegistry {
  private customTools = new Map<string, AgentTool>()

  /** 明确只读的内置工具。名字不在这里的一律按「会改动工作区」处理。 */
  private static readonly READ_ONLY = new Set([
    'list_files',
    'read_file',
    'search_files',
    'todo',
    'read_url_content',
    'invoke_subagent',
    'check_subagent',
    'send_subagent_message',
    'resume_subagent',
    // 等待子智能体只是挂在内存里等唤醒，不碰工作区
    'await_subagents',
    'notify_parent',
    'Skill',
    'skill',
    // 查看后台任务只读状态与输出，不产生任何写副作用
    'check_task',
    // 符号索引只做内存扫描与查询
    'find_symbol',
    // 批量读取与 list_files 同类，纯只读（批量修改是另一个工具 edit_files）
    'read_files',
    // 官方插件里真正只读的那几个：git 状态/diff/历史都只查不改（git_status 不执行
    // 任何写子命令）；get_outline 只读单个文件提取符号；inspect_project 只读清单文件
    // 并探测工具链版本。它们没有副作用，只读子智能体理当能用——不列在这里的话会被
    // 当成写工具被 mode 过滤器挡掉。注意 run_test_focused 不在此列：它执行测试命令，
    // 可能触发构建产物与临时文件，按写工具对待。
    'git_status',
    'git_diff',
    'git_log',
    'get_outline',
    'inspect_project',
    // 决策工具：只发模型请求与读 git diff，不改工作区。check_gate 虽然读 diff，
    // 但只是读——与 run_test_focused 不同，它不执行任何命令产生副作用。
    'decide',
    'design_decision',
    'check_gate',
  ])

  /**
   * 注册自定义/扩展工具
   */
  register(tool: AgentTool): void {
    this.customTools.set(tool.name, tool)
  }

  /**
   * 取消注册指定工具
   */
  unregister(name: string): void {
    this.customTools.delete(name)
  }

  /**
   * 清空所有自定义/扩展工具
   */
  clearCustomTools(): void {
    this.customTools.clear()
  }

  /**
   * 获取当前所有已注册的自定义工具
   */
  getCustomTools(): AgentTool[] {
    return Array.from(this.customTools.values())
  }

  /**
   * 创建适用于指定工作区的所有工具列表 (包含内置基础工具 + 已注册扩展工具)
   */
  getToolsForWorkspace(workspace: string, options?: { parentThreadId?: string }): AgentTool[] {
    const builtins: AgentTool[] = [
      createListTool(workspace),
      createReadTool(workspace),
      createSearchTool(workspace),
      createReadUrlTool(),
      createWriteTool(workspace),
      createEditTool(workspace),
      createBashTool(workspace),
      createRunBackgroundTool(workspace),
      createCheckTaskTool(),
      createKillTaskTool(),
      createFindSymbolTool(workspace),
      createTodoTool(),
      createSkillTool(undefined, workspace),
      createSubagentTool(workspace, options?.parentThreadId),
      createCheckSubagentTool(),
      createSendSubagentMessageTool(),
      createResumeSubagentTool(),
      createAwaitSubagentsTool(workspace, options?.parentThreadId),
      // 注意：notify_parent 刻意不在这里。它只对「作为子智能体运行」的身份有意义，
      // 由 store 在建子智能体工具表时单独追加（见 startSubagentThread）。
    ]

    const all = [...builtins]
    for (const custom of this.customTools.values()) {
      all.push(custom)
    }

    return all
  }

  /**
   * 根据当前会话协作模式 (code / plan / create) 获取精准适配的工具集
   */
  getToolsForMode(
    workspace: string,
    mode: 'code' | 'plan' | 'create' = 'code',
    options?: { parentThreadId?: string }
  ): AgentTool[] {
    const all = this.getToolsForWorkspace(workspace, options)

    if (mode === 'plan') {
      // 规划模式：仅提供只读与调研分析工具，过滤直接写文件与命令执行
      return all.filter((t) => !this.isWriteTool(t.name))
    }

    if (mode === 'create') {
      // 创造模式：激活元开发能力，注入工具 CRUD 与技能 CRUD 工具
      return [
        ...all,
        createManageTool(undefined, workspace),
        createManageSkillTool(undefined, workspace),
      ]
    }

    // 编码模式 (code)：常规全能敏捷编码工具
    return all
  }

  /**
   * 是否为产生写副作用的工具。
   *
   * 失败安全：只有列在白名单里的只读工具算安全，其余（含扩展注册的工具）都要
   * 在「只读」模式下走审批。扩展是工作区里的第三方代码，不能默认它无害。
   */
  isWriteTool(name: string): boolean {
    return !ToolRegistry.READ_ONLY.has(name)
  }
}

export const defaultToolRegistry = new ToolRegistry()

export interface BuiltinToolInfo {
  name: string
  label: string
  description: string
  isReadOnly: boolean
}

/**
 * 核心内置工具的展示目录（标签 + 说明 + 是否只读）。
 *
 * 只收录 `getToolsForWorkspace` 里那批**由引擎内置托管**的核心工具——插件提供的
 * 工具不在这里，它们从各自的 `PluginDescriptor` 读（见 README「扩展」与
 * `src/agent/plugins/types.ts`）。这份目录用于插件管理页的「内置工具」清单：
 * 那里需要在不实例化工具的前提下展示标签与说明。
 */
export const BUILTIN_TOOLS_CATALOG: BuiltinToolInfo[] = [
  { name: 'list_files', label: '列出文件', description: '遍历并列出指定目录下的文件与子目录结构', isReadOnly: true },
  { name: 'read_file', label: '读取文件', description: '安全读取工作区内的代码或文本文件内容', isReadOnly: true },
  { name: 'search_files', label: '搜索文件', description: '在工作区文件中快速全局搜索指定文本或模式', isReadOnly: true },
  { name: 'find_symbol', label: '查找符号', description: '按名字查找函数/类/结构体等定义的位置与签名', isReadOnly: true },
  { name: 'read_url_content', label: '读取网页', description: '抓取技术文档与开源库链接内容并提取为 Markdown', isReadOnly: true },
  { name: 'todo', label: '任务清单', description: '管理多步骤编码任务的进度与状态', isReadOnly: true },
  { name: 'Skill', label: '加载技能', description: '按需加载专业技能规范与操作流程指南（SKILL.md）', isReadOnly: true },
  { name: 'invoke_subagent', label: '委派子智能体', description: '委派专项任务给隔离运行的专用子智能体', isReadOnly: true },
  { name: 'check_subagent', label: '查询子智能体', description: '查询异步子智能体的运行状态与总结报告', isReadOnly: true },
  { name: 'send_subagent_message', label: '智能体通讯', description: '向子智能体发送消息以动态纠偏或唤醒续跑', isReadOnly: true },
  { name: 'await_subagents', label: '等待子智能体', description: '挂起等待子智能体送回结论，替代反复轮询查询', isReadOnly: true },
  { name: 'notify_parent', label: '唤醒上级智能体', description: '子智能体把结论或待决策问题送回主智能体（仅子智能体可用）', isReadOnly: true },
  { name: 'write_file', label: '写入文件', description: '在工作区创建新文件或覆盖已有文件', isReadOnly: false },
  { name: 'edit_file', label: '编辑文件', description: '通过精准替换文本修改已有代码文件', isReadOnly: false },
  { name: 'run_command', label: '执行命令', description: '在项目工作区根目录下执行终端命令', isReadOnly: false },
  { name: 'run_background', label: '后台命令', description: '后台启动长运行命令（dev server 等），立即返回任务 id', isReadOnly: false },
  { name: 'check_task', label: '查看后台任务', description: '查询后台任务的状态与输出', isReadOnly: true },
  { name: 'kill_task', label: '停止后台任务', description: '终止后台任务及其子进程', isReadOnly: false },
  { name: 'manage_tool', label: '工具管理', description: '在 Create 模式下自发编写、更新与管理工具扩展插件', isReadOnly: false },
  { name: 'manage_skill', label: '技能管理', description: '在 Create 模式下自发创建、更新与管理技能规范 (SKILL.md)', isReadOnly: false },
]
