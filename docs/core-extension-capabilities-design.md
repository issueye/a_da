# 核心扩展能力补齐方案（core capabilities）

> **⚠ 本文已被 `docs/plugin-system-design.md` 取代/吸收。**
> 本文以「补齐与 pi-jev 的六行差异」为目标；后续确认真正的目标是**完善插件系统**，
> 因此重新组织为 `plugin-system-design.md`。本文中仍然有效的部分
> （`beforeTurn` 轮次拦截、工具档位 `casual`、子智能体 `gate`、能力边界）
> 已并入该文件的 §6，并保留更细的代码级落点，可直接参考：
> - §2 现状盘点 → 新文件 §2
> - §3 阶段 A `beforeTurn` → 新文件 §6.2
> - §4 阶段 B 工具档位 → 新文件 §6.2（作为 `beforeTurn` 的消费者示例）
> - §5 阶段 C 子智能体 gate → 新文件 §6.3
> - §6 阶段 D 插件事件 API → 新文件 §4 + §6.1（提为契约层）- §7 阶段 E 决策后端 SDK → 新文件 §8（结论不变：不做）
> - §8 跨阶段硬约束 → 新文件 §11
> 仅**新增**部分以新文件为准（缺陷清单、契约统一、加载层语义、管理层）。
>
> **⚠ 契约名已变更**：新文件按 §4.3（本项目尚未正式使用，可调整式改动）决定
> **直接把 `BuiltinPluginPackage` 重命名为 `PluginDescriptor`，不留别名**。
> 本文中出现的 `BuiltinPluginPackage` 请按 `PluginDescriptor` 理解。

> 输入：`docs/decision-plugin-design.md` §3 的差异对照表（pi-jev vs a_da）
> 目标项目：a_da
> 状态：**设计待评审**，尚未实现
> 前置：决策插件本身（`decide` / `design_decision` / `check_gate`）已实现并在工作区中

---

## 1. 结论摘要

对照表六行里，真正卡住 a_da 的不是「没有 Jev 后端」，而是**扩展面只有「注册工具」一种形态**。
pi-jev 的七层能力全都建立在 `ExtensionAPI` 的 `on(event, handler)` 之上；a_da 的对应物只有
`BuiltinPluginPackage`（纯数据）和 observe-only 的 `ExtensionContext.onEvent`，
两者都**不能改变控制流**。

其中红框那一行是总病根：

> **轮次拦截**：pi-jev `before_agent_start` 事件 ↔ a_da「扩展无法拦截轮次开始」

它一卡，连带三件事全做不了：决策插件的「自动模式」、工具表的**按轮收窄**、以及任何
「先判断再决定给模型看什么」的机制。而工具表**始终全量下发**这一点，决定了 a_da 的
工具路由必须反过来做（收窄而非激活），其可行性也完全依赖轮次拦截。

因此本方案的排序不是照抄对照表，而是**按依赖倒序**：

| 阶段 | 内容 | 为什么排这里 | 是否改核心 |
|---|---|---|---|
| A | 轮次拦截 `beforeTurn` | 总病根，B/C 的前置 | ✅ 必须 |
| B | 级联工具表 + 工具激活态 | A 的第一个消费者；直接省 token | ✅ 必须 |
| C | 子智能体 `gate` | A 的第二个消费者；把决策插件接进生命周期 | ✅ 必须 |
| D | 插件事件 API 统一 | 把 A/C 的能力开到内置插件与第三方扩展 | ✅ 必须 |
| E | 决策后端（System One） | 对照表第一行；**不阻塞任何东西**，独立并行 | ❌ 可选 |

A/B/C/D 是「框架」，E 是「后端」。**建议先做 A**——它只有一处改动、一个回调、
十几个消费者，却决定了后面全部三件事能否落地。

---

## 2. 现状盘点：每个差异面的真实落点

以下都是核实过的代码事实（文件:行号），方案以此为基。

### 2.1 轮次拦截 —— 不存在，且没有反馈通道

- `runAgentLoop(messages, config, options)`：`src/agent/core/agent-loop.ts:202`
- 轮次边界：`for (let step = 0; ...)` `agent-loop.ts:237`；`yield { type: 'turn_start' }` `agent-loop.ts:248`
- 循环的既有钩子只有三个，全是**工具级**的：
  `beforeToolCall`（`agent-loop.ts:439-458`）、`afterToolCall`（`522-542`）、
  `shouldStopAfterTurn`（`595-604`）

**关键事实**：`turn_start` 是**单向 yield**，生成器不读消费者的返回值
（`AgentEvent` 是纯判别联合，`core/types.ts:127-153`），所以事件发出去也改不了任何东西。

### 2.2 工具表 —— 一次算好，轮轮全发

```ts
// agent-loop.ts:209-221  ← 循环开始之前，只算一次
const tools = options.tools ?? []
const toolMap = new Map<string, AgentTool>()
for (const tool of tools) toolMap.set(tool.name, tool)
const toolSpecs = tools.map((t) => ({ type: 'function', function: {...} }))
```

- 每次模型调用复用同一个 `toolSpecs`：`agent-loop.ts:281`（`llm_request` 事件同理 `273`）
- 「激活」概念的现有替代品只有**按模式名称过滤**，且是**按用户轮**而非按模型轮：
  `store.send` → `defaultToolRegistry.getToolsForMode(...)`（`store.ts:2958-2962`）
  → `registry.ts:135-158`：`plan` 去写工具 / `create` 加两个 manage 工具 / `code` 全量。

全仓 grep `getActiveTools|setActiveTools|activeTools|toolSubset|pruneTools|beforeTurn|
beforeModelCall|onRoundStart` **零命中**。

### 2.3 子智能体 —— 无 gate，且两个入口重复实现过滤

- profile 契约：`SubagentProfile`（`src/agent/subagents/types.ts:35-69`），
  字段有 `allowedTools` / `disallowedTools` / `mode` / `enabled` / `maxSteps` /
  `modelOverride` / `scope` —— **没有 `gate`**。
- 启动入口一：`store.startSubagentThread`（`store.ts:1649-1659`），前置校验**只有 `enabled`**
  （`1677-1679`，不满足就 throw）。
- 工具过滤：`store.ts:1756-1776` —— 白名单 ∪ 黑名单 ∪ `mode==='readonly'` 去写工具，
  再追加 `notify_parent`；`beforeToolCall` 里**二次**强制只读（`1853-1860`）。
- 启动入口二：`SubagentRunner.run`（`subagents/runner.ts:35`），过滤逻辑**再抄一份**
  （`runner.ts:51-68`），只多一个可选的 `beforeToolCall` 透传（`runner.ts:28`）。
- 唯一的外部否决机制是** shell 钩子**：`HookManager.run` 的 `before_tool` 非零退出即拦截
  （`src/agent/hooks.ts:133-157`）——命令式、每次 fork 进程，不能用来做进程内判定。

### 2.4 插件与扩展 —— 两套契约，都没有事件

| | 内置插件 | 第三方扩展 |
|---|---|---|
| 契约 | `BuiltinPluginPackage`（`builtin-plugins/types.ts:10-33`） | `ExtensionContext`（`tools/loader.ts:21-30`） |
| 字段 | `id/name/description/tools/skills?/prompts?` | `workspace/trace/registerTool/onEvent` |
| 加载 | `autoLoadExtensions` 遍历 `BUILTIN_PLUGINS`（`loader.ts:418-453`） | jiti 直执行（`loader.ts:73`），无沙箱 |
| 事件 | **完全没有** | `onEvent` 只能**观察** |

`onEvent` 的观察性是被代码钉死的：`dispatchAgentEvent`（`loader.ts:87-95`）逐个 listener
try/catch 调用、**丢弃返回值**；事件在 `store.ts:3010` 的 `for await` 里**事后回放**。

注册表层面还有个信息损失：`ToolRegistry.register(tool)`（`registry.ts:72-74`）只存工具对象，
**不记录它来自哪个插件**，所以「按插件禁用/启用工具」「插件级事件」目前无从实现。

### 2.5 决策后端 —— 只有 HTTP 兼容端点，没有 SDK

- `package.json` 依赖只有 `eventsource-parser` + `jiti`，**没有 `@typesafe-ai/sdk`**。
- `engine.ts` 里 `JevEngine` 实际是 POST `${baseUrl}/systemOne`（`engine.ts:554`）并手写解析
  `JevRawAnswer`（`engine.ts:518`）—— 是端点兼容层，不是 SDK 集成。
- `LocalEngine`（`engine.ts:272`）走 `streamModelChat`，`temperature: 0.7`、**不带工具**，
  3 次采样投票占比，`calibrated` 恒 `false`（这是刻意设计，见 `AGENTS.md` §9）。

---

## 3. 阶段 A：轮次拦截 `beforeTurn`（最高优先）

### 3.1 设计目标

在**每次模型调用之前**给出一处进程内回调，允许三件事：
「改工具表」「注入消息」「否决/短路本轮」。且必须**失败安全**：回调抛错不能让整轮崩。

### 3.2 契约（改 `src/agent/core/types.ts`）

```ts
export interface BeforeTurnContext {
  /** 第几步（0-based），对应 agent-loop 的 step */
  step: number
  /** 已累积的会话消息（只读视图，副本） */
  messages: AgentMessage[]
  /** 本轮将要下发的工具（上一次决策的结果或初始 tools） */
  tools: AgentTool[]
  /** 运行上下文，透传调用方信息 */
  workspace?: string
  /** 会话标识，供插件定位（可能 undefined，插件须自行兜底） */
  threadId?: string
  /** 主/子智能体区分 */
  kind: 'main' | 'subagent'
  /** 子智能体 id（kind='subagent' 时有值） */
  subagentId?: string
}

export interface BeforeTurnResult {
  /**
   * 本轮工具覆盖。语义是「相对上一次的增量」：
   * 传 undefined = 沿用；传数组 = 设为该数组；传 'casual' = 见 §3.4 惰性语义
   */
  tools?: AgentTool[] | 'casual'
  /** 追加到本轮消息队列（不会写回 workingMessages 的历史，除非 persist） */
  extraMessages?: AgentMessage[]
  /** 直接结束整轮（相当于终止），endReason 用 reason */
  terminate?: boolean
  terminateReason?: string
}

// AgentLoopOptions 新增（types.ts:164-178）
beforeTurn?: (ctx: BeforeTurnContext, signal?: AbortSignal) =>
  Promise<BeforeTurnResult | undefined>
```

### 3.3 循环改造（`agent-loop.ts:208-284`）

要点只有四条：

1. **`toolMap` / `toolSpecs` 从「循环外一次算」变成「循环内每轮算」**
   （把 `209-221` 挪进 `237` 的循环体开头）。这是本阶段唯一的破坏性重构，
   务必保持现有行为等价，先用现有测试钉住。
2. 每轮算出 baseline `tools` 后，调 `beforeTurn`，按返回值覆盖；覆盖结果就是本轮的
   `toolMap` + `toolSpecs`，用于 `turn_start` 之后的 `llm_request` 与 `streamModelChat`。
3. `beforeTurn` 整体包在 try/catch 里：**抛错时记 trace 并沿用上一轮工具**，绝不让扩展
   把主循环打崩——这是新增的失败安全面，参照 `isWriteTool` 的取向（保守但不致命）。
4. `sig = 工具名集合排序后 join(',')`，与上一轮相同则**复用同一份 `toolSpecs` 引用**，
   避免无意义抖动（也让 `llm_request` 事件的旁观者能靠引用判断「本轮工具未变」）。

`terminate` 语义严格对齐现有 `terminateBatch`：**跑完本轮剩余流程后 `break` 整轮**
（`agent-loop.ts:589-592` 的既有做法），不是「结束这一批」。这一点写进 JSDoc，
因为 `AGENTS.md` 第 5 条已经踩过「以为是批次控制」的坑。

### 3.4 惰性工具表 `'casual'`（省 token 的关键）

工具名 + 参数 schema 的 JSON 是**每轮都发**的固定成本。`thread.lastToolSpecsChars`
已经在统计它的大小（`store.ts:2975`），说明这笔开销早就被注意到了，只是没有行动。

`'casual'` 是**按需降级**语义：把工具表砍到常驻核心集。

```ts
// src/agent/tools/registry.ts
export const CASUAL_TOOL_CORE: string[] = [
  'read_file', 'read_files', 'write_file', 'edit_file', 'edit_files',
  'list_files', 'search_files', 'run_command', 'todo',
  'invoke_subagent', 'await_subagents', 'Skill',
]
```

**绝对不能进 `'casual'` 的**：`batch-ops`、`decision`、`git-tools`、`code-outline`、
`project-inspector`、`test-runner` —— 这些正是懒加载的目的。

**依赖闭包问题（必须处理）**：`'casual'` 裁掉 `invoke_subagent` 所需的边界工具会让模型
「派不出去活」；裁掉 `Skill` 会让技能加载断链。所以降级**必须保留核心集**，
且核心集里必须含 `invoke_subagent` / `await_subagents` / `Skill`（上面已含）。
子智能体侧同理（见 §3.5）。

**渐进披露协议**：模型要用的工具不在表里时，若强行调用会得到「未知工具」。
所以配一个常驻元工具：

```ts
// 名字待定：find_tools / tool_search
输入：{ need: string }            // 自然语言描述要做什么
输出：{ tools: [{ name, description, schema }] }  // 匹配到的工具的完整 schema
```

匹配先用**关键词/名称打分**（零成本、确定性、可测），不要一上来就接决策引擎——
决策引擎版本（§3.6）作为后续增强，且要显式标注成本。

### 3.5 子智能体也要走 `beforeTurn`

`store.startSubagentThread` 的 `runAgentLoop` 调用（`store.ts:1842-1861`）与
`resumeSubagentThread`（`2252-2269`）目前都没有任何 turn 级钩子。子智能体的
工具表同样是开局固定、轮轮全发，同样吃 `'casual'` 的红利，所以两处都要挂。

但**子智能体不得自行扩张授权**：`beforeTurn` 只能返回初始工具表的**子集**，
由 store 在包装层强制（不是靠插件自觉）：

```ts
// store 侧包装（示意）
beforeTurn: async (ctx) => {
  const r = await userHook?.(ctx)
  if (!r?.tools || r.tools === 'casual') return r
  const allowed = new Set(initialToolNames)   // 开局过滤后的白名单快照
  return { ...r, tools: r.tools.filter((t) => allowed.has(t.name)) }
}
```

这与既有的「白名单过滤 + `beforeToolCall` 二次强制只读」（`store.ts:1756-1776, 1853-1860`）
是同一种双重保险思路。

### 3.6 决策插件的第一个消费者

有了 `beforeTurn`，决策插件文档 §9 里标「❌ 需改核心」的两条才有落点：

- **自动模式**（每轮一次决策）：`beforeTurn` 里跑一次轻量 `noul`/`choice`，
  直接改变工具表。**必须显式配置开启**，并在 README 成本表里明示「每轮 N 次请求」。
- **工具路由（反向收窄）**：由 §3.4 的关键词 `find_tools` 升级为决策驱动，
  在 `beforeTurn` 里判定「本轮真正需要哪几个工具」。

两者的共同红线（沿用决策插件三条铁律）：**拿不到真实判断就失败，不要编**；
`'casual'` 降级失败时**退回全量工具表**（安全方向），不是退回空表。

---

## 4. 阶段 B：级联工具表与激活态

### 4.1 三个状态

| 状态 | 工具表 | 触发 | 成本 |
|---|---|---|---|
| `full` | 全量（按模式过滤后） | 默认；降级失败退回 | 0（token 最多） |
| `casual` | `CASUAL_TOOL_CORE` + `find_tools` | 连续 N 轮只用核心工具（默认 N=3），或 `beforeTurn` 显式返回 | 0（省 token） |
| `routed` | 决策选出的子集 + `find_tools` | 显式开启的决策路由 | 每轮 1 次决策请求 |

### 4.2 状态机放在 store，不放在 loop

`runAgentLoop` 保持**无状态**：它只负责「每轮问一次 `beforeTurn`」。状态
（当前档位、连续核心轮计数）放在 `store` 的 `Thread` 上，理由是：
store 已有 `thread.lastToolSpecsChars`（`store.ts:2975`）这类每轮元数据，
且 UI（`Composer.tsx:145,229`）已经在读 `lastToolSpecsChars` 展示工具表大小——
档位天然可以在同一处展示给用户。

`Thread` 建议新增：

```ts
toolTier?: 'full' | 'casual' | 'routed'
coreOnlyStreak?: number        // 连续只用核心工具的轮数
lastToolTierReason?: string    // 为什么降级/升级，给调试面板
```

### 4.3 UI 透明度（不可省）

用户必须看得见工具表被收窄过，否则遇到「明明有工具却调不到」会当成 bug：

- `Composer.tsx`：把 `lastToolSpecsChars` 的展示从「工具表大小」升级为
  「`full` / `casual` / `routed` + 字符数」徽标；
- 工具卡头部（`describeTool`，`src/agent/tools.ts`）在 `casual` 档下对未激活工具
  给一行说明：「本轮未激活，可用 `find_tools` 索取」。

---

## 5. 阶段 C：子智能体 `gate`

### 5.1 契约（`src/agent/subagents/types.ts`）

```ts
export interface SubagentGateContext {
  subagentId: string
  /** 触发方的会话 id（主智能体或其他子智能体） */
  parentThreadId: string
  task: string
  additionalContext?: string
  /** 可选：被判断的材料（通常是 task + context） */
  state: string
}

export interface SubagentGateResult {
  allowed: boolean
  /** 0–1，来自引擎；无引擎时为 undefined（不要编 0.5） */
  confidence?: number
  /** 人类可读理由，会进通知与事件流 */
  reason?: string
  /** 是否经过校准，对齐 DecisionAnswer.calibrated 的既有语义 */
  calibrated?: boolean
  /** 失败方向：默认 fail-close */
  failOpen?: boolean
}

export interface SubagentProfile {
  // ...既有字段不变
  /** 子智能体启动门禁。不配 = 直接放行（保持现状） */
  gate?: {
    /** 验收标准（自然语言），交给决策插件判定 */
    criteria: string
    /** 阈值，默认沿用 DEFAULT_GATE_THRESHOLD（0.7） */
    threshold?: number
    /** 无引擎时是否放行，默认 false（fail-close） */
    failOpen?: boolean
  }
}
```

### 5.2 失败方向：默认 fail-close，理由写进代码注释与测试

这是本阶段**唯一有争议**的默认值，必须把代价不对称写清楚：

- 门禁的用途是「防止不该跑的子智能体跑起来（烧 token / 改工作区）」。
- fail-open 意味着「没配引擎 ⇒ 门禁永远放行 ⇒ 形同虚设」，且用户**不会察觉**。
- fail-close 意味着「没配引擎 ⇒ 子智能体起不来」，用户**立刻会察觉**并去配置。
- 对照 `check_gate` 的既有默认（`gate.ts:120-143`，启发式下默认 `passed: false`，
  除非显式 `failOpen`）——两处必须一致，否则同一套语义在两个入口行为不同。

因此：`gate.failOpen` 显式配置才放宽，且放宽行为要在事件流里留痕。

### 5.3 落地：一处实现，两个入口共用

现状是 `store.startSubagentThread`（`1756-1776`）和 `runner.ts:51-68` **各抄了一份过滤**。
本次不再抄第三份，而是抽出共用函数：

```ts
// src/agent/subagents/access.ts（新文件）
export function resolveSubagentTools(
  profile: SubagentProfile,
  allTools: AgentTool[],
  registry: ToolRegistry,
): AgentTool[]

export async function runSubagentGate(
  profile: SubagentProfile,
  ctx: SubagentGateContext,
  signal?: AbortSignal,
): Promise<SubagentGateResult>   // 未配 gate 时直接 { allowed: true }
```

然后：

- `store.startSubagentThread`（`store.ts:1756` 起）与 `resumeSubagentThread`（`2252` 起）
  改调 `resolveSubagentTools`；`SubagentRunner.run`（`runner.ts:51-68`）同样改调。
- gate 检查插在 `enabled` 检查（`store.ts:1677-1679`）**之后**、构会话**之前**——
  不通过就**不创建 thread**（省掉挂 tab、建消息、起循环的全部开销），
  并返回结构化 `{ ok: false, reason }` 给 `invoke_subagent` 工具；
- `invoke_subagent`（`src/agent/tools/builtins/subagent.ts:98`）把 gate 结论透传进工具结果，
  让主智能体看到「被门禁拦下了，原因是 X」，而不是一个语焉不详的失败。

### 5.4 决策插件的第二个消费者

`gate.criteria` 直接喂给决策插件既有的 `runGate`（`decision/gate.ts:103`），
复用 `DecisionEngine` 抽象与 `calibrated` 标注——**不新建判定路径**，
避免出现第二套「阈值」「校准」语义。

**边界（明确写清）**：子智能体内部的工具级拦截是另一件事，走既有的
`beforeToolCall`（`store.ts:1853-1860`），**不要**和 gate 混为一谈。

---

## 6. 阶段 D：插件事件 API 统一

### 6.1 问题

能力全在核心（A/B/C），但**只有核心自己能调用**：
`BuiltinPluginPackage` 无事件字段；第三方 `onEvent` 丢弃返回值（`loader.ts:87-95`）。
不开这个口，内置插件（含决策插件）永远只能是「一堆被动工具」。

### 6.2 事件契约

```ts
// src/agent/core/events.ts（新文件）—— 与 core/types.ts 的 AgentEvent 分开，
// 因为 AgentEvent 是「上报给 UI 的已发生事实」，这里是「可干预的决策点」
export interface AgentHooks {
  beforeTurn?: (ctx: BeforeTurnContext) => Promise<BeforeTurnResult | undefined>
  beforeSubagentStart?: (ctx: SubagentGateContext) => Promise<SubagentGateResult | undefined>
  beforeToolCall?: (ctx: BeforeToolCallContext) => Promise<BeforeToolCallResult | undefined>
}
```

### 6.3 两套契约都开这个口

- `BuiltinPluginPackage` 增可选 `hooks?: AgentHooks`（`builtin-plugins/types.ts:10-33`）。
  纯数据契约的定位不变，`hooks` 是可选扩展。
- `ExtensionContext` 在 `onEvent`（只读观察，语义不变，向后兼容）之外，
  增 `registerHooks(hooks: AgentHooks): void`（`loader.ts:21-30`）。

### 6.4 同步改注册表：插件身份不能丢

`ToolRegistry.register(tool)`（`registry.ts:72-74`）需要增加可选 `pluginId`，
`autoLoadExtensions` 注册内置/工作区/全局工具时传入（`loader.ts:418-453`）。
否则「按插件禁用工具」「插件级事件」都无从实现。

### 6.5 安全边界（必须显式声明，不留默认）

内置插件是**随应用一起发布的本地代码**，第三方扩展是**jiti 直执行的任意 code**
（`loader.ts:73`，无沙箱）。两者信任级别不同，不能同权：

- `beforeTurn` 的返回**永远不能扩大**工具集：只允许取 `initialTools` 的子集
  （与 §3.5 对子智能体的约束同一机制，收敛到一处实现）；
- 扩展的 `beforeTurn` **默认只在交互模式（`code`）生效**；`plan` 模式必须显式配置才允许；
- 每个 hook 的耗时记入 trace，并在超过阈值（建议 500ms）时警告——
  否则一个慢扩展会静默拖慢每一轮。

---

## 7. 阶段 E：决策后端（可选，独立并行）

### 7.1 定位修正

对照表第一行「决策后端：`@typesafe-ai/sdk` 的 `systemOne()` ↔ 无」看起来最像缺口，
但**它不是任何能力的阻塞点**：`JevEngine`（`engine.ts:528`）已经覆盖了 Jev 后端的
**全部功能语义**。现在缺的只是「用官方 SDK 而非手写 HTTP」。

### 7.2 判断：不引入 SDK

理由（写在这里免得反复讨论）：

1. 引入 `@typesafe-ai/sdk` 会改变 a_da 的**部署前提**：现在是「Bun + 一个 OpenAI 兼容
   endpoint 即可跑」，引入后变成「可能需要额外服务/账号」，与「单文件可执行 + 本地优先」
   的定位冲突。
2. 现有 HTTP 层已经能工作（`engine.ts:554` + `JevRawAnswer` 解析），
   SDK 带来的增量主要是类型安全与自动重试，**不是能力**。
3. SDK 版本漂移会把 a_da 的核心插件绑到外部发布节奏上。

### 7.3 如果确实要做

- 做成**可选 peerDependency + 动态 `import()`**，检测不到就退回现有 HTTP 实现，
  保证「不装 SDK 也能跑」；
- `JevEngine` 内部拆成 `JevHttpTransport` / `JevSdkTransport` 两个实现，共用
  `DecisionEngine` 接口（`decision/types.ts:73-78`），对上层零改动；
- 无论走哪条路，`calibrated: true` 的判定依据必须仍是「专用 System One 模型」这一事实，
  **不能因为换了 SDK 就放宽标注**（`AGENTS.md` §9 第 2 条）。

---

## 8. 跨阶段硬约束（来自既有踩坑记录，逐条钉住）

改核心最容易触发的是「改了没人看得见」类问题。以下每条都对应 `AGENTS.md` 里的既有坑：

1. **白名单脱钩**：任何新增的**面向子智能体的工具**（如 §3.4 的 `find_tools`）
   必须同步更新 `subagents/builtins.ts` 的 `allowedTools`，否则子智能体拿不到
   （`AGENTS.md` §1）。`builtins.test.ts` 是守门测试，会红。
2. **`READ_ONLY` 失败安全**：`find_tools` 是**只读**（只读工具表、不改工作区），
   必须加进 `ToolRegistry.READ_ONLY`（`registry.ts:31-67`），
   否则只读子智能体与 plan 模式都拿不到（`AGENTS.md` §2）。
   §3.6 的决策路由工具若会产生模型请求，仍算只读；但若它会写缓存文件，则不得进 `READ_ONLY`。
3. **`terminate` 语义**：`BeforeTurnResult.terminate` 是「结束整轮」，不是「结束这批」
   （`AGENTS.md` §5）。JSDoc 必须写明，且要有测试钉住。
4. **子智能体唤醒的 `finally`**：本次若改动 `startSubagentThread` 的返回路径
   （§5.3 的 gate 拦截会新增一条提前返回），必须确认 `wakeParent()` 仍在
   `runningThreadIds.delete()` 之后调用，且离线兜底路径单独补一次唤醒（`AGENTS.md` §7）。
   这是本次改动**风险最高的单点**——gate 失败路径若从 `return` 出去而漏了 `finally`，
   父会话会永久挂起。
5. **不用轮询代替等待**：`beforeTurn` 里若要等子智能体或决策结果，
   用 `await_subagents` / 直接 await，不要轮询（`AGENTS.md` §8）。
6. **检查点与改动审阅**：本方案不新增写工具，因此不改
   `CHECKPOINT_TOOLS` / `checkpointPathsOf`。但若将来把决策路由做成「自动编辑」，
   必须补这两处（`AGENTS.md` §4）。
7. **两个门都要过**：`bun run typecheck` 与 `bun test` 是**两个独立门**，
   改核心后两个都跑（`AGENTS.md` 二）。
8. **已知既有失败**：`src/ui/PluginsDialog.test.tsx` 的
   `creates custom prompt and applies prompt content to composer` 在未改动的 HEAD 上
   同样失败，不算本次回归（`AGENTS.md` 三）。

---

## 9. 文件清单与实施顺序

### 9.1 新增文件

```
src/agent/core/events.ts                    # AgentHooks / BeforeTurnContext / BeforeTurnResult
src/agent/tools/registry.ts                 # CASUAL_TOOL_CORE 常量 + find_tools 注册（改既有）
src/agent/tools/builtin-plugins/router/     # 阶段 B：find_tools 独立小插件
  ├── index.ts
  └── tools.ts
src/agent/subagents/access.ts               # 阶段 C：resolveSubagentTools / runSubagentGate
src/agent/core/before-turn.test.ts          # 阶段 A 测试
src/agent/subagents/access.test.ts          # 阶段 C 测试
```

### 9.2 必须同步修改的既有文件

| 文件 | 改动 | 阶段 | 为什么 |
|---|---|---|---|
| `src/agent/core/types.ts` | `AgentLoopOptions` 加 `beforeTurn`；新增 ctx/result | A | 回调入口 |
| `src/agent/core/agent-loop.ts` | 工具表进循环；每轮算 + 调 `beforeTurn` + try/catch + sig 缓存 | A | 总病根 |
| `src/agent/tools/registry.ts` | `CASUAL_TOOL_CORE`；`READ_ONLY` 加 `find_tools`；`register` 加 `pluginId` | A/B/D | 激活与身份 |
| `src/agent/tools/builtin-plugins/index.ts` | 注册 router 插件 | B | 否则不加载 |
| `src/agent/tools.ts` | `describeTool` 加 `find_tools` case；`casual` 档下未激活工具说明 | B | 工具卡摘要 |
| `src/agent/store.ts` | `Thread` 加 `toolTier` 等；`send` 挂 `beforeTurn`；子智能体两处挂 `beforeTurn` + gate | A/B/C | 状态与集成 |
| `src/agent/store.ts` | `CHECKPOINT_TOOLS` / `checkpointPathsOf` | — | 本方案无写工具，**不改**（改动时再补） |
| `src/agent/subagents/types.ts` | `SubagentProfile.gate` | C | 契约 |
| `src/agent/subagents/builtins.ts` | 白名单补 `find_tools` | B | `AGENTS.md` §1 |
| `src/agent/subagents/builtins.test.ts` | 白名单守门断言 | B | 防漂移 |
| `src/agent/subagents/runner.ts` | 改调 `resolveSubagentTools` + gate | C | 去重复 |
| `src/agent/subagents/manager.ts` | 解析 `.json` / frontmatter 里的 `gate` | C | 第三方 profile 也要能配 |
| `src/agent/tools/builtins/subagent.ts` | `invoke_subagent` 透传 gate 结论 | C | 让主智能体知道被拦原因 |
| `src/agent/tools/loader.ts` | `ExtensionContext.registerHooks`；`register` 传 `pluginId` | D | 开事件口 |
| `src/agent/tools/builtin-plugins/types.ts` | `BuiltinPluginPackage.hooks?` | D | 开事件口 |
| `src/ui/Composer.tsx` | 档位徽标 | B | 透明度 |
| `README.md` | 工具档位 / gate / 成本表 | 全部 | 文档一致性 |
| `AGENTS.md` | 新增「轮次拦截与工具档位」一节 | A 完成后 | 给后来者 |

### 9.3 顺序与验收

1. **阶段 A**：`types.ts` + `agent-loop.ts`（含「工具表进循环但行为等价」的重构测试）
   → 加 `beforeTurn` → 跑两个门 → `bun run build`。**此步单独可交付、可回退。**
2. **阶段 B**：`CASUAL_TOOL_CORE` + `find_tools` + store 状态机 + UI 徽标
   → 子智能体白名单与守门测试。
3. **阶段 C**：`access.ts` + `gate` 契约 + 两个入口改造 + `invoke_subagent` 透传
   → **重点验证 gate 失败路径的 `wakeParent` 不漏**。
4. **阶段 D**：`AgentHooks` + 两套契约开口 + `pluginId`。
5. **阶段 E（可选）**：仅在明确需要时做，且不得破坏「不装 SDK 也能跑」。

每阶段结束跑 `bun run typecheck` 与 `bun test`（两个独立门），最终 `bun run build`。

---

## 10. 测试计划

**阶段 A（最重要，因为它改动主循环）**
- 无 `beforeTurn` 时行为与改动前**逐事件等价**（钉住重构不改行为）
- `beforeTurn` 返回 `tools` → 本轮 `llm_request.tools` 与 `streamModelChat` 收到该数组
- 返回 `undefined` → 沿用上一轮
- **抛错 → 沿用上一轮工具，循环继续**（失败安全，绝不崩）
- 工具集未变的轮次**复用同一 `toolSpecs` 引用**（sig 缓存）
- 返回 `terminate: true` → **跑完本轮后** `break`，`endReason` 符合预期（不是当前批就断）

**阶段 B**
- `CASUAL_TOOL_CORE` 保留 `invoke_subagent` / `await_subagents` / `Skill`（依赖闭包）
- `'casual'` 降级**失败时退回 `full`**，不是空表
- `find_tools` 被 `isWriteTool` 判为**只读**
- 子智能体在 `casual` 档下仍能 `invoke_subagent` 边界工具（若能派活）

**阶段 C**
- 未配 `gate` → 直接放行（**现状不回归**）
- 配了 `gate` 且引擎可用、`confidence < threshold` → **不创建 thread**
- 配了 `gate` 且引擎不可用、`failOpen` 未设 → **不允许**（fail-close）
- `failOpen: true` + 引擎不可用 → 允许，且留痕
- `.json` / frontmatter 里的 `gate` 解析正确（`manager.parseJsonSubagent` / `parseMarkdownSubagent`）
- **gate 拦截路径调用后，父会话不被永久挂起**（`runningThreadIds` 与 `wakeParent` 一致）
- `store` 与 `runner` 的过滤结果**逐工具名一致**（证明去重复成功）

**阶段 D**
- 扩展 `registerHooks` 后能拦截；返回值被采纳
- `onEvent` 行为**不变**（向后兼容）
- `beforeTurn` 的返回**无法扩大**工具集（传超集 → 被裁回子集）
- 非交互模式下扩展 hook **默认不生效**

**诚实性回归（跨阶段，对齐 `AGENTS.md` §9）**
- `gate` 引擎不可用时**不产出编造概率**，`confidence` 为 `undefined` 而非假值
- 决策路由工具返回的 `calibrated` 仍如实标注

---

## 11. 成本与风险

**新增固定开销：零。** 阶段 A/B 的 `beforeTurn` 不产生模型请求（`find_tools` 走关键词匹配）。

**可选开销（必须显式开启且明示）**：

| 入口 | 模型请求数 | 默认 |
|---|---|---|
| `find_tools`（关键词版） | 0 | ✅ 开启 |
| 决策路由 `routed` 档 | 1 / 轮 | ❌ 关闭 |
| `gate`（本地引擎） | N = samples（默认 3）/ 次 | 配置后开启 |
| `gate`（Jev 引擎） | 1 / 次 | 配置后开启 |
| `gate` 无引擎 | 0（fail-close，不判定） | — |

**风险排序**：

1. **`agent-loop.ts` 工具表重构**（阶段 A）—— 触及主循环，必须靠「行为等价」测试钉住，
   建议单独一个 commit 便于回退。
2. **gate 失败路径与 `wakeParent`**（阶段 C）—— 漏了会永久挂起父会话，
   是本次唯一可能造成「卡死」的缺陷，必须有专门测试。
3. **第三方扩展同权**（阶段 D）—— jiti 无沙箱，`beforeTurn` 是新攻击面。
   子集约束是硬边界，不能靠文档自觉。
4. **`'casual'` 误降级**（阶段 B）—— 模型调不到工具时会退化成「多轮试错」，
   反而更贵。所以降级要保守（连续 N 轮核心工具才降）且**必须可见**。

---

## 12. 待确认问题

1. **阶段 A 是否单独立项先做？** 建议是——它一处改动解开 B/C/D 三件事。
2. **`'casual'` 的触发阈值 N=3（连续核心轮数）是否合适？**
   过小会抖动（频繁伸缩反而破坏前缀缓存），过大省不到。
3. **`find_tools` 的命名与工具名？** 候选 `find_tools` / `tool_search` / `load_tools`。
4. **子智能体 gate 默认 fail-close 是否会挡住正常使用？**
   表现为「配了 criteria 但没配引擎 ⇒ 子智能体起不来」。需要在 UI 上有明确提示，
   否则用户只会觉得「子智能体坏了」。
5. **第三方扩展的 `beforeTurn` 是否默认只允许交互模式？**
   本方案建议「是」，但这会限制扩展的表达力，需确认。
6. **`pluginId` 进注册表是否需要一并支持「按插件禁用工具」的 UI？**
   本方案只做数据层，UI 可后续。
