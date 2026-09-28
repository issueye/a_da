/**
 * 插件加载器：把「内置插件的声明数组」与「工作区/全局目录里的 jiti 扩展」两条路径
 * 统一成同一个产物 {@link LoadedPlugin}。
 *
 * 设计依据：docs/plugin-system-design.md §4.3.2（一种产物、两个加载器）、§5.1-§5.5。
 *
 * 加载顺序与优先级：内置（`builtin:`）→ 工作区（`workspace:`）→ 全局（`global:`），
 * 后加载的同名工具覆盖先加载的（覆盖会被记进 `ToolRegistry.getConflicts()`）。
 * 声明了 `dependsOn` 的插件排在依赖之后；依赖缺失或未启用则标记 `broken` 且**不注册
 * 它的工具**——半个插件比没有插件更难查。
 */

import { createJiti } from 'jiti'
import {
  existsSync,
  readdirSync,
  statSync,
  mkdirSync,
  writeFileSync,
  rmSync,
  unlinkSync,
  type Dirent,
} from 'node:fs'
import { basename, join } from 'node:path'
import type { AgentEvent, AgentTool } from '../core/types'
import {
  createPluginDisabledResolver,
  readPluginConfig,
  readPluginSecret,
  setPluginDisabled,
} from '../config'
import { getAppHome } from '../home'
import { defaultPromptManager, type PromptItem } from '../prompts'
import { defaultSkillManager, type SkillSummary } from '../skills'
import { APP_VERSION, satisfiesRange } from '../version'
import { BUILTIN_PLUGINS } from './builtin-plugins'
import { getPluginDiagnostics, setLoadedPlugins } from '../plugins/registry'
import type {
  LoadedPlugin,
  PluginConfigSchema,
  PluginContributions,
  PluginDescriptorExport,
  PluginDiagnostic,
  PluginManifest,
  PluginPrompt,
  PluginScope,
  PluginSkill,
  PluginStatus,
  PluginToolFactory,
} from '../plugins/types'
import type { AgentHooks } from '../core/events'
import { defaultToolRegistry, type ToolConflict } from './registry'

/**
 * 传递给扩展插件的完整上下文 API
 */
export interface ExtensionContext {
  /** 当前项目工作区根目录 */
  workspace: string
  /** 向系统调试日志面板写入自定义追踪埋点 */
  trace: (message: string) => void
  /** 注册自定义 Agent 工具 */
  registerTool: (tool: AgentTool) => void
  /** 订阅 Agent 生命周期事件点位 (agent_start, turn_end, tool_execution_* 等) */
  onEvent: (listener: (event: AgentEvent) => void) => () => void
  /**
   * 注册**可干预**的钩子（设计文档 §6.1）。
   *
   * 与 `onEvent` 刻意分开：`onEvent` 只读观察、丢弃返回值；这里注册的钩子返回值会
   * 改变控制流（收窄工具集、注入消息、改写系统提示词、阻止工具调用）。分开声明，
   * 用户与审查者一眼能看出这个插件是"会动手"的。
   *
   * 受能力开关约束：`allowThirdPartyHooks` 关掉后第三方注册的钩子不生效，
   * plan 模式下看 `allowPlanModeHooks`；钩子里返回的工具只能收窄。
   */
  registerHooks: (hooks: AgentHooks) => void
}

export type ExtensionFunction = (
  ctx: ExtensionContext
) =>
  | void
  | Promise<void>
  | AgentTool
  | PluginToolFactory
  | PluginDescriptorExport
  | Array<AgentTool | PluginToolFactory>

/**
 * 扩展模块的导出形态。
 *
 * 两种写法都支持，产出的 `LoadedPlugin` 完全一样：
 *
 * 1. **声明式（首选）**：`export default { name, description, tools: [...] }`
 * 2. **函数式**：`export default (ctx) => { ctx.registerTool(...); ctx.onEvent(...) }`
 *
 * 模块级命名导出（`export const tools = [...]` 等）会与 default 的声明合并。
 */
export interface ExtensionModule {
  tool?: PluginToolFactory
  tools?: PluginToolFactory[]
  skills?: PluginSkill[]
  prompts?: PluginPrompt[]
  configSchema?: PluginConfigSchema
  dependsOn?: string[]
  version?: string
  author?: string
  engines?: { a_da?: string }
  hooks?: AgentHooks
  default?:
    | PluginToolFactory
    | ExtensionFunction
    | PluginDescriptorExport
    | Array<AgentTool | PluginToolFactory>
}

export interface PluginToolInfo {
  name: string
  description: string
  parameters?: Record<string, unknown>
  isWrite: boolean
}

/**
 * 插件管理页用的展示形状：**加载契约（{@link LoadedPlugin}）的投影 + 展示用字段**。
 *
 * `plugin` 是权威来源，其余平铺字段由它派生（skill/prompt 那两项除外——它们来自
 * 技能与提示词管理器的扫描结果，是"归纳后的条目"，不是插件自己声明的原文）。
 * 平铺保留是为了不动现有界面与测试；M3 的插件卡增强可以直接改读 `plugin`。
 */
export interface PluginItem {
  plugin: LoadedPlugin
  id: string
  name: string
  fileName: string
  filePath: string
  scope: PluginScope
  enabled: boolean
  status: PluginStatus
  version?: string
  diagnostics: PluginDiagnostic[]
  tools: PluginToolInfo[]
  /** 插件包内包含的技能列表（将 SKILL 归纳到插件系统中） */
  skills: SkillSummary[]
  /** 插件包内包含的提示词列表（将提示词归纳到插件系统中） */
  prompts: PromptItem[]
  /** 是否为复合插件包目录（包含 skills/、prompts/ 或独立子目录） */
  isPackage?: boolean
  /** 加载失败原文与 error 级诊断的汇总（插件管理页已有展示位，M3 再细分） */
  error?: string
  sizeBytes: number
  updatedAt: number
}

/** 加载一个插件时收集到的注册请求（工具、事件监听器与钩子）。 */
interface RegistrationSink {
  tools: AgentTool[]
  listeners: Set<(event: AgentEvent) => void>
  hooks: AgentHooks[]
}

/** 解析一个插件来源之后、判定状态之前的中间形态。 */
interface PluginCandidate {
  manifest: PluginManifest
  contributions: PluginContributions
  /** 声明式来源（内置）为 true；jiti 执行出来的第三方为 false */
  declarative: boolean
  /** 解析阶段就发现的问题（工具形态不对、模块抛错等） */
  diagnostics: PluginDiagnostic[]
  listeners: Set<(event: AgentEvent) => void>
  /** 加载抛错的原文 */
  loadError?: string
  source: {
    fileName: string
    filePath: string
    sizeBytes: number
    updatedAt: number
    isPackage: boolean
  }
}
/**
 * 合并同一个插件给出的多份钩子声明：描述符里的在前，`registerHooks` 注册的在后。
 *
 * 同一个点位只保留**最后一次**声明——一个插件对同一点位有两种意见是插件自己的 bug，
 * 与其猜它想要哪个，不如按"后写的覆盖先写的"这个通用规则走（与工具名冲突同一取向）。
 */
function mergeHooks(
  declared: AgentHooks | undefined,
  registered: AgentHooks[]
): AgentHooks | undefined {
  const merged: AgentHooks = { ...(declared ?? {}) }
  for (const hooks of registered) Object.assign(merged, hooks)
  return Object.keys(merged).length > 0 ? merged : undefined
}

/**
 * 判断一个导出值是不是插件描述符。
 *
 * 关键是与「工具」区分开：`AgentTool` 必有 `execute`，而描述符必有至少一个贡献字段。
 * 只凭 `'name' in value` 判断（早先的写法）会把描述符误当成工具——描述符也有 `name`
 * （插件显示名），结果是既不注册工具也不报错，插件"装上了却没生效"。
 */
function looksLikeDescriptor(value: unknown): value is PluginDescriptorExport {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  if (typeof (value as { execute?: unknown }).execute === 'function') return false
  return (['tools', 'skills', 'prompts', 'configSchema', 'dependsOn'] as const).some(
    (key) => key in (value as Record<string, unknown>)
  )
}

function isTool(value: unknown): value is AgentTool {
  return (
    !!value &&
    typeof value === 'object' &&
    typeof (value as AgentTool).name === 'string' &&
    typeof (value as AgentTool).execute === 'function'
  )
}

function isFactory(value: unknown): value is (workspace: string) => AgentTool {
  return typeof value === 'function'
}

/**
 * 从描述符里挑出属于 `PluginManifest` 的字段。
 *
 * 不能整个展开：描述符里还有 tools/skills/prompts/configSchema/dependsOn，那些属于
 * contributions，混进 manifest 会让"元信息"和"能力"两个字段组失去意义。
 */
function manifestFieldsOf(descriptor: PluginDescriptorExport): Partial<PluginManifest> {
  return {
    id: descriptor.id,
    name: descriptor.name,
    description: descriptor.description,
    version: descriptor.version,
    author: descriptor.author,
    scope: descriptor.scope,
    engines: descriptor.engines,
  }
}

/**
 * 按依赖关系排序：被依赖者先加载，后注册者的工具覆盖前者。
 *
 * 环（A 依赖 B、B 依赖 A）不拒绝加载，只是原序保留并把环里的 id 报出来——
 * 这类声明错误更可能是笔误，让插件照常可用、同时在诊断里说清楚更合适。
 */
function orderByDependencies(candidates: PluginCandidate[]): {
  ordered: PluginCandidate[]
  cyclic: string[]
} {
  const byId = new Map(candidates.map((item) => [item.manifest.id, item]))
  const state = new Map<string, 'visiting' | 'done'>()
  const ordered: PluginCandidate[] = []
  const cyclic: string[] = []

  const visit = (candidate: PluginCandidate): void => {
    const id = candidate.manifest.id
    const seen = state.get(id)
    if (seen === 'done') return
    if (seen === 'visiting') {
      cyclic.push(id)
      return
    }
    state.set(id, 'visiting')
    for (const dependency of candidate.contributions.dependsOn ?? []) {
      const target = byId.get(dependency)
      if (target) visit(target)
    }
    state.set(id, 'done')
    ordered.push(candidate)
  }

  for (const candidate of candidates) visit(candidate)
  return { ordered, cyclic }
}

export class ExtensionLoader {
  private jitiInstance = createJiti(import.meta.url)
  /**
   * 事件监听器**按插件分组**持有。
   *
   * 早先是一个扁平 Set，重载只清工具不清监听器——`onEvent` 订阅的回调会一轮一轮
   * 累积，每轮事件都要跑一遍所有历史版本的回调（设计文档 §3 缺陷 4）。
   */
  private eventListeners = new Map<string, Set<(event: AgentEvent) => void>>()
  private traceHandler?: (msg: string) => void

  /**
   * 绑定宿主环境的打点与事件分发管道
   */
  bindHost(trace: (msg: string) => void) {
    this.traceHandler = trace
  }

  /**
   * 将 Agent 运行时产生的生命周期事件派发给所有扩展脚本
   */
  dispatchAgentEvent(event: AgentEvent) {
    for (const listeners of this.eventListeners.values()) {
      for (const listener of listeners) {
        try {
          listener(event)
        } catch (err) {
          console.error('[Extension Event Error]', err)
        }
      }
    }
  }

  /** 退订事件监听器。不给 pluginId 就清空全部（重载前调用）。 */
  clearListeners(pluginId?: string): void {
    if (pluginId === undefined) this.eventListeners.clear()
    else this.eventListeners.delete(pluginId)
  }

  /** 当前订阅了事件的插件 id 与各自的监听器数量，用于测试与诊断。 */
  getListenerStats(): Record<string, number> {
    const stats: Record<string, number> = {}
    for (const [pluginId, listeners] of this.eventListeners) {
      if (listeners.size > 0) stats[pluginId] = listeners.size
    }
    return stats
  }

  /**
   * 全部加载诊断（error 在前）。数据来自最近一次 `autoLoadExtensions` 写进插件索引
   * 的结果；`scanPlugins` 只做展示投影，不覆盖它。
   */
  async getDiagnostics(): Promise<PluginDiagnostic[]> {
    return getPluginDiagnostics()
  }

  private trace(message: string): void {
    if (this.traceHandler) this.traceHandler(`[插件] ${message}`)
  }

  /** 造一个给插件用的上下文：把工具、监听器与钩子收进 sink，由加载器统一落位。 */
  private createContext(workspace: string, pluginId: string, sink: RegistrationSink): ExtensionContext {
    return {
      workspace,
      trace: (msg: string) => this.trace(`${pluginId} ${msg}`),
      registerTool: (tool: AgentTool) => {
        sink.tools.push(tool)
      },
      onEvent: (listener: (event: AgentEvent) => void) => {
        sink.listeners.add(listener)
        return () => {
          sink.listeners.delete(listener)
          this.eventListeners.get(pluginId)?.delete(listener)
        }
      },
      registerHooks: (hooks: AgentHooks) => {
        sink.hooks.push(hooks)
      },
    }
  }

  /**
   * 解析一个扩展模块，抽出贡献与描述符。
   *
   * 合并规则：`default` 里的声明优先于模块级命名导出（同名工具都保留，由注册表
   * 按名字定胜负并记冲突）；技能与提示词按名字去重，`default` 优先。
   */
  private async parseExtensionModule(
    mod: ExtensionModule,
    context: ExtensionContext,
    pluginId: string
  ): Promise<{
    descriptor: PluginDescriptorExport
    diagnostics: PluginDiagnostic[]
    initError?: Error
  }> {
    const diagnostics: PluginDiagnostic[] = []
    const tools: AgentTool[] = []
    const skills: PluginSkill[] = []
    const prompts: PluginPrompt[] = []
    let descriptor: PluginDescriptorExport = {}

    const say = (level: PluginDiagnostic['level'], message: string, hint?: string) => {
      diagnostics.push({ pluginId, level, message, hint })
    }

    /** 收集一个值：工具实例 / 工厂 / 描述符 / 数组，各自归位。 */
    const absorb = (value: unknown, where: string): void => {
      if (value === undefined || value === null) return

      if (Array.isArray(value)) {
        for (const item of value) absorb(item, where)
        return
      }

      if (typeof value === 'function') {
        let produced: unknown
        try {
          produced = value(context.workspace)
        } catch (err) {
          say('error', `${where} 里的工具工厂抛错：${(err as Error).message}`)
          return
        }
        absorb(produced, where)
        return
      }

      if (looksLikeDescriptor(value)) {
        mergeDescriptor(value)
        return
      }

      if (isTool(value)) {
        tools.push(value)
        return
      }

      // 走到这里说明导出的是个对象却不是工具（没有 execute）——早先会被当成工具
      // 塞进工具表，模型一调就炸。现在明确报出来。
      say(
        'warn',
        `${where} 导出的对象既不是工具也不是插件描述符`,
        '工具需要有 execute 函数；只想声明能力就用 export default { tools: [...] }'
      )
    }

    const mergeDescriptor = (incoming: PluginDescriptorExport): void => {
      const { tools: incomingTools, skills: incomingSkills, prompts: incomingPrompts, ...rest } = incoming
      descriptor = { ...descriptor, ...rest }
      if (incomingTools) absorb(incomingTools, 'descriptor.tools')
      if (incomingSkills) {
        for (const skill of incomingSkills) {
          if (!skills.some((item) => item.name === skill.name)) skills.push(skill)
        }
      }
      if (incomingPrompts) {
        for (const prompt of incomingPrompts) {
          if (!prompts.some((item) => item.name === prompt.name)) prompts.push(prompt)
        }
      }
    }

    // 1. default：描述符 / 函数 / 工具 / 数组，四种都可能
    let initError: Error | undefined
    if (mod.default !== undefined) {
      if (typeof mod.default === 'function') {
        let produced: unknown
        try {
          produced = await (mod.default as ExtensionFunction)(context)
        } catch (err) {
          // 初始化就抛错：这个插件基本是废的，交给状态判定标成 broken
          initError = err as Error
          say('error', `扩展初始化抛错：${initError.message}`)
          produced = undefined
        }
        absorb(produced, 'default()')
      } else {
        absorb(mod.default, 'default')
      }
    }

    // 2. 模块级命名导出（与 default 合并）
    for (const named of [mod.tool, ...(mod.tools ?? [])]) absorb(named, 'export tools')
    if (mod.skills) for (const skill of mod.skills) if (!skills.some((s) => s.name === skill.name)) skills.push(skill)
    if (mod.prompts) for (const prompt of mod.prompts) if (!prompts.some((p) => p.name === prompt.name)) prompts.push(prompt)
    if (mod.configSchema) descriptor.configSchema = mod.configSchema
    if (mod.dependsOn) descriptor.dependsOn = mod.dependsOn
    if (mod.version) descriptor.version = mod.version
    if (mod.author) descriptor.author = mod.author
    if (mod.engines) descriptor.engines = mod.engines
    if (mod.hooks) descriptor.hooks = mod.hooks

    if (tools.length > 0) descriptor.tools = [...(descriptor.tools ?? []), ...tools]
    if (skills.length > 0) descriptor.skills = skills
    if (prompts.length > 0) descriptor.prompts = prompts

    return { descriptor, diagnostics, initError }
  }

  /** 内置插件 → 候选（纯数据，不需要执行任何代码）。 */
  private buildBuiltinCandidates(workspace: string): PluginCandidate[] {
    return BUILTIN_PLUGINS.map((plugin) => {
      const id = `builtin:${plugin.id}`
      const tools = plugin.tools.map((entry) => (isFactory(entry) ? entry(workspace) : entry))
      return {
        manifest: {
          id,
          name: plugin.name,
          description: plugin.description,
          scope: 'builtin' as PluginScope,
          version: plugin.version,
          author: plugin.author,
          engines: plugin.engines,
        },
        contributions: {
          tools,
          skills: plugin.skills,
          prompts: plugin.prompts,
          configSchema: plugin.configSchema,
          dependsOn: plugin.dependsOn,
          hooks: plugin.hooks,
        },
        declarative: true,
        diagnostics: [],
        listeners: new Set(),
        source: {          fileName: `${plugin.id} (内置)`,
          filePath: `(builtin):${plugin.id}`,
          sizeBytes: 0,
          updatedAt: Date.now(),
          isPackage: true,
        },
      }
    })
  }

  /** 扫描一个目录下的全部扩展，产出候选（**不注册**任何东西，也不过滤停用项）。 */
  private async scanDirCandidates(
    dirPath: string,
    workspace: string,
    scope: 'workspace' | 'global'
  ): Promise<PluginCandidate[]> {
    if (!existsSync(dirPath)) return []
    let entries: Dirent[] = []
    try {
      entries = readdirSync(dirPath, { withFileTypes: true })
    } catch {
      return []
    }

    const candidates: PluginCandidate[] = []
    for (const entry of entries) {
      if (entry.name.startsWith('.') || entry.name === 'node_modules') continue
      const fullPath = join(dirPath, entry.name)
      const id = `${scope}:${entry.name}`

      let sizeBytes = 0
      let updatedAt = Date.now()
      try {
        const st = statSync(fullPath)
        sizeBytes = st.size
        updatedAt = st.mtimeMs
      } catch {}

      // 单文件插件与复合插件包目录：入口候选一致
      let scriptToLoad: string | null = null
      let isPackage = false
      if (entry.isFile() && (entry.name.endsWith('.ts') || entry.name.endsWith('.js'))) {
        scriptToLoad = fullPath
      } else if (entry.isDirectory()) {
        isPackage = true
        scriptToLoad =
          [
            join(fullPath, 'index.ts'),
            join(fullPath, 'index.js'),
            join(fullPath, 'tools.ts'),
            join(fullPath, 'tool.ts'),
            join(fullPath, `${entry.name}.ts`),
          ].find((file) => existsSync(file)) ?? null
      } else {
        continue
      }

      // 没有任何入口脚本的插件包（纯技能/提示词目录）也要能被看到
      const fallbackName = isPackage ? entry.name : basename(entry.name).replace(/\.[^.]+$/, '')
      const sink: RegistrationSink = { tools: [], listeners: new Set(), hooks: [] }
      const candidate: PluginCandidate = {
        manifest: {
          id,
          name: fallbackName,
          description: '',
          scope,
        },
        contributions: {},
        declarative: false,
        diagnostics: [],
        listeners: sink.listeners,
        source: {
          fileName: entry.name,
          filePath: scriptToLoad ?? fullPath,
          sizeBytes,
          updatedAt,
          isPackage,
        },
      }

      if (scriptToLoad) {
        const context = this.createContext(workspace, id, sink)
        try {
          const mod = (await this.jitiInstance.import(scriptToLoad)) as ExtensionModule
          const { descriptor, diagnostics, initError } = await this.parseExtensionModule(mod, context, id)
          candidate.diagnostics.push(...diagnostics)
          // 目录与文件名给出身份：描述符可以改名、改描述，但 **id 一律由加载器决定**
          // （id 是启停表、诊断、工具溯源的键，允许插件自报会让它们对不上）。
          candidate.manifest = {
            ...candidate.manifest,
            ...manifestFieldsOf(descriptor),
            id,
            scope,
            name: descriptor.name?.trim() || fallbackName,
          }
          // 函数形态的 ctx.registerTool 与声明式的 tools 都是来源，按名字合并
          const merged = new Map<string, AgentTool>()
          for (const tool of [...sink.tools, ...((descriptor.tools as AgentTool[] | undefined) ?? [])]) {
            merged.set(tool.name, tool)
          }
          candidate.contributions = {
            tools: Array.from(merged.values()),
            skills: descriptor.skills,
            prompts: descriptor.prompts,
            configSchema: descriptor.configSchema,
            dependsOn: descriptor.dependsOn,
            hooks: mergeHooks(descriptor.hooks, sink.hooks),
          }
          if (initError) candidate.loadError = initError.message
        } catch (err) {
          const error = err as Error
          candidate.loadError = error.message
          candidate.diagnostics.push({
            pluginId: id,
            level: 'error',
            message: `加载失败：${error.message}`,
            hint: '检查文件语法、依赖是否能解析',
          })
        }
      }

      candidates.push(candidate)
    }

    return candidates
  }

  /**
   * 判定每个插件的状态并补上诊断。
   *
   * 顺序：broken > not-ready > incompatible > conflict > ready——一个插件同时缺失
   * 依赖和版本不匹配时，最严重的那条决定它显示成什么，诊断列表里则一条不少。
   *
   * `availableIds` 是**可依赖**的插件集合（启用中的那些）：被停用的插件等同于缺失，
   * 否则用户关掉一个插件、依赖它的另一个照常跑，问题会在调用时才炸出来。
   */
  private async finalizePlugins(
    candidates: PluginCandidate[],
    conflicts: ToolConflict[],
    cyclic: string[],
    availableIds: Set<string>
  ): Promise<LoadedPlugin[]> {
    const loaded: LoadedPlugin[] = []

    for (const candidate of candidates) {
      const id = candidate.manifest.id
      const diagnostics = [...candidate.diagnostics]
      let status: PluginStatus = 'ready'

      if (candidate.loadError) {
        status = 'broken'
      }

      // 依赖：缺失与"装了但没启用"分开说清楚——后者用户自己能修
      for (const dependency of candidate.contributions.dependsOn ?? []) {
        if (availableIds.has(dependency)) continue
        status = 'broken'
        diagnostics.push({
          pluginId: id,
          level: 'error',
          message: `依赖的插件「${dependency}」不可用`,
          hint: '装好并在插件管理里启用它；被停用的插件等同于缺失',
        })
      }

      if (cyclic.includes(id)) {
        diagnostics.push({
          pluginId: id,
          level: 'warn',
          message: '依赖关系成环',
          hint: '把 dependsOn 改成单向的；成环时按声明顺序加载',
        })
      }

      // 版本：不匹配只警告、照常加载（软失败）；看不懂的范围也照常加载
      const range = candidate.manifest.engines?.a_da
      if (range) {
        const verdict = satisfiesRange(APP_VERSION, range)
        if (verdict === false) {
          if (status === 'ready') status = 'incompatible'
          diagnostics.push({
            pluginId: id,
            level: 'warn',
            message: `声明的兼容版本 ${range} 与当前应用 ${APP_VERSION} 不匹配`,
            hint: '插件仍会加载；行为可能异常，必要时联系插件作者',
          })
        } else if (verdict === 'unparsable') {
          diagnostics.push({
            pluginId: id,
            level: 'warn',
            message: `无法解析版本范围「${range}」，已按兼容处理`,
            hint: '用 1.2.3 / >=1.2.3 / ^1.2.3 / ~1.2.3 这类写法',
          })
        }
      }

      // 必填配置：缺失 → not-ready 且不注册工具
      const schema = candidate.contributions.configSchema
      if (schema) {
        const missing = await this.missingRequiredConfig(id, schema)
        if (missing.length > 0) {
          if (status === 'ready' || status === 'incompatible') status = 'not-ready'
          diagnostics.push({
            pluginId: id,
            level: 'error',
            message: `缺少必填配置：${missing.join('、')}`,
            hint: `在设置里补上；也可以写进 config.json 的 pluginConfig.${id}，或设 A_DA_PLUGIN_${id.toUpperCase()}_<KEY>`,
          })
        }
      }

      // 工具名冲突：谁覆盖了谁，双方都标出来
      const tools = (candidate.contributions.tools ?? []) as AgentTool[]
      for (const conflict of conflicts) {
        if (!tools.some((tool) => tool.name === conflict.name)) continue
        if (status === 'ready') status = 'conflict'
        const over = conflict.shadowedBuiltin
          ? '核心内置工具'
          : `插件「${conflict.shadowedPluginId ?? '未知'}」`
        if (conflict.pluginId === id) {
          diagnostics.push({
            pluginId: id,
            level: 'warn',
            message: `工具「${conflict.name}」覆盖了${over}的同名工具`,
            hint: '换个工具名可以避免模型混淆；当前是后注册者生效',
          })
        } else if (conflict.shadowedPluginId === id) {
          diagnostics.push({
            pluginId: id,
            level: 'warn',
            message: `工具「${conflict.name}」被插件「${conflict.pluginId ?? '未知'}」的同名工具覆盖`,
            hint: '换个工具名，或停用其中一个插件',
          })
        }
      }

      // broken / not-ready 的插件既不注册工具，也**不接管任何决策点**：一个缺配置的
      // 插件每轮都来干预工具表，比它干脆不出现更难查（诊断里已经说明原因）
      const usable = status !== 'broken' && status !== 'not-ready'

      loaded.push({
        manifest: candidate.manifest,
        contributions: usable
          ? candidate.contributions
          : { ...candidate.contributions, hooks: undefined },
        declarative: candidate.declarative,
        status,
        diagnostics,
      })
    }

    return loaded
  }

  /**
   * 列出缺失的必填配置项。
   *
   * 密钥类（`type: 'secret'`）按约定不在 config.json 里，用 `readPluginSecret` 查
   * 密钥文件——否则一个已经配好密钥的插件会被永远判成"缺少配置"。
   */
  private async missingRequiredConfig(
    pluginId: string,
    schema: PluginConfigSchema
  ): Promise<string[]> {
    const required = Object.entries(schema.properties).filter(([, property]) => property.required)
    if (required.length === 0) return []

    const merged = await readPluginConfig<Record<string, unknown>>(pluginId)
    const missing: string[] = []
    for (const [key, property] of required) {
      if (property.type === 'secret') {
        if (!readPluginSecret(pluginId, key)) missing.push(key)
        continue
      }
      const value = merged[key] ?? property.default
      const empty =
        value === undefined ||
        value === null ||
        (typeof value === 'string' && value.trim() === '')
      if (empty) missing.push(key)
    }
    return missing
  }

  /**
   * 把判定结果落位：注册工具（带来源）、订阅监听器。
   *
   * `broken` 与 `not-ready` 的插件**不注册工具、不订阅监听器**：一个缺配置的插件
   * 把工具塞进工具表，模型会调到一半失败，比它干脆不出现更难查。
   */
  private applyPlugins(
    loaded: LoadedPlugin[],
    candidates: PluginCandidate[],
    workspace: string
  ): string[] {
    const byId = new Map(candidates.map((item) => [item.manifest.id, item]))
    const names: string[] = []

    for (const plugin of loaded) {
      const id = plugin.manifest.id
      const candidate = byId.get(id)
      const usable = plugin.status !== 'broken' && plugin.status !== 'not-ready'

      if (usable) {
        for (const entry of plugin.contributions.tools ?? []) {
          // 描述符里直接写工厂也要能用：`tools: [(workspace) => createTool(workspace)]`
          const tool = isFactory(entry) ? entry(workspace) : entry
          defaultToolRegistry.register(tool, { pluginId: id, scope: plugin.manifest.scope })
          names.push(tool.name)
        }
        if (candidate) {
          this.eventListeners.set(id, new Set(candidate.listeners))
        }
      }

      for (const diagnostic of plugin.diagnostics) {
        this.trace(`${id} [${diagnostic.level}] ${diagnostic.message}`)
      }
      if (plugin.status !== 'ready') {
        this.trace(`${id} 状态：${plugin.status}（诊断 ${plugin.diagnostics.length} 条）`)
      }
      // 诊断是给界面与 `getDiagnostics()` 用的结构化渠道；`console.warn` 保留一份，
      // 因为脚本与测试里没有事件日志面板可看（设计文档 §3 缺陷 3 的要求）。
      if (plugin.status === 'broken' || plugin.status === 'not-ready') {
        const first = plugin.diagnostics.find((item) => item.level === 'error')
        console.warn(
          `[ExtensionLoader] 插件 ${id} 未生效（${plugin.status}）：${first?.message ?? '无诊断'}`
        )
      }
    }

    // 工具表里同名只留一份，返回值（用于"已加载扩展工具"这类日志）也跟着去重：
    // 两个插件抢同一个名字是**冲突诊断**要说的事，不该在日志里显示成"加载了两条"。
    return Array.from(new Set(names))
  }

  /** 把候选按依赖排序、判定状态，再落位。 */
  private async loadCandidates(
    candidates: PluginCandidate[],
    workspace: string
  ): Promise<{ plugins: LoadedPlugin[]; names: string[] }> {
    const { ordered, cyclic } = orderByDependencies(candidates)
    const availableIds = new Set(candidates.map((item) => item.manifest.id))
    const plugins = await this.finalizePlugins(
      ordered,
      defaultToolRegistry.getConflicts(),
      cyclic,
      availableIds
    )
    const names = this.applyPlugins(plugins, ordered, workspace)
    return { plugins, names }
  }

  /** 打印一行加载摘要，并把有问题的插件单独点名。 */
  private reportLoad(plugins: LoadedPlugin[]): void {
    if (plugins.length === 0) return
    const troubled = plugins.filter((plugin) => plugin.status !== 'ready')
    const ready = plugins.length - troubled.length
    this.trace(`已加载 ${plugins.length} 个插件（就绪 ${ready}，异常 ${troubled.length}）`)
    for (const plugin of troubled) {
      const first = plugin.diagnostics.find((item) => item.level === 'error') ?? plugin.diagnostics[0]
      this.trace(`${plugin.manifest.id} 处于 ${plugin.status}：${first?.message ?? '（无诊断）'}`)
    }
  }

  /**
   * 扫描工作区与全局目录的所有扩展插件元数据（包含单文件插件与复合插件包）。
   *
   * 只做**展示投影**，不注册工具、不写插件索引（索引属于最近一次真实加载）。
   * 状态与诊断用与加载同一套判定逻辑算出来，所以界面上看到的与真实生效的一致。
   */
  async scanPlugins(workspace: string): Promise<PluginItem[]> {
    const isDisabled = await createPluginDisabledResolver(workspace)
    const allSkills = await defaultSkillManager.scanSkills(workspace)
    const allPrompts = await defaultPromptManager.scanPrompts(workspace)

    // 展示要**含被停用**的插件——用户得能看到它并把它打开
    const candidates = [
      ...this.buildBuiltinCandidates(workspace),
      ...(await this.scanDirCandidates(
        join(workspace, '.ada', 'extensions'),
        workspace,
        'workspace'
      )),
      ...(await this.scanDirCandidates(join(getAppHome(), 'extensions'), workspace, 'global')),
    ]

    // 状态判定与真实加载用同一套逻辑，依赖集合也一致（只看启用中的那些）
    const availableIds = new Set(
      candidates.filter((item) => !isDisabled(item.manifest.id)).map((item) => item.manifest.id)
    )
    const { ordered, cyclic } = orderByDependencies(candidates)
    const loaded = await this.finalizePlugins(
      ordered,
      defaultToolRegistry.getConflicts(),
      cyclic,
      availableIds
    )
    const loadedById = new Map(loaded.map((plugin) => [plugin.manifest.id, plugin]))

    const items: PluginItem[] = []
    for (const candidate of candidates) {
      const id = candidate.manifest.id
      const plugin = loadedById.get(id)!
      const tools = ((plugin.contributions.tools ?? []) as AgentTool[]).map((tool) => ({
        name: tool.name,
        description: tool.description,
        parameters: tool.parameters as Record<string, unknown> | undefined,
        isWrite: defaultToolRegistry.isWriteTool(tool.name),
      }))
      const matchingSkills = allSkills.filter(
        (skill) =>
          skill.scope === 'plugin' &&
          (skill.pluginId === id || skill.pluginName === plugin.manifest.name)
      )
      const matchingPrompts = allPrompts.filter(
        (prompt) =>
          prompt.scope === 'plugin' &&
          (prompt.pluginId === id || prompt.pluginName === plugin.manifest.name)
      )

      const errors = plugin.diagnostics.filter((item) => item.level === 'error')
      items.push({
        plugin,
        id,
        name: plugin.manifest.name,
        fileName: candidate.source.fileName,
        filePath: candidate.source.filePath,
        scope: plugin.manifest.scope ?? 'workspace',
        enabled: !isDisabled(id),
        status: plugin.status,
        version: plugin.manifest.version,
        diagnostics: plugin.diagnostics,
        tools,
        skills: matchingSkills,
        prompts: matchingPrompts,
        isPackage: candidate.source.isPackage,
        error:
          candidate.loadError ??
          (errors.length > 0 ? errors.map((item) => item.message).join('；') : undefined),
        sizeBytes: candidate.source.sizeBytes,
        updatedAt: candidate.source.updatedAt,
      })
    }

    return items
  }

  /**
   * 从指定目录动态扫描并加载 TypeScript/JavaScript 扩展模块并注册工具。
   *
   * 独立调用（测试、脚本）用这个；应用走 {@link autoLoadExtensions}，那条路径会把
   * 内置、工作区、全局一起排序判定，并写插件索引。
   */
  async loadExtensionsFromDir(
    dirPath: string,
    workspace: string,
    disabledSet: Set<string> = new Set(),
    scope: 'workspace' | 'global' = 'workspace'
  ): Promise<string[]> {
    const candidates = (await this.scanDirCandidates(dirPath, workspace, scope)).filter(
      (item) => !disabledSet.has(item.manifest.id)
    )
    const { names } = await this.loadCandidates(candidates, workspace)
    return names
  }

  /**
   * 自动重新加载工作区及全局未禁用的扩展插件（应用入口）。
   */
  async autoLoadExtensions(workspace: string): Promise<string[]> {
    defaultToolRegistry.clearCustomTools()
    // 工具与监听器一起清：只清工具会让 onEvent 订阅一轮一轮累积（缺陷 4）
    this.clearListeners()

    const isDisabled = await createPluginDisabledResolver(workspace)
    const candidates = [
      ...this.buildBuiltinCandidates(workspace),
      ...(await this.scanDirCandidates(
        join(workspace, '.ada', 'extensions'),
        workspace,
        'workspace'
      )),
      ...(await this.scanDirCandidates(join(getAppHome(), 'extensions'), workspace, 'global')),
    ].filter((item) => !isDisabled(item.manifest.id))

    const { plugins, names } = await this.loadCandidates(candidates, workspace)
    setLoadedPlugins(plugins)
    this.reportLoad(plugins)
    return names
  }

  /**
   * 切换插件的启用/禁用状态并持久化。
   *
   * 写的是**全局**停用表：插件管理页的开关是应用级偏好，"这个项目里关掉"属于
   * 工作区级的覆盖（`workspacePluginState`），M3 在界面上给用户选，M1 先把两侧的
   * 读写 API 落定（见 `config.ts` 的 `setPluginDisabled`）。
   */
  async togglePlugin(pluginId: string, enabled: boolean, workspace: string): Promise<void> {
    await setPluginDisabled(pluginId, !enabled)
    await this.autoLoadExtensions(workspace)
  }

  /**
   * 快速创建一个扩展插件模板
   */
  async createPluginTemplate(
    workspace: string,
    scope: 'workspace' | 'global',
    rawName: string,
    customCode?: string
  ): Promise<string> {
    const cleanName = rawName.trim().replace(/\.[^.]+$/, '').replace(/[^a-zA-Z0-9_-]/g, '_')
    const fileName = `${cleanName || 'my_custom_tool'}.ts`
    const targetDir =
      scope === 'workspace'
        ? join(workspace, '.ada', 'extensions')
        : join(getAppHome(), 'extensions')

    mkdirSync(targetDir, { recursive: true })
    const fullPath = join(targetDir, fileName)

    const toolIdentifier = cleanName.replace(/-/g, '_')
    const code = customCode?.trim() || `/**
 * 扩展工具: ${cleanName}
 *
 * 声明式写法：一个纯数据对象，加载器直接读它，不需要执行任何初始化代码。
 * 需要订阅事件或按工作区动态建工具时，改成 \`export default (context) => { ... }\`
 * 并在里面调用 \`context.registerTool(...)\` / \`context.onEvent(...)\`。
 */
export default {
  name: '${cleanName}',
  description: '自定义扩展工具 ${cleanName}',
  tools: [
    {
      name: '${toolIdentifier}',
      label: '${cleanName}',
      description: '这是自定义扩展工具 ${cleanName} 的功能描述',
      parameters: {
        type: 'object',
        properties: {
          query: {
            type: 'string',
            description: '输入查询参数',
          },
        },
        required: ['query'],
      },
      async execute(callId, args) {
        // 可以在这里执行网络请求、文件操作或系统调用
        return {
          output: \`扩展 [${cleanName}] 成功处理输入: \${args.query}\`,
          ok: true,
        }
      },
    },
  ],
}
`
    writeFileSync(fullPath, code, 'utf8')
    await this.autoLoadExtensions(workspace)
    return fullPath
  }

  /**
   * 删除指定的插件文件并重新加载
   */
  async deletePlugin(filePath: string, workspace: string): Promise<boolean> {
    try {
      if (existsSync(filePath)) {
        const st = statSync(filePath)
        if (st.isDirectory()) {
          rmSync(filePath, { recursive: true, force: true })
        } else {
          unlinkSync(filePath)
        }
        await this.autoLoadExtensions(workspace)
        return true
      }
    } catch (err) {
      console.warn(`[ExtensionLoader] 删除插件失败:`, err)
    }
    return false
  }
}

export const defaultExtensionLoader = new ExtensionLoader()
