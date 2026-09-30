# 插件系统完善方案

> 目标：**把 a_da 的插件系统做完善**——让「写插件」这件事有清晰的契约、可组合的能力、
> 可靠的加载语义、可诊断的失败、以及可被用户管理的界面。
> 目标项目：a_da
> 状态：**设计待评审**，尚未实现
> 相关：`docs/decision-plugin-design.md`（决策插件）、`docs/core-extension-capabilities-design.md`
> （本文件的前一版，偏「补对照表差异」；本版改为以插件系统为中心，前一版中仍适用的
> 轮次拦截 / 工具档位 / 子智能体 gate 内容已并入本文对应章节）

---

## 1. 结论摘要

对照 pi-jev 得到的六行差异，本质不是「a_da 少了六个功能」，而是**插件系统的契约太薄**：
`BuiltinPluginPackage` 只有 6 个字段、`ExtensionContext` 只有 4 个成员。能力少 → 插件只能
是「一堆被动工具」→ 于是轮次拦截、gate、路由这些看起来像"缺功能"的东西，全都做不了。

所以本方案不按对照表逐行补，而是按**插件系统的四个层次**重建：

| 层次 | 现状 | 目标 |
|---|---|---|
| **契约层**：插件能声明什么 | 6 字段 / 4 成员 | 工具 + 技能 + 提示词 + 子智能体 + 钩子 + 配置声明 + 元信息 |
| **运行时层**：插件能干预什么 | 只能注册工具；事件只读且丢弃返回值；点位不成对且不开放 | 只读事件（保持）+ **成对的可干预点位**（带能力边界） |
| **加载层**：插件怎么被装进来 | 静默覆盖、静默失败、无溯源、无版本 | 显式冲突策略、可见失败、溯源、兼容检查、可重载 |
| **管理层**：用户能做什么 | 全局启停 + 删除 | 按工作区启停、配置、密钥、诊断、冲突提示 |

运行时层有一条贯穿性设计原则，单独在 §6.0 展开：

> **凡是「可以有事前事后的点位，都必须成对存在，并且在插件系统里都能提供。**」

对照现状，工具调用是**唯一**成对的点位，且**没有开放给插件**；轮次、整轮、子智能体
统统只有单侧（或只有只读事件）。这条原则同时解决三个实际问题——插件无法自我校准
（没有事后回执）、无法清理事前资源、无法审计（事前放行 + 事后记录才成轨迹）。

第二条贯穿性原则在 §6.4 展开——**开放与用户决定**：

> **点位开放给插件后，用什么、放开到什么程度，由用户决定，不由核心代拍。**

本方案早期版本在这一点上写的是大量"默认收紧"（第三方不能改文本、不能阻止删会话、
不能替换系统提示词、plan 模式不生效、gate 默认 fail-close 等）。这些**核心替用户做的
安全判断已全部改为可配置开关，默认值统一取"开放"**（§6.4.1、§6.4.2）。
核心只保留两件事：**如实呈现**每个开关的后果、**忠实执行**用户的设置。

唯一不可配置的只剩一条：**钩子返回值不能扩张工具集**（§6.4.3）——
因为工具集是审批闸门的依据，放开它等于绕过审批机制本身。这条保留的理由是
"放开会破坏机制自身可信性"，而不是"危险所以禁止"。

第三条前提见 §4.3——**本项目尚未正式使用，可以调整式改动**：

> 没有外部用户、没有已发布的插件生态、没有需要保住的历史配置，因此**不做兼容层，
> 直接改到目标形态**。类型改名、契约统一、`config.json` 结构、决策插件的配置读取
> 都不留后路。

省掉的只是**别名、迁移代码、双读路径**；测试反而更重——用一次性**等价性验证**
（改名前后的行为对照、新配置结构的解析一致）替代长期的兼容性负担。

本文同时**修掉盘点中发现的若干真实缺陷**（§3），它们比新功能更能体现「完善」：

1. 同名插件工具**静默覆盖**，早注册的被丢掉，无任何提示（`registry.ts:72-74`）。
2. 自定义工具与内置工具同名时，工具数组里出现**两份**同名工具一起发给模型（`registry.ts:124-127`）。
3. 插件加载失败只 `console.warn`，用户在运行时不被告知（`loader.ts:408`）。
4. 事件监听器存在永不清理的 `Set` 里，重载后**旧监听器残留**（`loader.ts:74`）。
5. 注册表**不记录工具来自哪个插件**，「按插件禁用工具」无从实现（`registry.ts:72-74`）。
6. 停用状态是**全局的**（单个 `~/.a-da/config.json`），一个工作区停用会影响所有工作区。
7. `.ts` 单文件扩展**无法贡献技能与提示词**（`ExtensionModule` 没有这两个字段）。
8. 子智能体循环**不派发生命周期事件**给插件（`store.ts:1842-1861`）。
9. **静态元数据与运行时注册会漂移**：`BUILTIN_TOOLS_METADATA`（`registry.ts:201-203`）
   与 `BUILTIN_PLUGINS` 的真实注册结果是两份独立的手写来源，没有一致性检查
   （详见 §7.2；按 §4.3 可直接删除这份冗余元数据，见 §4.3.1）。

---

## 2. 现状盘点：插件系统的真实边界

### 2.1 契约层

**内置插件** `BuiltinPluginPackage`（`src/agent/tools/builtin-plugins/types.ts:10-33`）：
`id` / `name` / `description` / `tools` / `skills?` / `prompts?` —— 就这 6 个。

**第三方扩展** `ExtensionContext`（`src/agent/tools/loader.ts:21-30`）：
```ts
export interface ExtensionContext {
  workspace: string
  trace: (message: string) => void
  registerTool: (tool: AgentTool) => void
  onEvent: (listener: (event: AgentEvent) => void) => () => void
}
```
以及 `ExtensionModule`（`loader.ts:36-44`）：`tool?` / `tools?` / `default?`。

**两套契约不互通**：内置插件是声明式数据（编译进应用），第三方是命令式代码（jiti 执行）。
内置插件甚至拿不到 `onEvent`，第三方拿不到 skills/prompts 字段。

### 2.2 工具作者的真实写法

以 `git-tools.ts` 为范本（`builtin-plugins/git-tools.ts`）：
```ts
export const gitToolsPlugin: BuiltinPluginPackage = {
  id: 'git-tools', name: '...', description: '...',
  tools: [
    (workspace: string) => ({
      name: 'git_status',
      description: '...',
      parameters: {...},
      async execute(callId, args, signal) { return { output, ok } },
    }),
  ],
  skills: [{ name, description, content }],   // SKILL.md 文本内嵌
  prompts: [{ name, description, argumentHint, content }],
}
```
**插件工具是「以 workspace 为参数的工厂」**，注册表按工作区实例化（`loader.ts:430`）。
这是当前唯一的「按工作区定制」通道。

### 2.3 加载层

`autoLoadExtensions`（`loader.ts:418-453`）：

1. `clearCustomTools()` 清空全部自定义工具（`:419`）
2. `readDisabledPlugins()` → Set（`:420`）
3. 内置插件逐个注册，id 前缀 `builtin:`（`:426-434`）
4. 工作区 `<workspace>/.ada/extensions`，id 前缀 `workspace:`（`:439-444`）
5. 全局 `~/.a-da/extensions`，id 前缀 `global:`（`:445-450`）

**实际优先级 = global > workspace > builtin**（后注册覆盖先注册），
因为 `register` 是 `Map.set`（`registry.ts:73`），且 `getToolsForWorkspace` 里
内置工具在前、自定义在后（`:124-127`）。

**没有任何清单/版本/依赖/权限校验**，jiti 直接执行（`loader.ts:73`），无沙箱。

### 2.4 管理层

`PluginsDialog.tsx` 七个 tab（`:25-35`）：skills / subagents / prompts / builtin-plugins /
workspace / global / builtins。插件卡（`:1666-2008`）可看工具/技能/提示词、启停（`:141-145`）、
删除（两步确认，仅非内置）。

用户**看不到**：插件报错详情（只有静态扫描的 `PluginItem.error`）、工具冲突、
插件来自哪个 scope 的细粒度标识（只有 tab 与一个 badge）、也没法配置插件。

### 2.5 配置层

`SavedConfig`（`config.ts:59-63`）只有全局键 + `disabledPlugins: string[]`（`:113-116`）。
**没有 per-plugin 配置块，没有 schema**。决策插件是自己去读 `saved.decision`
（`decision/config.ts:90-113`）+ 自己的 secrets 文件（`:57-67`）——**完全自助、不受核心管理**。

### 2.6 能力总览

| 插件能做 | 插件不能做 |
|---|---|
| 注册工具（单文件或目录包） | 拦截/改变控制流（事件丢弃返回值） |
| 目录包内提供 skills/`prompts/*.md` | 注册进程内钩子（只能靠 shell `hooks.json`） |
| 内置包内提供 skills[]/prompts[] | 贡献 slash 命令 / 子智能体 / 任何 UI |
| `onEvent` 只读观察（仅主循环） | 声明配置、密钥、权限、依赖、版本 |
| 读 `config.json` 任意键（自助） | 从远端安装；访问 store/会话状态 |
| 被全局启停、创建、删除 | 单文件 `.ts` 提供技能/提示词；重载时清理监听器 |

---

## 3. 缺陷修复清单（先修，属于"完善"的地基）

这 9 项都是**确定的缺陷**，与是否新增能力无关，应作为第一批提交。

| # | 缺陷 | 位置 | 修法 |
|---|---|---|---|
| 1 | 同名工具静默覆盖 | `registry.ts:72-74` | `register` 检测重名：返回冲突信息，按 `pluginId` 记录；内置被覆盖时**必须警告**（明确"插件遮蔽了内置工具"） |
| 2 | 自定义与内置同名 → 工具表出现两份 | `registry.ts:124-127` | 合并时按名字去重，自定义优先；并在冲突报告里标出 |
| 3 | 加载失败静默 | `loader.ts:408` | 失败写入可查询的**插件诊断列表**（`defaultExtensionLoader.getDiagnostics()`），UI 显红；`console.warn` 保留 |
| 4 | 监听器泄漏 | `loader.ts:74` | 监听器 Set 改为按插件分组持有，`clearCustomTools()` 同步清理；重载时先退订 |
| 5 | 无溯源 | `registry.ts:72-74` | `register(tool, { pluginId })`，map 值改存 `{ tool, pluginId }`；提供 `getToolOrigin(name)` |
| 6 | 停用状态全局 | `config.ts:113-121` | 新增按工作区的停用表（见 §5.3），保留全局作为默认 |
| 7 | 单文件扩展无法贡献技能/提示词 | `loader.ts:36-44` | 按 §4.3 统一到 `PluginDescriptor` 描述符形态后，`skills`/`prompts` 是**一等字段**，此缺陷自然消失（不必给 `ExtensionModule` 打补丁） |
| 8 | 子智能体不派发事件 | `store.ts:1842-1861`, `2252-2269` | 子循环同样 `dispatchAgentEvent`，事件带 `kind: 'subagent'` |
| 9 | 静态元数据与运行时注册无一致性检查 | `registry.ts:201-203` | 按 §4.3 **直接删除冗余的 `BUILTIN_TOOLS_METADATA`**，单一数据源（见 §7.2）；保留一条"声明了就要注册"的自洽性测试 |

这 9 项都是**确定的缺陷**，与是否新增能力无关，应作为第一批提交；
其中缺陷 3、5、6 是用户可直接感知的，优先级最高。

---

## 4. 目标契约（契约层）

### 4.1 统一插件描述

现状是两套契约各缺一半。目标是**一个描述符**，两套加载路径都产出它：

```ts
// src/agent/plugins/types.ts（新文件，作为插件系统的单一事实来源）
export interface PluginManifest {
  /** 唯一 id。内置插件为 'builtin:<id>' 形式，第三方由加载器按 scope 生成 */
  id: string
  /** 展示名 */
  name: string
  description: string
  /** 语义版本，用于兼容检查与展示（新增） */
  version?: string
  /** 作者信息（新增，展示用） */
  author?: string
  /** 插件来源，影响信任级别 */
  scope: 'builtin' | 'workspace' | 'global'
  /** 声明兼容的应用版本范围（新增） */
  engines?: { a_da?: string }
}

/** 插件提供的全部能力（每个都是可选的） */
export interface PluginContributions {
  tools?: BuiltinToolFactory[]
  skills?: Array<{ name: string; description: string; content: string }>
  prompts?: Array<{ name: string; description: string; argumentHint?: string; isSystem?: boolean; content: string }>
  /** 贡献子智能体 profile（新增，见 §6.3） */
  subagents?: SubagentProfile[]
  /** 贡献进程内钩子（新增，受能力边界约束，见 §6.1） */
  hooks?: AgentHooks
  /** 声明需要用户配置的项（新增，见 §5.2） */
  configSchema?: PluginConfigSchema
  /** 依赖的其他插件 id（新增，见 §5.4） */
  dependsOn?: string[]
}

/** 完整插件 = 元信息 + 能力 */
export interface PluginDescriptor extends PluginManifest, PluginContributions {}
```

**采纳方式：直接改名，不留别名**（本项目尚未正式使用，见 §4.3）：
- `BuiltinPluginPackage` **直接重命名为 `PluginDescriptor`**，6 个插件文件在同一次改动里
  改完（每个只是改 import 与类型标注，无逻辑变更）；
- 不存在"旧名字兼容期"——本项目**尚未正式使用**，没有需要保护的存量。

### 4.2 两套契约的关系：声明式 vs 命令式

必须把这件事讲清楚，否则设计会摇摆：

- **内置插件（builtin）**：随应用编译，**声明式纯数据**。适合官方能力，可被静态校验、
  可在不执行代码的前提下展示给用户（`PluginsDialog` 的 builtin tab 正是这么做的）。
- **第三方扩展**：用户本地代码，**命令式**，能拿到运行时上下文、能注册钩子。
  信任级别更低。

两者**共用 `PluginDescriptor` 的能力字段**，且**能力开关对两者一致适用**（§6.4.2）：
- 内置插件的 `hooks` 随应用发布，随应用一起被审计；
- 第三方钩子默认同样开放，但其风险由用户通过 `pluginCapabilities` 决定如何承担。

**核心不为两者硬编码不同的权限**——差异只体现在"用户默认更可能关掉第三方的某些能力"
这一预期上，而不是核心替用户做出区分。唯一对所有插件一律强制的是
**钩子返回值不能扩张工具集**（§6.4.3）。

不建议把内置插件也改成命令式——那会让「插件管理页能在不执行代码的前提下展示能力」这件事失效。

### 4.3 调整式改动：本项目尚未正式使用，可以破格

**前提**：a_da 还没有正式使用，没有外部用户、没有已发布的插件生态、没有需要保住的历史配置。
因此**不做兼容层，直接改到目标形态**。这解锁的不只是"省掉别名"，而是一系列本来会
被兼容性绑住的设计自由度。

#### 4.3.1 可以直接做的（不必留后路）

| 项 | 不留兼容的做法 |
|---|---|
| 类型改名 | `BuiltinPluginPackage` → `PluginDescriptor` **直接替换**，不留 `type X = Y` 别名 |
| 第三方扩展 API | `ExtensionContext` **可自由重新设计**；仓库内唯一的示例 `.ada/extensions/web-search.ts` 在同一次改动里改完 |
| 契约统一 | 两套契约统一到 `PluginDescriptor` 的**加载后形态**（见 §4.3.2） |
| 配置文件结构 | `config.json` 可**直接改结构**：`pluginCapabilities`、`pluginConfig`、按工作区停用表都按最终形态定，不做旧结构迁移 |
| 决策插件配置 | **不再保留**旧变量名兼容（`PI_JEV_BASE_URL` / `TYPESAFE_*`）与旧 `saved.decision` 块的读取路径——直接迁到 `readPluginConfig('decision')` |
| 插件 id 格式 | `builtin:` / `workspace:` / `global:` 前缀**可重新设计**，不必沿用 |
| 注册表内部结构 | `customTools: Map<string, AgentTool>` 可直接改成按 `pluginId` 分组的结构，无迁移 |

**一个重要澄清**："不留兼容"不等于"可以少做事"。省掉的是**别名、迁移代码、双读路径**，
而不是**测试**——恰恰相反，§10 的测试从"兼容性回归"换成了**更值钱的行为测试**
（见 §4.3.3）。

**改动量的实测**（已核对代码，用于判断"调整式改动"是否真的划算）：

| 项 | 实测 | 说明 |
|---|---|---|
| `BuiltinPluginPackage` 引用 | **8 个文件，每个 2 处** | 6 个插件文件 + `types.ts` + `index.ts`（+ decision 的 index） |
| `BUILTIN_TOOLS_METADATA` 消费方 | **仅 `PluginsDialog.tsx`（3 处）** | 加上 `describeTool`（`tools.ts`），删除成本很低 |
| 第三方扩展导出形态 | **仅 1 个示例**（`web-search.ts:81` 的 `export default function (api)`） | 改它一个文件 |
| 总计 | **约 10 个文件、20 处引用** | 一次 PR 可完成；`typecheck` 能抓住全部漏改 |

#### 4.3.2 契约统一：一种加载后形态，两个加载器

既然没有存量，就不该再维持"两套契约各缺一半"的局面（这正是 §2.1 批评的问题）。
目标是一个**加载后统一形态**：

```ts
// 加载后的插件，无论来源，都是这个形状
export interface LoadedPlugin {
  manifest: PluginManifest          // id / name / version / scope / engines
  contributions: PluginContributions // tools / skills / prompts / subagents / hooks / configSchema
  /** 声明式来源为 true（内置）；jiti 来源为 false */
  declarative: boolean
  /** 加载诊断（失败原因、冲突、缺依赖），UI 直接渲染 */
  diagnostics: PluginDiagnostic[]
}
```

- **两个加载器，一种产物**：内置走 `BUILTIN_PLUGINS` 数组，第三方走 jiti 扫描，
  两者都产出 `LoadedPlugin`。注册表、`PluginsDialog`、诊断、能力开关**只认这一种形状**，
  不再为来源分叉。
- **`declarative` 标志的真实用途**：只用来决定"能否在不执行代码的前提下展示"
  （内置 tab 可以，第三方列表必须已加载）。**不用于权限**——权限只看 §6.4.2 的开关。
- **第三方模块的导出形态**：`web-search.ts` 那种 `export default (api) => { api.registerTool(...) }`
  改为**首选导出描述符**：

  ```ts
  // 新的首选写法（与内置插件同形）
  export default {
    name: '联网搜索',
    description: '...',
    tools: [{ name: 'web_search', description: '...', parameters: {...}, execute(...) {...} }],
  } satisfies Partial<PluginDescriptor>

  // 需要运行时上下文的场景，仍可导出函数（此时由核心调用并收集它的注册）
  export default (ctx: ExtensionContext) => ({ tools: [...] })
  ```

  两条路径都产出同一个 `PluginDescriptor`，所以对注册表与 UI 是透明的。

#### 4.3.3 测试策略随之调整

| 原策略 | 调整为 |
|---|---|
| "6 个内置插件零修改仍可加载"（兼容性回归） | **删除**——本来就要改这 6 个文件 |
| "web-search.ts 不改也能跑"（兼容性回归） | **删除**——同一个 PR 里改它 |
| — | 新增：**6 个插件改名后行为不变**（改名前后的工具名/描述/技能/提示词逐项对照） |
| — | 新增：**两种导出形态（描述符 / 函数）产出等价的 `LoadedPlugin`** |
| — | 新增：**配置从旧结构迁到新结构后，决策插件解析结果一致**（验证迁移等价，而非保留双读） |

第三行是关键：**不是不测兼容，而是把"兼容"从运行时负担变成一次性的等价性验证**。
迁移做一次，验证做一次，之后代码里只留一条路径。

---

## 5. 加载层完善

### 5.1 冲突策略显式化（修缺陷 1、2、5）

```ts
// registry.ts
register(tool: AgentTool, origin?: { pluginId?: string; scope?: PluginScope }):
  { ok: true } | { ok: false; conflictWith: string; shadowed: boolean }

/** 查询工具的来源，供 UI 与"按插件禁用工具"使用 */
getToolOrigin(name: string): { pluginId: string; scope: PluginScope } | undefined
```

规则：
- **自定义覆盖自定义**：后注册者胜，但记入冲突报告（`getConflicts()`），UI 可见。
- **自定义覆盖内置**：**默认可覆盖**（受 `pluginCapabilities.allowBuiltinShadow` 控制，
  默认开）。早先版本建议 `strict` 默认拒绝——那是核心代拍，现已开放。
  覆盖必须产生一条**可见警告**（模型突然拿到的 `read_file` 可能行为完全不同），
  用户看到后可以关掉该开关或停用该插件。
- **工具表去重**：`getToolsForWorkspace` 按名字合并，不再出现同名两份。

### 5.2 插件配置声明（新增，取代"自助读配置"）

现状决策插件自己读 `saved.decision` + 自己的 secrets 文件，核心毫不知情。目标：

```ts
export interface PluginConfigSchema {
  /** 配置项声明，键即 config.json 中 pluginConfig[pluginId] 下的键 */
  properties: Record<string, {
    type: 'string' | 'number' | 'boolean' | 'secret'
    title: string
    description?: string
    default?: unknown
    /** 是否必填；缺失时插件被标记为"未就绪"而不是静默失败 */
    required?: boolean
    /** secret 类型存入 secrets 目录，不进 config.json */
    secret?: boolean
  }>
}
```

配套：
- 新增 `readPluginConfig(pluginId)`：env `A_DA_PLUGIN_<ID>_<KEY>` > `config.json`
  `pluginConfig[pluginId]` > secret 文件，与决策插件既有的优先级语义一致
  （`decision/config.ts` 已经这么做了，把它**提升为通用设施**）；
- **`required` 缺失时插件标记 `not-ready`**，UI 显示"需要配置"，且**不注册其工具**
  （而不是注册了然后每次调用失败）。这是"完善"的关键：把静默失败变成显式状态。
- 决策插件**直接迁移**到这套设施上：`decision/config.ts` 的自助解析改为调用
  `readPluginConfig('decision')`。按 §4.3 **不保留**旧变量名（`PI_JEV_BASE_URL` 等）
  与旧 `saved.decision` 块的读取路径——迁移一次到位，代码里只留一条路径。

### 5.3 按工作区启停（修缺陷 6）

```jsonc
// config.json —— 按最终形态定，不做旧结构迁移（§4.3）
{
  "disabledPlugins": ["git-tools"],                  // 全局默认；id 格式可重新设计
  "workspacePluginState": {                          // 按工作区覆盖
    "E:/codes/foo": { "disabledPlugins": ["web-search.ts"] }
  }
}
```
解析顺序：工作区覆盖 > 全局默认。

### 5.4 依赖与版本（新增，最小可用）

- `dependsOn: string[]`：加载时做**拓扑排序**；缺失依赖 → 插件标记 `broken`，
  原因写诊断列表，不注册工具。第三方的可选依赖建议用软失败（`dependsOn` 缺失仍加载）。
- `engines.a_da`：简单 semver range 检查，不匹配 → 标记 `incompatible` + 诊断，
  **默认仍然加载但警告**（避免一刀切把用户插件全禁掉）。

### 5.5 生命周期：重载要真正干净（修缺陷 4、8）

- `reloadPlugins()` 现在的实现（`autoLoadExtensions` → `clearCustomTools`）只清工具，
  不清事件监听器。目标：**按插件分组持有全部贡献**（tools / listeners / hooks），
  重载时整组退订再重建。
- 子智能体循环也派发事件（缺陷 8），事件需带 `kind: 'main' | 'subagent'` 与
  `subagentId`，否则插件无法区分来源（这是当前 `AgentEvent` 无法表达的）。

---

## 6. 运行时层：让插件能干预（但边界清晰）

这一层对应前一版方案的阶段 A/B/C/D，是**本轮最实质的能力新增**。

### 6.0 设计原则：点位必须成对，且成对地开放给插件

**凡是「可以有事前事后的点位，都必须成对存在，并且在插件系统里都能提供。」**
这是本层的第一原则，先立在这里，因为它决定了后面每个接口的形状。

现状盘点（这是本原则的动机）：

| 点位 | 事前 | 事后 | 插件可用？ |
|---|---|---|---|
| 工具调用 | `beforeToolCall`（`types.ts:171`） | `afterToolCall`（`:172`） | ✅ 唯一成对的 |
| 轮次 | ❌ 只有 `turn_start` 单向 yield（`agent-loop.ts:248`） | ❌ 只有 `turn_end`（`:378,587`） | ❌ 无回调通道 |
| 整轮 | `agent_start`（`agent-loop.ts:234`） | `agent_end`（`:607`） | ❌ 只有事件 |
| 子智能体启动 | ❌ 无 | ❌ 无 | ❌ |
| 子智能体结束 | — | ❌ 无（只有 `SubagentRunResult` 返回值） | ❌ |
| gate / 门禁 | ❌ 无 | ❌ 无（判定完就结束，无回执） | ❌ |

三个问题同时存在：**有些点位缺事后**（轮次、子智能体）、**有些点位两侧都缺回调**
（轮次只有事件、子智能体什么都没有）、**没有任何一项暴露给插件**（`agent-loop.ts` 的
三个钩子由 `store` 独占，`ExtensionContext` 完全够不到）。

**成对的意义不只是"对称"**，而是三件实际的事：
1. **可测量**：没有 after，插件无法知道自己的决策产生了什么后果（耗时、结果、是否被否决），
   也就无法自我校准。这直接呼应 `AGENTS.md` §9 的诚实性原则——不让插件有机会"以为"自己生效了。
2. **可清理**：没有 after，事前分配的资源（临时文件、锁、埋点计时器）没有释放时机。
3. **可审计**：事前放行 + 事后记录才构成完整轨迹；只有事前，出错时无法回答"到底跑没跑"。

### 6.1 统一的钩子契约：每个点位都是 `before` / `after` 成对

```ts
// src/agent/core/events.ts（新文件）
// 与 core/types.ts 的 AgentEvent 分开：AgentEvent 是「上报给 UI 的已发生事实」，
// 这里是「可干预的决策点」，有返回值、能改变控制流，信任要求更高

export interface AgentHooks {
  // ── 整轮（对应 agent_start / agent_end） ──
  beforeAgentStart?: (ctx: BeforeAgentStartContext) => Promise<BeforeAgentStartResult | undefined>
  afterAgentEnd?: (ctx: AfterAgentEndContext) => Promise<AfterAgentEndResult | undefined>

  // ── 轮次（对应 turn_start / turn_end）── 本次新增，成对
  beforeTurn?: (ctx: BeforeTurnContext) => Promise<BeforeTurnResult | undefined>
  afterTurn?: (ctx: AfterTurnContext) => Promise<AfterTurnResult | undefined>

  // ── 工具调用（既有，本次开放给插件）── 已成型
  beforeToolCall?: (ctx: BeforeToolCallContext) => Promise<BeforeToolCallResult | undefined>
  afterToolCall?: (ctx: AfterToolCallContext) => Promise<AfterToolCallResult | undefined>

  // ── 子智能体（本次新增，成对）──
  beforeSubagentStart?: (ctx: SubagentGateContext) => Promise<SubagentGateResult | undefined>
  afterSubagentEnd?: (ctx: SubagentEndContext) => Promise<AfterSubagentEndResult | undefined>
}
```

两套契约都开口，方式一致：
- 内置插件：`PluginDescriptor.hooks?`
- 第三方：`ExtensionContext.registerHooks(hooks)`（受 §6.4.2 的能力开关约束）

**`onEvent` 与 `registerHooks` 的职责必须分开**：`onEvent` 只读观察、丢弃返回值；
`registerHooks` 的返回值会改变控制流。两者分开声明，用户与审查者一眼能看出
这个插件是会"动手"的。按 §4.3 本项目尚未正式使用，`ExtensionContext` 可整体重新设计，
不必为旧的 4 成员形态留兼容。

### 6.2 轮次拦截：`beforeTurn` / `afterTurn`（最高价值，成对）

**为什么它是最关键的一项**：它是「插件能否影响模型看到什么」的唯一入口。
没有它，插件的所有贡献都是"多给模型几个工具"；有了它，插件才能做工具路由、
上下文裁剪、成本控制。

```ts
export interface BeforeTurnContext {
  step: number
  messages: AgentMessage[]
  tools: AgentTool[]
  workspace?: string
  threadId?: string
  kind: 'main' | 'subagent'
  subagentId?: string
}

export interface BeforeTurnResult {
  /** 本轮工具表覆盖。语义为「增量」：undefined=沿用，数组=设为该数组，'casual'=惰性档位 */
  tools?: AgentTool[] | 'casual'
  /** 追加消息（不写回历史，除非显式 persist） */
  extraMessages?: AgentMessage[]
  /** 结束整轮（= 现有 terminateBatch 语义，不是"结束这批"） */
  terminate?: boolean
  terminateReason?: string
}

/** 新增：轮次结束后回调，让事前决策变得可测量、可清理 */
export interface AfterTurnContext {
  step: number
  /** 本轮 assistant 消息（已定稿） */
  message: AssistantMessage
  /** 本轮工具结果 */
  toolResults: ToolResultMessage[]
  /** 本轮实际下发的工具名（= beforeTurn 决策的**结果**，不是它的输入） */
  effectiveToolNames: string[]
  /** 本轮模型请求耗时（ms） */
  llmDurationMs: number
  /** 本轮工具批耗时（ms） */
  toolsDurationMs: number
  kind: 'main' | 'subagent'
  threadId?: string
  subagentId?: string
  /** 事前是否被 terminate 短路 */
  terminatedByHook?: string
}

export interface AfterTurnResult {
  /**
   * 追加一条旁注消息给模型（下一轮生效），用于"你上一轮漏了 X"。
   *
   * **实现已收窄**：本节初稿还列过一个 `replaceText`（覆盖本轮 assistant 文本），
   * 实际未实现——本项目没有"改写已渲染回复"的交付通道，声明它只会变成静默失效。
   * `allowTextRewrite` 开关实际管的是这条 `appendNote` 与 `AfterAgentEndResult.appendText`。
   * 见 `docs/plugin-system-dev-plan.md` 的「M0-M3 独立复核记录」。
   */
  appendNote?: string
  /** 请求结束整轮 */
  terminate?: boolean
  terminateReason?: string
}
```

**关键设计点：`AfterTurnContext.effectiveToolNames` 是事前决策的"回执"。**
插件在 `beforeTurn` 里选了工具集，在 `afterTurn` 里能看到**实际生效的**是什么
（可能被 §6.4 的子集裁剪或 `casual` 降级改写）。这直接解决"插件以为自己生效了"的
诚实性问题——如果实际集合与它的意图不符，它可以在 `afterTurn` 里记 trace 或告警。

改造 `agent-loop.ts`：
1. `toolMap`/`toolSpecs` 从「循环外算一次」（`209-221`）改为**循环内每轮算**（挪进 `237`）。
   这是唯一的破坏性重构，**必须靠「逐事件行为等价」测试钉住**。
2. 每轮开头调 `beforeTurn`，按返回值覆盖；`turn_end`（`:378` 或 `:587`）之后调 `afterTurn`。
   **注意 `turn_end` 有两个出口**（无工具调用 `:378`、有工具调用 `:587`），
   `afterTurn` 必须在**两处都发**，否则会漏掉"纯文本轮"。
3. **整体 try/catch：抛错则沿用上一轮工具 + 记 trace**，绝不让插件打崩主循环。
   `afterTurn` 抛错**只记 trace，不影响本轮结果**（事后钩子不该改变已发生的事实）。
4. 工具名集合的 sig 未变则**复用同一 `toolSpecs` 引用**，避免无谓抖动。
5. `afterTurn` 的耗时**不计入 `llmDurationMs`/`toolsDurationMs`**，
   否则插件自身开销会污染它要测量的数据——这是个容易搞错的地方，写进注释。

### 6.2.1 整轮级：`beforeAgentStart` / `afterAgentEnd`

轮次钩子解决"每轮"，但有些插件关心的是"这次会话"：初始化资源、汇总统计、
在收尾时检查工作区。既有 `agent_start`（`:234`）/`agent_end`（`:607`）只有事件，没有回调。

```ts
export interface BeforeAgentStartContext {
  messages: AgentMessage[]
  tools: AgentTool[]
  systemPrompt: string
  workspace?: string
  threadId?: string
  kind: 'main' | 'subagent'
}
export interface BeforeAgentStartResult {
  /** 在系统提示词后追加一段（默认形态；`allowSystemPromptReplace` 开时可整体替换） */
  appendSystemPrompt?: string
  /**
   * 整体替换系统提示词。**受 `allowSystemPromptReplace` 控制**（默认开）。
   * 替换时核心仍会在末尾附上工具使用约定等不可协商段落，避免插件让模型
   * 完全失去"怎么用工具"的认知。
   */
  systemPrompt?: string
  /** 追加初始消息 */
  extraMessages?: AgentMessage[]
  /** 调整初始工具集（只能收窄，§6.4.3 唯一强制项） */
  tools?: AgentTool[]
}

export interface AfterAgentEndContext {
  reason: AgentEndReason
  messages: AgentMessage[]
  stepsExecuted: number
  durationMs: number
  kind: 'main' | 'subagent'
}
export interface AfterAgentEndResult {
  /** 追加一段文本到本次会话的最终回复（受 `allowTextRewrite` 控制，默认开） */
  appendText?: string
}
```

约束：`beforeAgentStart` **可替换 `systemPrompt`**（受 `allowSystemPromptReplace` 控制，
默认开），但**不得扩大工具集**（`tools` 只能收窄，§6.4.3 唯一强制项）。
`beforeAgentStart` 的工具表成为后续所有 `beforeTurn` 的初始集合基准，
所以它一旦收窄，整轮会话都在收窄后的基础上运行。

### 6.3 子智能体：成对的 `before` / `after`

对应 pi-jev 的 `gate`，但按本原则补上事后。

```ts
export interface SubagentProfile {
  // ...既有字段不变
  gate?: {
    criteria: string           // 验收标准，交给决策插件判定
    threshold?: number         // 默认 DEFAULT_GATE_THRESHOLD（0.7）
    failOpen?: boolean         // 默认 false
  }
}
```

**事前 `beforeSubagentStart` / `SubagentGateResult`**（同前版）：

```ts
export interface SubagentGateContext {
  subagentId: string
  subagentName: string
  parentThreadId: string
  task: string
  additionalContext?: string
  state: string
}
export interface SubagentGateResult {
  allowed: boolean
  confidence?: number      // 无引擎时 undefined，**不编造**
  reason?: string
  calibrated?: boolean
  /** 事前改写交给子智能体的任务描述（如补上"只读"约束） */
  taskOverride?: string
  /** 事前调整工具集（只能收窄，§6.4.3 唯一强制项） */
  tools?: AgentTool[]
}
```

**事后 `afterSubagentEnd`**（本次新增，**这是子智能体侧最缺的一块**）：

```ts
export interface SubagentEndContext {
  subagentId: string
  subagentName: string
  parentThreadId: string
  /** gate 是否放行（false 表示根本没启动） */
  allowed: boolean
  gateReason?: string
  /** 实际执行结果；被 gate 拒绝时为 undefined */
  result?: SubagentRunResult
  /** 子智能体实际用到的工具名，供"授予过宽"的自查 */
  effectiveToolNames: string[]
  durationMs: number
}

export interface AfterSubagentEndResult {
  /** 追加给父会话的旁注（如"本次子智能体尝试了 3 次写操作均被只读模式拒绝"） */
  appendParentNote?: string
}
```

`SubagentEndContext.effectiveToolNames` 与 `afterTurn` 的同名思想一致：
让"我给子智能体开了什么权限"这件事在事后可核对，而不只是事前的白名单意图。

**失败方向由用户决定**（本版调整，见 §6.4.4 第 5 条）：

- `failOpen` **未显式配置时视为 `true`（放行）**——"用户没表态"不该被核心解读为
  "要求安全"，用户的决定优先。
- 但**必须让用户知道**：某子智能体配了 `gate.criteria` 却没配 `failOpen`、
  且无引擎可用时，核心**不拦截**，同时在 UI 与事件流里提示
  **"门禁未生效（无引擎、未指定 failOpen）"**。
  开放不等于放任用户蒙在鼓里——**如实呈现**是开放的配套义务。
- 需要 fail-close 的用户显式写 `failOpen: false`。
- `gate.failOpen` 是**子智能体配置**（`.json` / frontmatter / builtins），
  不是全局开关，因为不同子智能体的风险差别很大。

**与早先版本的区别**：早先写的是"默认 fail-close，因为门禁永远放行比要求复核危险"——
那是核心替用户判断代价不对称。现在改为默认放行 + 强制提示。

**`afterSubagentEnd` 必须与既有唤醒机制共存**（这是最高风险点）：
`store.ts` 的 `finally` 里要做三件事，顺序不能错——
`afterSubagentEnd` 回调 → `runningThreadIds.delete()` → `wakeParent()`。
**回调必须放在 `wakeParent` 之前但不得依赖它**，且回调抛错**绝不能阻断唤醒**
（否则父会话永久挂起）。离线兜底路径同样要补
（`AGENTS.md` §7 已经踩过一次"兜底路径走不到 finally"）。

落地时**抽出共用函数**（现状 `store.ts:1756-1776` 与 `runner.ts:51-68` 各抄了一份过滤）：
```ts
// src/agent/subagents/access.ts（新文件）
export function resolveSubagentTools(profile, allTools, registry): AgentTool[]
export async function runSubagentGate(profile, ctx, signal?): Promise<SubagentGateResult>
```
gate 检查插在 `enabled` 检查（`store.ts:1677-1679`）之后、**构建会话之前**，
不通过就不创建 thread（省掉建消息、挂 tab、起循环的全部开销），
并在 `afterSubagentEnd` 里如实报告 `allowed: false` + `gateReason`。

**哪些入口要过门禁（2026-09-30 定案）**：凡是真的会**开跑**的入口都要过——
`startSubagentThread`（委派任务本身作判定输入）、`resumeSubagentThread`
（判定输入是「原始任务 + 本次恢复指示」，见 `store.resumeGateTask`：首轮已把原始 task
消耗掉，只给一句"网络恢复了，继续"判定方无从判断）、以及 `subagents/runner.ts` 的同步
兜底。恢复被拦下时**在写任何东西之前**抛出，会话原样不动、也不进运行集合。

仍有两条"续跑"入口没过门禁（**已知缺口**，都是既有行为，改动会牵到 `turn()` 的工具表，
需要单独拍板）：`steerSubagentThread` 对已停止子智能体的重新排队，以及用户在子智能体
标签页里直接输入（后者连 profile 白名单都不生效）。明细见 `docs/unfinished-features.md`
与 `docs/plugin-system-dev-plan.md` 的待办。

### 6.4 能力边界：默认开放，用户决定

**本节的第一原则：点位开放给插件后，用什么、放开到什么程度，由用户决定，不由核心代拍。**
核心只负责两件事——**如实呈现**每个开关的后果、**忠实执行**用户的设置。

所以本节不是一张"禁止清单"，而是一张**能力开关表**。

#### 6.4.1 立场变更说明（本版重要调整）

本方案早先版本在此处写的是"第三方钩子不能扩权"、`strict` 默认拒绝覆盖内置、
第三方只在 `code` 模式生效等**核心代拍的限制**。现按「用户决定什么就做什么」
调整为：

- **默认值统一取"开放"**，用户可以逐项关掉；
- 核心**不再有**"内置与第三方不同权"的隐式硬编码——差异只体现在**默认值建议**上，
  且用户可改写默认值；
- 唯一保留为**不可配置**的，是那些"放开后会破坏机制自身可信性"的项（见 §6.4.3），
  数量刻意压到最少，且每条都要写明**为什么不能配**。

#### 6.4.2 能力开关表

配置写入 `config.json` 的 `pluginCapabilities`（§5.2 的插件配置设施，全局 + 按工作区）：

```jsonc
{
  "pluginCapabilities": {
    // 全局默认：以下全部开放（除最后一条强制项，它不可配置）
    "allowSystemPromptReplace": true,    // beforeAgentStart 可整体替换系统提示词
    "allowTextRewrite": true,            // afterTurn.appendNote / afterAgentEnd.appendText 生效
    "allowThreadDeleteBlock": true,      // beforeThreadDelete 可阻止删除
    "allowCompactionReplace": true,      // beforeCompaction 可替换选择策略
    "allowPlanModeHooks": true,          // 钩子在 plan 模式也生效
    "allowThirdPartyHooks": true,        // 第三方扩展可注册钩子
    "allowBuiltinShadow": true,          // 插件工具可覆盖同名内置工具
    "hookTimeoutMs": 500,                // 超时；0 = 不限
    // 按插件覆盖
    "overrides": {
      "workspace:web-search.ts": { "allowPlanModeHooks": false }
    }
  }
}
// 注：工具集只能收窄、不能扩张——这一条不在此表中，因为它不可配置（§6.4.3）
```

| 能力 | 默认 | 关掉的效果 |
|---|---|---|
| `allowThirdPartyHooks` | **开** | 第三方只能 `registerTool`，回到现状 |
| `allowSystemPromptReplace` | **开** | `beforeAgentStart` 只能 append |
| `allowTextRewrite` | **开** | `afterTurn.appendNote` / `afterAgentEnd.appendText` 被忽略 |
| `allowThreadDeleteBlock` | **开** | 插件不能阻止删会话，只能归档 |
| `allowCompactionReplace` | **开** | `beforeCompaction` 只能追加保留消息 |
| `allowPlanModeHooks` | **开** | plan 模式下所有钩子不生效（回到现状） |
| `allowBuiltinShadow` | **开** | 插件工具不能覆盖同名内置工具：同名的那条**不注册**，保留内置工具，插件标为 `conflict` 并写明原因 |
| `hookTimeoutMs` | 500 | 超时后**放行并记 trace**（不 block——超时不该变成隐式拒绝） |
| ~~工具集扩张~~ | — | **不可配置**，见 §6.4.3 |

> **✅ `allowBuiltinShadow` 已实现（2026-09-30 补）。**
> 它曾在 2026-09 的独立复核里被记为"幽灵开关"：声明了、有默认值、走三层配置解析、
> UI 甚至写明关掉后的效果，但没有任何代码读它。现状是判定落在
> `tools/loader.ts` 的 `finalizePlugins`（加载与界面 `scanPlugins` **共用**这一步），
> 结论写进 `LoadedPlugin.blockedTools`，注册层 `applyPlugins` 照着跳过注册。
> 守门测试：`plugins/loader.test.ts` 的「能力开关：allowBuiltinShadow」两条
> （开着＝覆盖 + 标冲突 + 失去只读身份；关掉＝不注册 + 保留内置 + 原因可见）。
>
> **配套修掉的分类缺陷**：`isWriteTool` 过去只看名字，于是插件借走 `read_file`
> 这类只读内置名就"继承"了只读身份——plan 模式放行、readonly 审批档不问、只读子智能体
> 也拿得到，而 §6.4.3 的全部论证都建立在"工具集是审批闸门的依据"之上。现在分类
> **连来源一起看**（`tools/registry.ts`：非内置插件顶着只读名字注册的工具一律按写处理，
> scope `builtin` 的官方只读工具不受影响）。守门测试：`tools/registry.test.ts` 的
> 「写工具的判定要看来源」四条。

**每个开关都必须能被用户看到后果**：`PluginsDialog` 里每个插件卡显示它实际用到、
以及被哪些开关限制的能力（§7.1）。关掉某能力时，用到它的插件必须显示"受限"状态与原因，
**不允许静默失效**——这是本方案对"开放"的配套要求。

#### 6.4.3 唯一不可配置项：工具集不可扩张

`beforeTurn.tools` / `beforeAgentStart.tools` / `SubagentGateResult.tools`
**只能收窄，不能扩张**——这一条不可配置，理由必须说透：

工具集是**审批闸门的依据**。`store.gate()` 按 `isWriteTool(name)` 决定要不要弹审批，
而 `isWriteTool` 是一份**静态只读名单**（`registry.ts` 的 `READ_ONLY`）——名字不在名单里
就算写操作，非内置插件借走只读名字的也一律算写（见 §6.4.2 的补记）。
插件若能凭返回值塞进一个新名字的工具，就等于：

1. 绕过了审批（新名字可能不在白名单，或干脆是个从未注册的"幽灵工具"）；
2. 让 `afterTurn.effectiveToolNames` 这个"回执"失去意义——它回报的实际集合
   将包含核心并不认识的东西。

**注**：插件当然可以让自己**注册**的工具进表——那是走 `registerTool` 的正规路径，
会被 `isWriteTool` 正常分类、该审批就审批。被禁止的只是**在钩子返回值里动态塞工具**，
即在运行期扩大自己已有的授权。

如果用户确实想要"钩子动态加工具"的语义，正确做法是插件用 `registerTool` 注册，
再用钩子**把它收窄回去**——授权来源始终是注册表，不是钩子返回值。

#### 6.4.4 仍然保留的机制性约束（与自由度无关）

这些不是"限制插件能力"，而是**保证机制本身可工作**的约束，任何配置下都成立：

1. **`after*` 必须执行，即使 `before*` 被短路。** 否则被短路插件的清理逻辑就没了——
   这正是"成对"必须强制的原因。用户可配置的是**插件能做什么**，不是**钩子是否被调用**。
2. **钩子顺序**：多个插件注册同一钩子时按加载顺序串行；`before*` 中任一返回
   `block`/`terminate` 即短路后续的 `before*`。
3. **每个点位的耗时记入 trace**（默认超 500ms 警告），因为成对可以精确测量
   before 与 after 之间的插件开销。用户可调阈值或关掉警告，但**测量本身保留**。
4. **核心不读取 `Thread.pluginData`**（§6.7.3）——它是用户的插件数据，
   核心若去解释它就等于把插件数据变成隐式契约。
5. **`gate` 无引擎时的失败方向由用户的 `failOpen` 决定**（§6.3）。这里刻意**不再给
   核心默认值**：`failOpen` 未显式配置时视为 **true（放行）**，因为"用户没表态"
   不该被核心解读为"要求安全"。需要 fail-close 的用户显式写 `failOpen: false`。

   与早先版本的区别要说清：早先写的是"默认 fail-close，因为代价不对称"——
   那是核心替用户判断。现在改为**默认放行 + 配置提示**：当某子智能体配了 `gate.criteria`
   却没配 `failOpen` 且无引擎可用时，核心**不拦截**，但要在 UI 与事件流里
   **明确提示"门禁未生效（无引擎、未指定 failOpen）"**。用户的决定优先，
   但必须让他知道自己的决定产生了什么。

### 6.5 决策插件的接入点（成对地接）

决策插件（`decision/`）是这套钩子的第一个消费者，且**它本身也该受益于成对**：

| 决策插件能力 | 事前钩子 | 事后钩子 |
|---|---|---|
| 自动模式（每轮跑一次判定） | `beforeTurn` 决定工具表 | `afterTurn` 校验实际生效的工具集是否如预期，不符则 trace |
| 工具路由（反向收窄） | `beforeTurn` 选子集 | `afterTurn` 记录省下的 token 与是否漏工具 |
| 子智能体门禁 | `beforeSubagentStart`（`gate.criteria`） | `afterSubagentEnd` 复核"放行的是否真是需要的" |
| 验收门禁 `check_gate` | 判定即事前 | **无事后**（判定本身是终态，无需配对） |

最后一行是**本原则的边界**：**纯判定类点位天然无需成对**——它没有"后续状态"可观察，
强行配对只是加重负担。所以本原则的准确表述是「**有状态延续的点位都应成对**」，
而不是"一切都必须成对"。门禁放行后的状态延续属于子智能体的生命周期，
已由 `afterSubagentEnd` 覆盖。

### 6.6 点位总清单与补齐优先级

前面 §6.2-6.3 只覆盖了四类点位。这里给出**完整清单**，包括已经存在但没做成对的、
机制已有但完全没暴露的、以及目前根本没有的。这是本层的落地范围界定。

#### 6.6.1 现状：全仓点位盘点

| 点位 | 事前干预 | 事后干预 | 观测事件 | 插件可见 |
|---|---|---|---|---|
| 工具调用 | ✅ `beforeToolCall`(`types.ts:171`) | ✅ `afterToolCall`(`:172`) | ✅ 3 个 | ❌ store 独占 |
| 批次停止 | ✅ `shouldStopAfterTurn`(`:173`) | —（语义上即终态） | ❌ | ❌ store 独占 |
| 轮次 | ❌ | ❌ | ⚠️ `turn_start`(`agent-loop.ts:248`)/`turn_end`(`:378,587`) | ❌ |
| 整轮 | ❌ | ❌ | ✅ `agent_start`(`:234`)/`agent_end`(`:607`) | ⚠️ 仅 `onEvent` |
| 模型请求 | ❌ | ❌ | ✅ `llm_request`(`:269`)/`llm_response`(`:370`) | ⚠️ 仅 `onEvent` |
| 子智能体启动/结束 | ❌ | ❌ | ❌（子循环完全不派发事件） | ❌ |
| 审批闸门 | ❌（内部 `store.gate()` `:3237`） | ❌ | ❌ | ❌ |
| 检查点 | ❌ | ❌（内部 `captureCheckpoint` `:3256`） | ❌ | ❌ |
| 上下文压缩 | ❌（内部 `shouldAutoCompact` `policy.ts:95`、`selectCompactSelection` `runner.ts:20`） | ❌ | ❌ | ❌ |
| 系统提示词组装 | ❌（内部 `getCompositeSystemPrompt` `prompts/manager.ts:467`） | —（组装即终态） | ❌ | ❌ |
| 技能加载 | ❌（内部 `loadSkillContent` `skills/manager.ts:338`） | ❌ | ❌ | ❌ |
| 消息落盘 | ❌（内部 `persist` `:2841`） | —（落盘即终态） | ❌ | ❌ |
| 会话生命周期 | ❌（内部 `deleteThread` `:1162`） | ❌ | ❌ | ❌ |
| shell 钩子 | ✅ 可拦截(`hooks.ts:148`) | fire-and-forget(`store.ts:2992`) | — | ❌ 独立机制 |

**结论：全仓成对的点位只有工具调用一个**，且没有任何一个暴露给插件。
轮次两侧都缺回调；子智能体连事件都没有；审批/压缩/落盘/会话这些**语义明确的内部点
全部没有对外通道**。

#### 6.6.2 可补充点位与优先级

**第一优先——机制已在，只缺配对或暴露，补了立刻有用**

| 点位 | 现有实现 | 补什么 | 价值 |
|---|---|---|---|
| 轮次 | `turn_start`/`turn_end` 事件 | `beforeTurn`/`afterTurn`（§6.2，已设计） | 唯一能影响"模型看到什么" |
| 整轮 | `agent_start`/`agent_end` | `beforeAgentStart`/`afterAgentEnd`（§6.2.1） | 会话级初始化/收尾 |
| 子智能体 | 只有 `enabled` 检查 + `wakeParent` | `beforeSubagentStart`/`afterSubagentEnd`（§6.3） | 子智能体目前对插件完全不可见 |
| **审批闸门** | `store.gate()` `:3237-3390`，可 block | `beforeApproval`/`afterApproval` | **能实现"白名单工具免问"等自动批准策略**；现在只能靠 shell 钩子 |
| **上下文压缩** | `shouldAutoCompact` `policy.ts:95`、`selectCompactSelection` `runner.ts:20`、`executeCompaction` `:79` | `beforeCompaction`/`afterCompaction` | **能决定压缩保留什么消息**——pi-jev「压缩时保留关键历史」正落在这里（决策插件文档 §9 原标为"❌ 需改核心"） |

`beforeCompaction` 的可干预点：`selectCompactSelection` 返回 `CompactSelection`，
插件**默认可替换**（受 `allowCompactionReplace` 控制，默认开），
也可只**追加**必须保留的消息（如错误信息、用户明确要求）。
早先版本限定"只能追加"——那是核心代拍，现已开放为可配置。
风险提示仍要给出：替换过激会让压缩白做，所以 UI 要能显示本次压缩前后消息数对比，
让用户看得出"某个插件让压缩几乎没生效"。

**第二优先——补的是"暴露"，有效但需谨慎**

| 点位 | 现有实现 | 补什么 | 说明 |
|---|---|---|---|
| 模型请求 | `llm_request`/`llm_response` | `beforeLlmRequest`/`afterLlmResponse` | **上下文真正定稿之处**（`convertMessagesToLlm` `agent-loop.ts:261-264` 之后）。适合做"最后一刻脱敏/按 token 预算裁剪"。与 `beforeTurn` 的区别：turn 管工具表，llmRequest 管**最终 messages** |
| 系统提示词 | `getCompositeSystemPrompt` `prompts/manager.ts:467` | `beforeSystemPrompt` | 独立点位比塞进 `beforeAgentStart.appendSystemPrompt` 更清晰（§6.2.1 的写法是权宜） |
| 技能加载 | `loadSkillContent` `skills/manager.ts:338` | `beforeSkillLoad`/`afterSkillLoad` | 可注入/改写技能内容 |
| 消息落盘 | `persist` `:2841` | `beforePersist` | **敏感信息脱敏后再落盘**，安全价值高 |
| 检查点 | `captureCheckpoint` `:3256` | `afterCheckpoint` | 让插件感知"哪个文件将被改"，做只读审计 |

**第三优先——需要新前置能力，或收益需实测**

| 点位 | 阻塞点 |
|---|---|
| 模型选择（每轮换模型） | 现在是单例 `ProviderConfig`，**需先有多模型池** |
| 上下文超限前主动裁 | 需要 token 计数能力（与压缩不同：这是撞墙前裁） |
| 并发批次控制 | 并行/串行是 `agent-loop.ts:408-413` 硬判的，插件无法影响 |
| 错误/重试 | 重试在 `ai/stream.ts` 内部，插件看不到 |
| 用户输入改写 | `store.send` 入口；风险高（能改用户意图），需独立评估 |

#### 6.6.3 不要把所有东西塞进 `beforeTurn`

补点位有个容易犯的错：**把能塞的都塞进 `beforeTurn`**。判断标准是
**「触发时机」与「决策依据」是否真的等价**：

| 点位 | 触发时机 | 决策依据 | 能否合并 |
|---|---|---|---|
| `beforeTurn` | 每轮 | 当前对话状态 | — |
| `beforeCompaction` | 仅压缩时 | 哪些消息值得保留 | ❌ 时机不同 |
| `beforePersist` | 每条消息 | 内容是否敏感 | ❌ 时机不同 |
| `beforeLlmRequest` | 每次模型调用 | 最终 messages 是否超预算 | ❌ 与 turn 的"工具表"关注点不同 |

时机不同就该是独立点位。塞在一起会让插件的 `beforeTurn` 里堆满
`if (正在压缩) ...` 这类分支，而成对原则要求每个点位有明确的 after 语义——
合并之后 after 无法表达"什么时候算这次决策结束"。

### 6.7 会话生命周期：事前事后（本次新增）

会话（Thread）是比"轮次"更外层的单位，也是**目前在插件系统里完全不可见的一层**。
它的点位全部埋在 `store` 内部，插件既观测不到也干预不了。

#### 6.7.1 现状

| 会话点位 | 现有实现 | 是否有通道 |
|---|---|---|
| 创建 | `store.ts:1042`(`openTab`)及子智能体 `:1722` | ❌ |
| 打开/切换标签 | `openTab` `:1122` | ❌ |
| 关闭标签 | `closeTab` `:1136`（只关标签，**不碰数据**） | ❌ |
| 删除会话 | `deleteThread` `:1162`（连带清理子会话 `:1176`、`:1233`） | ❌ |
| 压缩 | `compactThread` `:2599` | ❌ |
| 中止 | `abort` `:333`、`controller.abort()` `:1303,1531,1600` | ❌ |

`Thread`（`types.ts:123-139`）只带 `id/title/createdAt/workspace/items/messages/mode/
parentId/subagentId/isSubagent` 与三个 `last*` 统计字段——**没有任何插件可挂载的元数据位**。

#### 6.7.2 契约

```ts
export interface BeforeThreadCreateContext {
  workspace: string
  /** 主会话无父；子智能体会话有 */
  parentId?: string
  isSubagent: boolean
  /** 触发来源：用户新建 / 子智能体派发 / 恢复已有会话 */
  origin: 'user' | 'subagent' | 'resume'
}

export interface BeforeThreadCreateResult {
  /** 建议标题（用户新建时可用；不得为空） */
  title?: string
  /** 会话级插件元数据，存进 Thread.pluginData[pluginId] */
  data?: Record<string, unknown>
}

export interface AfterThreadCreateContext {
  threadId: string
  thread: Thread
  origin: 'user' | 'subagent' | 'resume'
}

export interface BeforeThreadDeleteContext {
  threadId: string
  thread: Thread
  /** 连带删除的子会话 id（子智能体会话随之清理） */
  childThreadIds: string[]
  reason: 'user' | 'cascade'
}

export interface BeforeThreadDeleteResult {
  /** 是否可以拒绝删除（如插件仍有未提交的索引需先落盘） */
  block?: boolean
  blockReason?: string
  /**
   * 删除前的最后机会：插件在此做外部资源的清理/归档。
   * **不能**依赖它做持久化——会话文件马上会被删掉。
   */
  archiveBeforeDelete?: () => Promise<void>
}

export interface AfterThreadDeleteContext {
  threadId: string
  childThreadIds: string[]
  reason: 'user' | 'cascade'
  /** 归档是否成功（插件返回 archiveBeforeDelete 时才有值） */
  archived: boolean
}

/** 会话切换（标签打开/关闭不涉及数据变更，但影响"当前活跃会话"） */
export interface ThreadSwitchContext {
  fromThreadId?: string
  toThreadId: string
  workspace: string
}
```

并入 `AgentHooks`：

```ts
export interface AgentHooks {
  // ...§6.1 的成对点位

  /** 会话创建前——可建议标题、写入会话级元数据 */
  beforeThreadCreate?: (ctx: BeforeThreadCreateContext) => Promise<BeforeThreadCreateResult | undefined>
  afterThreadCreate?: (ctx: AfterThreadCreateContext) => Promise<void>

  /** 会话删除前——最后一次干预机会，可拒绝 */
  beforeThreadDelete?: (ctx: BeforeThreadDeleteContext) => Promise<BeforeThreadDeleteResult | undefined>
  afterThreadDelete?: (ctx: AfterThreadDeleteContext) => Promise<void>

  /** 会话切换（仅通知，不提供返回值——切换是 UI 行为，不该能改变） */
  onThreadSwitch?: (ctx: ThreadSwitchContext) => void
}
```

#### 6.7.3 三个必须写清的语义

1. **`onThreadSwitch` 刻意不成对、且不可干预。**
   切换标签是纯 UI 行为，没有"后续状态"需要配对，也**不应该**允许插件阻止用户看某个会话。
   所以它只有一个单向通知——这是 §6.5 那条"纯判定/纯通知类点位无需成对"原则的又一实例。

2. **`beforeThreadDelete` 的 `block` 对**所有插件**开放**（受 `allowThreadDeleteBlock`
   控制，默认开）。早先版本只给内置插件——那是核心代拍，现已取消。
   - 若用户关掉该开关，一切插件都只能 `archiveBeforeDelete`（善后），不能否决。
   - `block` 必须在 UI 显示拒绝理由，否则用户无法理解为什么删不掉。这**不是限制**，
     而是"拒绝要有理由"的基本要求——用户看到理由才能决定是否去关掉该插件的权限。

3. **`Thread` 需要新增 `pluginData` 字段承载会话级元数据。**
   现状（`types.ts:123-139`）没有插件可用的挂载位。建议：
   ```ts
   export interface Thread {
     // ...
     /** 插件写入的会话级元数据，键为 pluginId。核心不解释其内容 */
     pluginData?: Record<string, unknown>
   }
   ```
   约束：**核心永不读取它**（否则插件数据会变成隐式契约），
   且随会话一起持久化与删除（不能变成泄漏源）。
   这是 `beforeThreadCreate` 能返回 `data` 的前提——没有它，插件就无法在会话上
   留下任何跨轮次的状态。

#### 6.7.4 与 `agent_end` 的区别（别混淆）

| | `afterAgentEnd` | `afterThreadDelete` |
|---|---|---|
| 触发 | 一次 `runAgentLoop` 结束（一轮对话跑完） | 会话被**移除**（数据即将消失） |
| 频率 | 每个会话多次 | 每个会话至多一次 |
| 会话数据 | 仍在，可读写 | **即将被删**，只能归档 |
| 用途 | 汇总本次统计 | 清理外部资源、撤销索引 |

**最常见的错误**是把持久化逻辑放在 `afterThreadDelete`——那时会话文件正在被删。
要持久化的东西必须用 `afterAgentEnd` 或 `afterTurn`。

### 6.8 补齐后的点位全景

按本方案实施后，插件的可见范围：

| 层 | 事前（可干预） | 事后（可观测/可修正） |
|---|---|---|
| 会话 | `beforeThreadCreate`、`beforeThreadDelete` | `afterThreadCreate`、`afterThreadDelete`、`onThreadSwitch`(单向) |
| 整轮 | `beforeAgentStart` | `afterAgentEnd` |
| 压缩 | `beforeCompaction` | `afterCompaction` |
| 轮次 | `beforeTurn` | `afterTurn` |
| 模型请求 | `beforeLlmRequest` | `afterLlmResponse` |
| 工具 | `beforeToolCall`、`beforeApproval` | `afterToolCall`、`afterApproval`、`afterCheckpoint` |
| 子智能体 | `beforeSubagentStart` | `afterSubagentEnd` |
| 资源 | `beforeSystemPrompt`、`beforeSkillLoad`、`beforePersist` | `afterSkillLoad` |

**仍然没有对外通道的**（第三优先，需前置能力）：模型选择、上下文超限前裁剪、
并发批次控制、错误/重试、用户输入改写。它们列在 §6.6.2 的第三优先表中，
不在本次范围内。

### 6.9 审批：策略归插件，执行归核心

**本节是"把审批功能用插件系统实现"的落地说明**（后补，2026-09）。

#### 6.9.1 分工：一个判断，不是一次交互

审批的本质是**判断**——"这次调用要不要经用户确认"。而弹卡片、等点击、中止、超时、
把结果写回历史是**交互执行**。前者适合插件（每个团队的白名单不同），后者必须留在核心。

把执行也交给插件会带来两个无法接受的后果：

1. **解释权外移**：插件自己实现等待，就等于由它解释"用户点了什么"——它可以谎报批准；
2. **中止/超时无处保证**：核心无从知道插件是否还在等，会话中止时会留下悬挂的 promise。

所以审批拆成两半，各归其位：

| 归插件（策略） | 归核心（执行） |
|---|---|
| 哪些工具免问 | 弹卡片、卡片状态 |
| 哪些命令要二次确认 | 等用户点击（`approvals` 句柄） |
| 拒绝时给模型什么理由 | 中止 / 超时收尾 |
| 免问的粒度与条件 | 把理由写回模型历史 |

#### 6.9.2 受控能力 `askUser`

插件要实现自定义策略（如"高危命令即便在白名单里也要问"）就必须能发起询问。
核心为此提供**一个受控能力**，注入在 `beforeApproval` 的上下文上（`core/events.ts`）：

```ts
beforeApproval: async (ctx) => {
  // ctx.askUser 由核心实现，插件只能调用
  const answer = await ctx.askUser({ reason: '这条命令命中了高危模式，请确认' })
  return answer.approved ? { decision: 'allow' } : { decision: 'deny', reason: '用户拒绝' }
}
```

能力边界（**这是本节最要紧的部分**）：

- **只能转发真实点击**：`askUser` 内部走的就是核心 `waitForUserApproval`，
  与内核自己的审批闸门**共用同一份实现**。核心**不提供任何"直接批准"的接口**，
  所以插件无法伪造用户意图。
- **不能决定怎么弹**：`AskUserRequest` 只有 `reason` / `options` / `toolCall`，
  没有渲染或超时控制——界面归核心。
- **未注入时字段缺席**（子智能体循环、无界面场景）：插件应当返回 `undefined`
  让核心照常问，而不是自己猜一个答案。`approval-guard` 就是这么做的。
- **`answeredBy` 如实区分** `'user'` 与 `'aborted'`：两者在结果上都算拒绝，
  但事后审计必须能分辨"用户说不"和"用户没答"。

#### 6.9.3 点位顺序：`beforeToolCall` 先于 `beforeApproval`

两者都在工具执行前触发，语义不同，顺序不能反：

```
beforeToolCall（插件：这个调用根本不该发出去）
      ↓ 未被 block
beforeApproval（核心闸门 + 插件策略：要不要问用户）
      ↓ 允许
用户/免问 → 执行
```

理由是"不该发生"应当先于"要不要问"——否则用户会被问一个注定被插件拦下的调用。

**实施说明（重要）**：`beforeToolCall`/`afterToolCall` 早在 M2 就已进 `AgentHooks`
并由 `hook-runtime` 完整实现（超时、计时、折叠），但**主循环从来没有调用过
`hooks.beforeToolCall`**——它只调 `options.beforeToolCall`（store 的审批闸门）。
于是插件注册的这两个点位是**死代码**：声明了、合成了、单独测过，就是没人调。
本次修复把 `hooks.*` 接进循环，并补了能抓住它的测试（断言"被 block 时工具
**真的没执行**"——只有副作用断言能暴露"声明了却没人调用"这类缺陷）。

#### 6.9.4 内置插件 `approval-guard`

审批策略的参考实现，三层（自上而下，先命中先返回）：

| 层 | 规则 | 为什么排这里 |
|---|---|---|
| 1 | **高危命令二次确认** | 必须最先——排在免问之前，否则白名单会把最需要确认的调用一起放过 |
| 2 | **只读档位硬约束** | `readonly` 时写操作照常问；**不返回 `allow`**（核心会忽略它，插件也不该发出注定被忽略的意图） |
| 3 | **免问白名单** | 默认**空**——"默认自动批准"不是可接受的默认值 |

它是**纯策略插件**：`tools: []`，只订 `beforeApproval`。这带来一个契约事实——
「三位一体」（工具+技能+提示词）是对**带工具那类插件**的要求，
纯策略插件不该为了满足断言去造一个没有用途的工具。

匹配语义要说清：`confirmCommands` 是**字面子串**（大小写不敏感），不是通配或正则。
`git push` 能命中 `git push --force`，但 `docker prune` **匹配不到**
`docker system prune -a`（中间隔着 `system`）。选这个语义是因为它可预期、
无回溯风险；代价是跨词模式写不出来。

#### 6.9.5 一个实施中发现的缺陷：`askUser` 会死等

`gate()` 里卡片是在**插件判定之后**才建的，而 `askUser` 在判定**之中**就被调用。
最初只建"等待句柄"不建卡片，于是界面上没有任何可点的东西——`askUser` 永久挂起。

修法：`waitForUserApproval` **没有卡片就现造一张**（状态 `awaiting`）并推进会话，
`gate` 随后按 `call.id` 复用同一张，不再造第二张（否则界面上会出现两个同一次调用的卡片）。

这类缺陷只有**端到端测试**能发现——单测 `askUser` 的实现、或单测插件的策略逻辑，
两边都会通过。所以 §10 的 `askUser` 用例是走真实 `gate` + 真实 `decide()`
（模拟点击）的。

---

## 7. 管理层：让用户看得见、配得了

### 7.1 插件卡增强（`PluginsDialog.tsx`）

现状一张卡只有：名字、scope badge、启停、删除、工具/技能/提示词列表、静态扫描报错。

新增：
- **状态徽标**：`ready` / `not-ready`（缺必填配置）/ `incompatible`（版本不匹配）/
  `broken`（依赖缺失或加载失败）/ `conflict`（有工具冲突），取代现在"只有 error 才显示"。
- **版本与作者**（有则显示）。
- **冲突提示**：这个插件覆盖了哪些工具（尤其"遮蔽了内置 `read_file`"必须显眼）。
- **配置表单**：由 `configSchema` **自动生成**（string/number/boolean/secret），
  secret 输入框不回显。这是 §5.2 的 UI 面。
- **诊断详情**：加载失败的真实原因（现在是 `console.warn`，用户看不到）。
- **贡献计数**：`工具 3 · 技能 1 · 提示词 1 · 子智能体 0`，一目了然。

### 7.2 内置插件 tab 与运行时的一致性

`PluginsDialog` 的 builtin 列表来自 `BUILTIN_TOOLS_METADATA`（静态），
而实际注册来自 `autoLoadExtensions`——**两者会漂移**。决策插件的三个工具此刻是对齐的，
但那是人工维护的结果，机制上没有任何东西阻止下一次改动漏掉一边（这正是缺陷 9）。

**按 §4.3 的调整式改动，这里可以做得更彻底：直接消除 `BUILTIN_TOOLS_METADATA`。**
理由是它本身就是冗余的第二事实来源——内置插件的工具定义已经在 `PluginDescriptor.tools` 里，
再维护一份静态元数据只可能漂移。改为：

- `PluginsDialog` 只读**运行时注册结果**（`registry.listByPlugin()` → `LoadedPlugin`）；
- 需要"不执行代码也能展示"的场景（内置 tab），读 `BUILTIN_PLUGINS` 的**声明数据**
  （它本来就是纯数据，无需执行）；
- `BUILTIN_TOOLS_METADATA` 删除，`describeTool` 等消费方改从工具对象自身取字段。

保留一条测试作为回归保险：`BUILTIN_PLUGINS` 里每个插件的每个工具都能在
注册表里找到（防"声明了但没注册"）。但此时它守的是**同一份数据源**的自洽性，
而不是两个来源的一致性——漂移的可能性已经不存在了。

### 7.3 诊断与自检

- `defaultExtensionLoader.getDiagnostics(): PluginDiagnostic[]`
  （`{ pluginId, level, message, hint? }`），UI 与 `scripts/extension-check.ts` 共用。
- `scripts/extension-check.ts` 扩展为：加载校验 + 工具进表校验 + **冲突/依赖/配置就绪**校验。

---

## 8. 与前一版方案的关系（避免重复决策）

前一版 `core-extension-capabilities-design.md` 已经把 `beforeTurn` / 工具档位 /
子智能体 gate 的**代码级落点**写得很细。本方案的调整是：

| 项 | 前一版定位 | 本版定位 |
|---|---|---|
| `beforeTurn` | 阶段 A，独立目标 | **插件系统的一种能力**（§6.2），归属不变，优先级不变 |
| 工具档位 `casual` | 阶段 B，省 token | **保留**，但降为 `beforeTurn` 的一个消费者示例（§6.2） |
| 子智能体 gate | 阶段 C | **保留**，并入 §6.3 |
| 插件事件 API | 阶段 D，最后做 | **提到 §4 契约层**，因为它是"完善插件系统"的本体 |
| 决策后端 SDK | 阶段 E，可选 | **本方案不做**。理由不变：`JevEngine`（`engine.ts:528`）功能语义已完整，引入 SDK 会把「Bun + 一个 OpenAI 兼容端点即可跑」变成「可能要额外服务」，与单文件可执行 + 本地优先冲突。要做须为可选 peerDependency + 动态 `import()` |

**优先级重排**（以"完善插件系统"为目标）：

1. **§3 缺陷修复**（9 项，尤其缺陷 3、5、6 用户可直接感知）——地基
2. **§4 契约统一 + §5 加载层**——插件系统的本体
3. **§6.1 / §6.2 成对的 `beforeTurn` / `afterTurn`**——解锁"插件能影响模型看到什么"
   （**必须成对一起做**：先有 after 才能验证 before 是否真的生效）
4. **§6.4 能力边界**——与 §6.1 同批做，不能事后补
5. **§6.6.2 第一优先其余四项**：整轮（§6.2.1）、子智能体（§6.3）、
   审批闸门、上下文压缩——机制都已在，只缺暴露，性价比最高
6. **§6.7 会话生命周期**——补上最外层、目前完全不可见的一层
7. **§6.6.2 第二优先五项**：`beforeLlmRequest` / `beforeSystemPrompt` /
   `beforeSkillLoad` / `beforePersist` / `afterCheckpoint`
8. **§7 管理层**——让上面的一切对用户可见

关于 §6.2 的 `afterTurn` **不能延后**：如果先只发 `beforeTurn`，插件会在"以为自己改了工具表
但其实被子集裁剪/降级改写"的状态下运行，而没有任何通道能发现这一点——这正是
`AGENTS.md` §9 所反对的"名存实亡"。成对是这一层的最小可用单位，不是可选项。

§6.6 给出的**完整点位清单**是本层的落地范围界定：第一优先补的是"机制已在、只缺暴露"的五类
（轮次、整轮、子智能体、审批、压缩），第二优先是"暴露但需谨慎"的五类，
第三优先五项（模型选择、上下文超限前裁剪、并发批次、错误重试、用户输入改写）
**明确不在本次范围内**——它们需要前置能力（如多模型池），列出来是为了划清边界。

---

## 9. 文件清单

### 9.1 新增

```
src/agent/plugins/types.ts          # §4.1 PluginManifest/Contributions/Descriptor/LoadedPlugin/ConfigSchema
src/agent/core/events.ts            # §6.1 AgentHooks（成对，含 §6.7 会话生命周期）+ 各 before*/after* 上下文类型
src/agent/plugins/registry.ts       # 插件级索引：按 pluginId 聚合贡献、诊断、冲突
src/agent/subagents/access.ts       # §6.3 resolveSubagentTools / runSubagentGate
src/agent/hooks/compaction-hooks.ts # §6.6.2 压缩点位包装（默认允许替换，可配为仅追加）
src/agent/plugins/plugin-system.test.ts
src/agent/core/before-turn.test.ts  # §6.2 含成对性测试
src/agent/core/hooks-pairing.test.ts # §6.1 成对契约的接口层断言（漏配即红）
src/agent/core/thread-hooks.test.ts  # §6.7 会话生命周期
src/agent/plugins/equivalence.test.ts # §4.3.3 改名/统一契约后的一次性等价性验证
src/agent/subagents/access.test.ts
```

### 9.2 必须同步修改

| 文件 | 改动 | 章节 |
|---|---|---|
| `tools/builtin-plugins/types.ts` | `BuiltinPluginPackage` **直接重命名为** `PluginDescriptor`，**不留别名**；新增能力字段 | §4.1, 4.3 |
| `tools/builtin-plugins/*.ts`（6 个插件文件） | 改 import 与类型标注为新名字（无逻辑变更） | §4.3.1 |
| `.ada/extensions/web-search.ts` | 改为**首选导出描述符**形态（§4.3.2） | §4.3.2 |
| `tools/builtin-plugins/index.ts` | 加载/注册顺序与冲突报告接入 | §3-1,2 / §5.1 |
| `tools/builtin-plugins/decision/index.ts` | 迁移配置到 `readPluginConfig`（工具注册本身无需改） | §5.2 |
| `tools/loader.ts` | 产出统一的 `LoadedPlugin`；`register(…, origin)`；监听器分组清理；诊断列表；`registerHooks`；`ExtensionContext` 重新设计 | §3-2,4,5,8 / §4.3.2 / §5.5 / §6.1 |
| `tools/registry.ts` | 冲突检测与去重、`getToolOrigin`、`listByPlugin`、`READ_ONLY` 补新工具、**删除 `BUILTIN_TOOLS_METADATA`** | §5.1 / §7.2 |
| `core/types.ts` + `core/agent-loop.ts` | **成对** `beforeTurn`/`afterTurn` + `beforeAgentStart`/`afterAgentEnd`，工具表进循环，`turn_end` 两处出口都发 after | §6.2, 6.2.1 |
| `core/types.ts` | `Thread` 新增 `pluginData`（核心不读取） | §6.7.3 |
| `core/events.ts`（既有事件） | 事件加 `kind`/`subagentId` | §5.5 |
| `compact/runner.ts` + `policy.ts` | 压缩点位包装：接受 `beforeCompaction` 的**追加**保留消息 | §6.6.2 |
| `subagents/types.ts` | `gate` 字段；`SubagentEndContext` | §6.3 |
| `subagents/builtins.ts` + `.test.ts` | 白名单补新工具 | §10-1 |
| `subagents/runner.ts` | 改调 `access.ts` | §6.3 |
| `subagents/manager.ts` | 解析 `gate` | §6.3 |
| `tools/builtins/subagent.ts` | `invoke_subagent` 透传 gate 结论 | §6.3 |
| `agent/config.ts` | `readPluginConfig`、按工作区停用表 | §5.2,5.3 |
| `tools/builtin-plugins/decision/config.ts` | **直接迁移**到 `readPluginConfig`，**不留**旧变量名/旧 config 块的双读路径 | §4.3.1 / §5.2 |
| `ui/PluginsDialog.tsx` | 状态徽标、配置表单、冲突/诊断、**能力开关面板与"受限"提示**、不再读 `BUILTIN_TOOLS_METADATA` | §7.1 / §6.4.2 / §7.2 |
| `ui/Composer.tsx` | 工具档位徽标（若做 `casual`） | §6.2 |
| `tools.ts` | `describeTool` 改为从工具对象自身取字段（`BUILTIN_TOOLS_METADATA` 删除后） | §7.2 |
| `store.ts` | `beforeTurn`/`afterTurn` 挂载（主 + 子）、`afterSubagentEnd` 与 `wakeParent` 的顺序、gate、按插件工具过滤、审批/压缩/会话生命周期各点位、**能力开关读取** | §6 |
| `scripts/extension-check.ts` | 冲突/依赖/配置就绪校验 | §7.3 |
| `README.md` | 重写「扩展」一节为插件系统契约说明 | §7 |

### 9.3 顺序

**第 0 步（改动式改动的前提，必须最先做且单独成 commit）**：
按 §4.3 做**不兼容的结构调整**——类型改名（`BuiltinPluginPackage` → `PluginDescriptor`）、
契约统一到 `LoadedPlugin`、删除 `BUILTIN_TOOLS_METADATA`、`config.json` 结构定稿、
决策插件配置迁移。**这一批只做结构调整，不加新功能**，并用 §4.3.3 的等价性测试钉住
"行为完全没变"。好处是后续每一步都建立在新结构上，不必回头改两次。
`plugins/types.ts` 与 `loader.ts` 的 `LoadedPlugin` 改造也在这一步。

然后：

1. 缺陷修复（§3 剩余项 → 独立 commit，便于 review）
2. `plugins/registry.ts` + 加载层完善（`pluginId` 溯源、冲突、诊断、依赖、可重载，§5）
3. `core/events.ts` + **成对的** `beforeTurn`/`afterTurn`（§6.1、§6.2）+
   能力开关表（§6.4）—— 三者同一批，成对性是这一批的验收条件
4. `beforeAgentStart`/`afterAgentEnd`（§6.2.1）+ 审批闸门与压缩点位（§6.6.2 第一优先）
5. `access.ts` + gate + `afterSubagentEnd`（§6.3）
6. 会话生命周期（§6.7，含 `Thread.pluginData`）
7. 第二优先点位（§6.6.2：`beforeLlmRequest` / `beforeSystemPrompt` /
   `beforeSkillLoad` / `beforePersist` / `afterCheckpoint`）
8. `PluginsDialog` 增强（§7）
9. README 与 `scripts/extension-check.ts` 收尾

---

## 10. 测试计划

**契约与加载层**（按 §4.3.3：兼容性回归已删，改为等价性验证）
- **6 个内置插件改名后行为不变**：工具名/描述/技能/提示词逐项对照改动前
- **两种导出形态（描述符 / 函数）产出等价的 `LoadedPlugin`**
- **`web-search.ts` 改为描述符形态后，`web_search` 工具行为不变**
- **决策插件配置迁到 `readPluginConfig` 后，解析结果与迁移前一致**（验证迁移等价）
- 同名工具冲突：自定义覆盖内置时产生可见警告；工具表**不出现同名两份**
- `getToolOrigin` 对内置/工作区/全局工具都返回正确 pluginId
- 依赖拓扑：缺失依赖 → 插件 `broken`，其工具**不注册**
- `engines` 不匹配 → `incompatible` + 诊断，但仍加载（软失败）
- 缺必填配置 → `not-ready`，其工具**不注册**
- 重载后**旧事件监听器已退订**（修缺陷 4 的回归）
- `LoadedPlugin` 是唯一产物形状：内置与第三方加载后字段结构一致

**一致性守门（防漂移）**
- `BUILTIN_PLUGINS` 里每个插件的每个工具都真实注册（修缺陷 9 的自洽性守门，
  删除 `BUILTIN_TOOLS_METADATA` 后只守单一数据源）
- README 列出的内置插件工具都能在注册表找到
- 子智能体白名单里的每个工具名都真实存在（防拼写漂移）

**`beforeTurn` / `afterTurn`（改主循环，最重要）**
- 无钩子时行为与改动前**逐事件等价**（钉住重构）
- 返回 `tools` → 本轮 `llm_request.tools` 与 `streamModelChat` 收到该数组
- **抛错 → 沿用上一轮工具，循环继续**
- **`afterTurn` 在 `turn_end` 的两个出口都触发**（无工具调用的纯文本轮 `:378`
  与有工具调用的轮 `:587`）——漏一处就会静默丢事件
- `afterTurn.effectiveToolNames` **等于实际下发的工具名**，而非 `beforeTurn` 的意图
  （构造"意图被裁剪"的场景验证两者确实不同）
- `afterTurn` 抛错**不影响本轮结果**、不影响后续轮次
- `afterTurn` 的耗时**不计入 `llmDurationMs`/`toolsDurationMs`**
- 工具集未变的轮次复用同一 `toolSpecs` 引用
- `terminate` → **跑完本轮后** break（不是当前批就断）

**成对原则的机制性测试（新一组，防止"成对"名存实亡）**
- `before*` 被短路（block/terminate）时，**`after*` 仍然执行**——保证清理逻辑不丢
- 每个 `before*` 都有对应的 `after*` 在契约里存在（**接口层断言**：
  对 `AgentHooks` 的键做配对检查，漏配即测试失败；刻意单向的点位须登记进 `UNPAIRED_HOOKS`）
- `beforeAgentStart` **可以替换 `systemPrompt`**（受 `allowSystemPromptReplace` 控制，
  默认开）——注意这一条曾按早先的"核心代拍"版本反向写下，§6.4 已改为默认开放
- `afterAgentEnd` / `afterSubagentEnd` 抛错**不阻断** `wakeParent`（防永久挂起）

**第一优先新增点位（机制已在，只缺暴露）**
- 审批闸门：`beforeApproval` 返回允许 → 不弹卡片直接执行；返回拒绝 → 等同 `block`，
  且**拒绝理由回给模型**（不能变成"工具执行失败"）
- 审批闸门：`afterApproval` 能拿到实际决策（批准/拒绝/超时）与耗时
- 压缩：`beforeCompaction` 追加的保留消息**确实出现在压缩后的历史里**
- 压缩：插件**可以替换** `CompactSelection`（受 `allowCompactionReplace` 控制，默认开）；
  关掉时只能追加保留消息——同上，这一条也按 §6.4 的开放原则改过
- `afterCompaction` 能拿到压缩前后的消息数对比

**会话生命周期（§6.7）**
- `beforeThreadCreate` 的 `title` 建议被采纳；返回空标题被忽略（不产生无名会话）
- `beforeThreadCreate` 的 `data` 落进 `Thread.pluginData[pluginId]`，且**随会话持久化**
- `afterThreadCreate` 拿到的 `thread` 是已就绪状态（能读到 `id`/`workspace`）
- `beforeThreadDelete` 返回 `block: true` → 阻止删除，且有可见理由；**对所有插件一致**
  （受 `allowThreadDeleteBlock` 控制，默认开。早先版本只给内置插件否决权，那是核心代拍，
  已取消——见 §6.7.3）
- `archiveBeforeDelete` 在删除**之前**执行；`archived` 如实反映成功与否
- **归档失败不阻止删除**（用户明确要删，不能因插件归档失败而卡住）
- 级联删除子会话时，每个子会话的 `beforeThreadDelete` 都被调用一次
- `onThreadSwitch` **无返回值、不能阻止切换**
- `Thread.pluginData` 随删除一并清理（**不产生泄漏**）
- 核心代码**不读取** `pluginData`（静态检查/审计：grep 核心目录无非测试引用）

**能力边界（开放原则，必测）**
- 每个 `pluginCapabilities` 开关**关掉后确实生效**，且开/关行为可区分
- `allowBuiltinShadow: false` → 占用了核心内置工具名字的插件工具**不注册**、内置工具保留、
  插件状态标 `conflict` 且原因在插件卡上可见；开着时相反（覆盖 + 标冲突）
- **`isWriteTool` 的判定要看来源**：非内置插件顶着只读名字注册的工具按写处理，
  scope 为 `builtin` 的官方只读工具不受影响（`tools/registry.test.ts`）
- `allowThreadDeleteBlock: false` → 插件 `block: true` 不阻止删除
- `allowSystemPromptReplace: false` → 传 `systemPrompt` 被忽略，只生效 append
- `allowTextRewrite: false` → `afterTurn.appendNote` / `afterAgentEnd.appendText` 无效
- `allowPlanModeHooks: false` → plan 模式下钩子不生效
- `allowCompactionReplace: false` → 只能追加保留消息
- **能力被开关限制时，插件与用户都能看到"受限"状态**（不允许静默失效）
- **唯一强制项**：任何开关组合下，钩子返回**超集 → 一定被裁回子集**
  （该行为不可配置，表里没有对应开关）
- **`after*` 在 `before*` 被短路时仍执行**（与任何开关无关）
- `hookTimeoutMs` 超时 → **放行并记 trace**，不变成隐式 block
- `gate` 未配 `failOpen` + 无引擎 → **放行**，且 UI/事件流有"门禁未生效"提示
- `gate.failOpen: false` + 无引擎 → 拦截
- 子智能体白名单里的每个工具名都真实存在（防拼写漂移）

**gate**
- 未配 `gate` → 直接放行（**现状不回归**）
- 引擎可用 + `confidence < threshold` → **不创建 thread**
- 引擎不可用 + `failOpen` 未设 → **放行**，并有"门禁未生效"提示（§6.3）
- 引擎不可用 + `failOpen: false` → **拦截**
- `.json` / frontmatter 里的 `gate` 解析正确
- **gate 拦截路径调用后父会话不被永久挂起**（`wakeParent` 与 `runningThreadIds` 一致）
- `store` 与 `runner` 过滤结果逐工具名一致（证明去重复成功）

**诚实性回归（对齐 `AGENTS.md` §9）**
- gate 引擎不可用时**不产出编造概率**（`confidence` 为 `undefined`）
- `calibrated` 标注不变

---

## 11. 风险与既有坑（逐条钉住）

**风险排序**
1. **`agent-loop.ts` 工具表重构**——触及主循环，靠"行为等价"测试钉住，单独 commit 便于回退。
2. **`afterTurn` 的出口易漏**——`turn_end` 在 `agent-loop.ts` 有**两个**出口
   （纯文本轮 `:378`、带工具轮 `:587`），`afterTurn` 漏一处就会静默丢事件。
   这类"成对却不完整"的缺陷最难发现，必须靠测试钉住两条路径。
3. **gate 失败路径与 `wakeParent`**——漏了会**永久挂起父会话**，是本次唯一可能造成"卡死"的缺陷，
   必须有专门测试。新增 `afterSubagentEnd` 回调后风险更高：
   回调的插入位置必须在 `runningThreadIds.delete()` 与 `wakeParent()` 之间，
   且**回调抛错绝不阻断唤醒**。改动 `startSubagentThread` 返回路径时**风险最高的单点**。
4. **第三方钩子同权（本版改为有意开放）**——jiti 无沙箱，`before*` 是新攻击面。
   早先版本靠"核心代拍的限制"来收敛这个风险；本版按「用户决定什么就做什么」
   改为**默认开放 + 能力开关（§6.4.2）+ UI 诚实呈现**。
   风险并没有消失，只是**从核心承担转移给用户承担**，因此配套义务必须做实：
   - 每个用到受限能力的插件，卡上必须显示"它正在做什么"（改文本 / 阻止删会话 / 替换提示词）；
   - 关掉开关后必须显示"受限"状态，**不允许静默失效**；
   - README 要有"开放意味着什么"的直白说明，不藏在文档深处。
   唯一保留的强制项是**工具集不可扩张**（§6.4.3）——放开它等于绕过审批机制本身。
5. **改名的机械性风险**——6 个插件文件 + `loader.ts` 的引用点要一次改全。
   这不是"兼容性"风险（无存量），而是**漏改导致编译失败**：`typecheck` 会直接抓住，
   所以这是所有风险里最低的一个。真正要盯的是 §4.3.3 的**等价性验证**——
   改名很容易顺带改错一个工具名或描述，而那不会被 typecheck 发现。
6. **`casual` 档误降级**——模型调不到工具会退化成多轮试错，反而更贵。降级保守 + 必须可见。
7. **钩子数量增长带来的每轮成本**——成对原则让点位翻倍（4 个点位 → 8 个回调），
   即使都是进程内调用，也会累加。所以：无插件注册该钩子时**完全跳过**
   （不进入 try/catch 与耗时统计），这一点必须在实现时确认。

**`AGENTS.md` 既有坑的对应**
1. **白名单脱钩**：新增任何面向子智能体的工具（如工具路由的 `find_tools`）必须同步
   `subagents/builtins.ts` 的 `allowedTools`，`builtins.test.ts` 是守门测试。
2. **`READ_ONLY` 失败安全**：只读新工具必须登记，否则只读子智能体与 plan 模式拿不到。
   判据是「是否改动工作区或系统状态」。
3. **`terminate` 语义**：`BeforeTurnResult.terminate` 是**结束整轮**，不是结束这批。JSDoc 写明 + 测试钉住。
4. **子智能体唤醒的 `finally`**：见风险 3。
5. **不用轮询代替等待**：钩子里要等结果就 await，不要轮询。
6. **两个门都要过**：`bun run typecheck` 与 `bun test` 独立，改核心后都跑。
7. **已知既有失败**：`src/ui/PluginsDialog.test.tsx` 的
   `creates custom prompt and applies prompt content to composer` 在未改动 HEAD 上同样失败，不算本次回归。

**本方案建议同时加进 `AGENTS.md` 的四条约定**：

第一条（点位成对，这是本方案的核心原则，必须写进约定否则会被后来者破坏）：
> **凡是有状态延续的点位，事前/事后钩子必须成对提供，且成对开放给插件。**
> 新增一个 `beforeXxx` 时必须同时加 `afterXxx`；只加一半会被
> `hooks-pairing.test.ts` 的接口层断言拦下。
> 例外只有**纯判定/纯通知类点位**（如 `check_gate`、`onThreadSwitch`）——
> 它们没有后续状态可观察，无需配对。
> 成对的理由不是对称美观，而是：插件要能**自我校准**（看到自己决策的实际结果）、
> **清理资源**、以及**可审计**（事前放行 + 事后记录才成轨迹）。

第二条（开放原则——本方案的第二个核心，同样必须写进约定）：
> **插件点位默认开放，用什么、放开到什么程度由用户决定，不由核心代拍。**
> 新增一个钩子点位时，默认要让它**能干预**，并把"放开后的后果"作为可配置项
> 交给用户（`pluginCapabilities`）；**不要**在核心硬编码"第三方不许做 X"这类判断。
> 核心的义务是两件事：**如实呈现**每个开关的后果、**忠实执行**用户的设置。
> 关闭某能力时，用到它的插件必须显示"受限"状态——**不允许静默失效**。
> 唯一例外：**钩子返回值不能扩张工具集**——工具集是审批闸门的依据，
> 放开等于绕过审批机制本身（理由详见设计文档 §6.4.3）。

第三条（第三方代码会执行，安全提示仍须保留）：
> 打开项目会执行 `.ada/extensions` 下的第三方代码（jiti 直执行，无沙箱）。
> 钩子默认对第三方开放，且默认可改写文本、可阻止删会话、可替换系统提示词——
> 这些都是**有意开放**的能力，可通过 `pluginCapabilities` 逐项关闭。
> 用户应当知道自己装了什么。

第四条（**本项目尚未正式使用，可以调整式改动**——这条会随时间失效，需注明期限）：
> a_da **尚未正式发布使用**：没有外部用户、没有已发布的插件生态、没有需要保住的历史配置。
> 因此在这一阶段，**重构优先于兼容**——
> 可以改类型名、改配置结构、改插件契约，**不留别名、不写迁移、不做双读路径**；
> 直接改到目标形态，并在同一个变更里把仓库内的调用点（6 个内置插件、
> `.ada/extensions/web-search.ts`、`config.json` 读取方）一次改完。
> 代价用**一次性等价性测试**兜住（改名前后的行为逐项对照），而不是长期的兼容负担。
> **注意**：正式发布后此条失效，届时新增/修改插件契约必须走正常的兼容流程。
> 判断依据是"是否有用户依赖"，不是"改动大小"。

---

## 12. 待确认问题

**已按「开放原则」定案、不再列为待确认的项**（早先版本并列在此，现已直接定稿）：
覆盖内置工具默认可覆盖（`allowBuiltinShadow`）、`afterTurn` 可改写文本
（`allowTextRewrite`）、`beforeAgentStart` 可替换系统提示词（`allowSystemPromptReplace`）、
`beforeCompaction` 可替换选择策略（`allowCompactionReplace`）、`beforeThreadDelete`
的 `block` 对所有插件开放（`allowThreadDeleteBlock`）、plan 模式钩子默认生效
（`allowPlanModeHooks`）、第三方可注册钩子（`allowThirdPartyHooks`）、
`gate.failOpen` 未配置时默认放行（§6.3）。以上均为**默认开放 + 可配置 + 必须可见**。

剩余待确认：

1. **`plugins/types.ts` 的最终位置**：注解式范围是新建 `src/agent/plugins/types.ts`
   作为单一事实来源，原 `builtin-plugins/types.ts` 与 `loader.ts` 的类型**直接迁过去**
   （不留 re-export，§4.3）。是否认可这个落点？
2. **必填配置缺失时不注册工具**，这个"要么好要么不出现"的策略是否合适？
   替代方案是注册但调用即失败。建议前者，因为它让状态在 UI 上可见
   （与"开放但必须可见"的原则一致）。
3. **门禁拦截是否也适用于 `invoke_subagent` 之外**（如 `manage_tool` 创建插件）？建议后续。
4. **`dependsOn` 是否需要 UI 展示依赖图**？本方案只做数据层 + 诊断，UI 可后续。
5. **`Thread.pluginData` 是否需要大小上限**？无限制的话插件可以把大对象塞进会话文件，
   影响落盘体积。建议加一个保守上限（如 64KB/插件）并在超限时**拒绝写入并可见报错**，
   而非静默截断——与"不允许静默失效"一致。
6. **能力开关的粒度**：全局 + 按插件覆盖（§6.4.2）是否够用？是否需要"按工作区"？
   现有的 `pluginCapabilities.overrides` 已按插件 id 覆盖，工作区维度可以再加一层。
7. **第二优先五项是否本次都做**？`beforePersist`（落盘脱敏）价值高但会触及每个消息；
   `beforeLlmRequest` 是"最后一刻裁剪"的唯一挂点。建议至少做这两项，其余可延后。
8. **插件 id 格式**：既然不留兼容（§4.3），`builtin:` / `workspace:` / `global:`
   前缀可以重新设计。是否要简化（如 scope 与 name 分开存，不再拼进 id）？
