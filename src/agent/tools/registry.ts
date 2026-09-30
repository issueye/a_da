/**
 * 工具注册中心
 * 参考 @earendil-works/pi-coding-agent 统一工具管理
 */

import type { AgentTool } from '../core/types'
import type { PluginScope } from '../plugins/types'
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

/** 工具从哪来。`pluginId` 缺省表示核心自身注册（目前没有这种工具，留作区分）。 */
export interface ToolOrigin {
  pluginId?: string
  scope?: PluginScope
}

/** 一次注册把已在同一名字上的工具覆盖掉了。 */
export interface ToolConflict {
  name: string
  /** 后注册者（胜出的一方）的插件 id */
  pluginId?: string
  /** 被覆盖者的插件 id；被覆盖者是核心内置工具时为 undefined */
  shadowedPluginId?: string
  /** 被覆盖的是核心内置工具——比插件之间互相覆盖更值得警告 */
  shadowedBuiltin: boolean
}

interface RegisteredTool {
  tool: AgentTool
  origin: ToolOrigin
}

export class ToolRegistry {
  private customTools = new Map<string, RegisteredTool>()
  private conflicts: ToolConflict[] = []

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
    // 向用户提问只是挂起等待回答，不碰工作区或系统状态；plan 阶段正是最需要澄清的时候
    'ask_user',
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
   * 注册自定义/扩展工具。
   *
   * 同名覆盖的规则：**后注册者胜**，但覆盖必须被看见——返回值与
   * {@link getConflicts} 都会带上冲突信息，加载器据此产生插件诊断，插件管理页
   * 就能显示"谁遮蔽了谁"。遮蔽**核心内置工具**会额外 `console.warn`：这类冲突
   * 最容易被误当成"内置工具坏了"。
   *
   * 这里只记录、不拒绝。是否允许遮蔽内置是 M2 的能力开关
   * （`allowBuiltinShadow`，默认开）的事，加载层先把事实摆出来。
   */
  register(tool: AgentTool, origin: ToolOrigin = {}): ToolConflict | undefined {
    const previous = this.customTools.get(tool.name)
    const shadowsBuiltin = !previous && this.isBuiltinToolName(tool.name)
    this.customTools.set(tool.name, { tool, origin })

    let conflict: ToolConflict | undefined
    if (previous) {
      conflict = {
        name: tool.name,
        pluginId: origin.pluginId,
        shadowedPluginId: previous.origin.pluginId,
        shadowedBuiltin: false,
      }
    } else if (shadowsBuiltin) {
      conflict = { name: tool.name, pluginId: origin.pluginId, shadowedBuiltin: true }
    }

    // 同名工具只保留最新那条冲突：重载后旧记录会先被 clearCustomTools 清掉，
    // 这里再按名字去重，避免反复注册同一个名字累积出一串历史。
    this.conflicts = this.conflicts.filter((item) => item.name !== tool.name)
    if (conflict) {
      this.conflicts.push(conflict)
      const from = origin.pluginId ?? '(未知来源)'
      const over = previous
        ? `插件 ${previous.origin.pluginId ?? '(未知来源)'}`
        : '核心内置工具'
      console.warn(`[ToolRegistry] 工具名冲突：「${from}」的 "${tool.name}" 覆盖了${over}`)
    }
    return conflict
  }

  /** 取消注册指定工具 */
  unregister(name: string): void {
    this.customTools.delete(name)
    this.conflicts = this.conflicts.filter((item) => item.name !== name)
  }

  /** 清空所有自定义/扩展工具 */
  clearCustomTools(): void {
    this.customTools.clear()
    this.conflicts = []
  }

  /**
   * 清空指定插件注册的工具。
   *
   * 按插件重载（而不是整表清空）时需要它：`clearCustomTools` 会把内置插件的工具
   * 一起清掉，而内置插件并不需要重新执行。
   */
  unregisterPlugin(pluginId: string): void {
    for (const [name, entry] of this.customTools) {
      if (entry.origin.pluginId === pluginId) {
        this.customTools.delete(name)
        this.conflicts = this.conflicts.filter((item) => item.name !== name)
      }
    }
  }

  /** 获取当前所有已注册的自定义工具 */
  getCustomTools(): AgentTool[] {
    return Array.from(this.customTools.values()).map((entry) => entry.tool)
  }

  /** 某个工具是哪个插件注册的；核心内置工具（未注册进插件）返回 undefined。 */
  getToolOrigin(name: string): ToolOrigin | undefined {
    return this.customTools.get(name)?.origin
  }

  /** 某个插件注册的全部工具名，按注册顺序。 */
  listByPlugin(pluginId: string): string[] {
    const names: string[] = []
    for (const [name, entry] of this.customTools) {
      if (entry.origin.pluginId === pluginId) names.push(name)
    }
    return names
  }

  /** 当前存在的工具名冲突（重载会重新计算）。 */
  getConflicts(): ToolConflict[] {
    return [...this.conflicts]
  }

  /**
   * 这个名字是否为**核心内置工具**。
   *
   * 判据是展示目录 `BUILTIN_TOOLS_CATALOG`——它收录的正是 `getToolsForWorkspace`
   * 与 create 模式注入的那批名字，且只收录它们（插件工具一律不在其中）。
   * `equivalence.test.ts` 有一条测试钉住"目录里的名字都真实注册"，所以这份判据
   * 不会悄悄漂移。
   *
   * 公开出去是给**加载层**用的：`allowBuiltinShadow` 关闭时要靠它判断
   * "这个插件工具是不是在占核心内置工具的名字"（见 `loader.ts` 的 `finalizePlugins`）。
   */
  isBuiltinToolName(name: string): boolean {
    return BUILTIN_TOOLS_CATALOG.some((item) => item.name === name)
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
      // 传 workspace 与 threadId：任务清单的钩子要靠它们定位上一次是什么
      createTodoTool(workspace, options?.parentThreadId),
      createSkillTool(undefined, workspace),
      createSubagentTool(workspace, options?.parentThreadId),
      createCheckSubagentTool(),
      createSendSubagentMessageTool(),
      createResumeSubagentTool(),
      createAwaitSubagentsTool(workspace, options?.parentThreadId),
      // 注意：notify_parent 刻意不在这里。它只对「作为子智能体运行」的身份有意义，
      // 由 store 在建子智能体工具表时单独追加（见 startSubagentThread）。
    ]

    // 按名字合并，**不出现同名两份**：内置工具先占位，插件工具覆盖同名项。
    // 之前这里是"内置一批 + 插件一批"直接拼接，一个名叫 read_file 的插件工具
    // 会让工具表里出现两条同名记录——模型看到两条，审批按名字判断，都会乱。
    const byName = new Map<string, AgentTool>()
    for (const tool of builtins) byName.set(tool.name, tool)
    for (const entry of this.customTools.values()) byName.set(entry.tool.name, entry.tool)

    return Array.from(byName.values())
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
   *
   * **分类要连"谁注册的"一起看**，不能只看名字。`READ_ONLY` 是名字级的名单，而插件
   * 可以借走内置工具的名字（`allowBuiltinShadow` 默认开）。若只看名字，一个叫
   * `read_file` 的插件工具就会被判成只读，于是 plan 模式放行、readonly 审批档不问、
   * 只读子智能体也拿得到——审批闸门的依据（见 `docs/plugin-system-design.md` §6.4.3）
   * 与 `afterTurn.effectiveToolNames` 那份回执会一起失真。
   *
   * 因此规则是：**非内置插件顶着只读名字注册的工具，一律按写处理**。官方内置插件的
   * 只读工具（`git_status` / `get_outline` 等，scope 为 `builtin`）不受影响；第三方
   * 真正的只读工具本来就不在名单里、按写处理——这与本方法的失败安全取向一致。
   */
  isWriteTool(name: string): boolean {
    if (!ToolRegistry.READ_ONLY.has(name)) return true
    const origin = this.customTools.get(name)?.origin
    // scope 缺失（有人不走插件路径直接注册）也按写处理，同样是失败安全。
    return origin !== undefined && origin.scope !== 'builtin'
  }
}

export const defaultToolRegistry = new ToolRegistry()

/**
 * `BuiltinToolInfo` 的形状已搬到契约层 `src/shared/protocol`（协议设计 §7.1）：
 * 插件管理页的「内置工具」页签要跨进程拿到它。这里原样再导出。
 */
import type { BuiltinToolInfo } from '../../shared/protocol'
export type { BuiltinToolInfo }

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
  { name: 'resume_subagent', label: '恢复子智能体工作', description: '恢复被中断的子智能体，让它从上次的状态与上下文继续推进', isReadOnly: true },
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
