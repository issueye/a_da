# 插件系统开发计划（含 MVP 里程碑）

> 依据：`docs/plugin-system-design.md`（设计文档，1414 行）
> 目标项目：a_da
> 状态：**待评审 → 逐步实施**
> 基线：本文档撰写时已实测（见 §2），非 UI 测试全绿

---

## 1. 计划总览

设计文档给出的是「目标形态」，不是一个可以直接开工的顺序。本计划把它切成
**4 个 MVP 里程碑**，每个里程碑都有：明确的交付物、可验证的验收标准、可回滚的边界。

```
M0 结构调整 ──→ M1 契约与加载层 ──→ M2 运行时干预（MVP 核心）──→ M3 管理与完善
   不兼容改名        可安装可诊断         成对点位 + 能力开关           UI + 会话生命周期
   （纯重构）        （无新能力）        （插件第一次能影响模型）      （可配置可观测）
```

**为什么这样切**：设计文档 §9.3 已定「第 0 步先做结构调整」，因为后续每一步都要建立在新契约上。
M1 与 M2 的分界是**"能不能干预"**——M1 结束时插件仍只是"一堆工具"（与现状等能力，但基础设施齐了），
M2 结束时插件才第一次能影响模型看到什么。这是本项目的价值拐点，所以 M2 是 MVP 核心。

**每个里程碑独立可交付**：M0 是纯重构（行为不变），M1 不改变插件能力上限，
M2 引入新能力但受开关控制，M3 补齐可见性与配置。任一里程碑中断，仓库都处于可发布状态。

---

## 2. 基线（动手前的实测，用于验收对照）

| 门 | 命令 | 基线结果 |
|---|---|---|
| 类型检查 | `bun run typecheck` | **通过**（exit 0） |
| 非 UI 测试 | `bun test src/agent` | **319 pass / 0 fail** |
| 全量测试 | `bun test` | 448 pass / **18 fail** |

**关于 18 个失败**：全部位于 `src/ui/`（`TabStrip` 10、`Sidebar` 8、`SettingsDialog` 8、
`PluginsDialog` 4、`Sidebar add project` 6，按测试名去重后共 18 条），
均为 GPUIX 渲染相关测试，**与本计划改动的 agent/插件层无关**。
`AGENTS.md` 只记录了其中 1 条（`PluginsDialog` 的 prompt 用例）为已知失败，
实际有 18 条——**这是 `AGENTS.md` 的记载不全，不是本次改动引入的回归**
（已在未改动的 HEAD 上复现）。

**因此本计划的验收门定为**：
1. `bun run typecheck` 必须 exit 0；
2. `bun test src/agent` 必须 **0 fail**（这是我的真实回归线）；
3. `bun test` 的失败数**不得超过 18**，且**不得新增失败用例名**。

> 建议后续顺手修正 `AGENTS.md` 第三节的"1 个红"为实际的 18 个（另立小任务，不在本计划内）。

---

## 3. MVP 里程碑

### M0 — 结构调整（纯重构，行为不变）✅ **已完成**

**状态**：已实施并验证通过（见本节末的完成记录）

**目标**：把契约改到目标形态，**不加任何新能力**。这一步单独成 commit，便于回滚。

**交付物**

| # | 任务 | 涉及文件 |
|---|---|---|
| M0-1 | `BuiltinPluginPackage` → `PluginDescriptor`，**不留别名** | `builtin-plugins/types.ts` + 6 个插件文件 + `index.ts` |
| M0-2 | 新建 `src/agent/plugins/types.ts` 作为插件契约单一事实来源；`PluginManifest` / `PluginContributions` / `PluginDescriptor` / `LoadedPlugin` | 新建 |
| M0-3 | 处理 `BUILTIN_TOOLS_METADATA`：**改名为 `BUILTIN_TOOLS_CATALOG` 并保留**（它是 23 个核心工具的展示目录，不是插件元数据）；**只删除其中与插件重复的 3 条决策工具**，插件工具改为从 `PluginDescriptor` 读取 | `registry.ts`、`PluginsDialog.tsx`、`tools.ts` |
| M0-4 | `config.json` 结构定稿（`pluginCapabilities` / `pluginConfig` / `workspacePluginState`），**不做旧结构迁移** | `config.ts` |
| M0-5 | 走 `readPluginConfig()` 统一配置读取；决策插件迁移，**不留旧变量名双读** | `config.ts`、`decision/config.ts` |
| M0-6 | 等价性测试：改名前后行为逐项对照 | 新建 `plugins/equivalence.test.ts` |

**验收标准**
- [ ] `typecheck` exit 0（改名漏改会被它直接抓住——这是本步最低风险的原因）
- [ ] `bun test src/agent` 0 fail
- [ ] 6 个插件的**工具名 / 描述 / 技能名 / 提示词名**与改动前逐项一致（等价性测试断言）
- [ ] 决策插件配置解析结果与迁移前一致
- [ ] 全量测试失败数 ≤ 18 且无新增用例名

**明确不做**：不加钩子、不改 `ExtensionContext`、不动 `agent-loop`。

**实测改动量**（已核对）：约 10 个文件、20 处引用。一次 PR 可完成。

#### M0 完成记录（已验证）

**门禁结果**（对照 §2 基线）

| 门 | 基线 | M0 后 | 判定 |
|---|---|---|---|
| `bun run typecheck` | exit 0 | exit 0 | ✅ |
| `bun test src/agent` | 319 pass / 0 fail | **326 pass / 0 fail** | ✅（+7 为新等价性测试） |
| `bun test` | 448 pass / 18 fail | **455 pass / 18 fail** | ✅ 失败用例名与基线**完全一致**（全是 UI） |
| `bun run build` | — | `dist/a-da.exe` 产出成功 | ✅ |

**实际改动**

| 文件 | 改动 |
|---|---|
| `src/agent/plugins/types.ts` | **新增**：`PluginManifest` / `PluginContributions` / `PluginDescriptor` / `LoadedPlugin` / `PluginConfigSchema` / `PluginDiagnostic` / `PluginStatus` |
| `src/agent/tools/builtin-plugins/types.ts` | 改为纯 re-export，指向新契约；**不留别名** |
| 6 个插件文件 + `index.ts` + `decision/index.ts` | `BuiltinPluginPackage` → `PluginDescriptor` |
| `src/agent/tools/registry.ts` | `BUILTIN_TOOLS_METADATA` → `BUILTIN_TOOLS_CATALOG`，**删除 3 条与插件重复的决策工具** |
| `src/ui/PluginsDialog.tsx` | 跟随改名（3 处） |
| `src/agent/config.ts` | `SavedConfig` 定型新结构；新增 `readPluginConfig` / `readPluginSecret` / `readPluginDisabled` |
| `decision/config.ts` | 迁移到 `readPluginConfig`；**删除** `PI_JEV_BASE_URL` / `TYPESAFE_*` / `saved.decision` 双读路径 |
| `decision.test.ts` | 旧变量名与旧 config 块测试 → 新契约测试 |
| `src/agent/plugins/equivalence.test.ts` | **新增**：8 条等价性测试 |

**实施中发现并修正的两处计划偏差**

1. **M0-3 原计划"删除 `BUILTIN_TOOLS_METADATA`"是错的。**
   实测它是 **23 个核心工具**的展示目录（不只是插件元数据），删了「内置工具」tab
   就没有标签与描述。改为**改名保留 + 只删除 3 条与插件重复的决策工具**，
   插件工具一律从 `PluginDescriptor` 读——这才真正消除重复来源。
2. **`readPluginConfig` 的环境变量映射有个真实缺陷，已在实施中修掉并加了注释**：
   - 只遍历 `merged` 的键，导致**只写在 config.json 里的键无法被环境变量覆盖**；
   - `key.toUpperCase()` 把 `baseUrl` 变成 `BASEURL`，而用户会写 `BASE_URL`——
     **设了却不生效，且不报错**。
   修法：键名取 `defaults ∪ fromFile` 的并集，并新增 `envKeyOf()` 按驼峰拆词
   （`baseUrl` → `BASE_URL`）。这条在 M1-6 声明 `configSchema` 时会直接受益。

**一个需要后续处理的事项**：`AGENTS.md` 第三节记录"已知失败 1 个"，
实测为 **18 个**（全在 `src/ui/`，GPUIX 渲染相关，与插件层无关）。
已在本计划 §2 记录实测值；`AGENTS.md` 的更正建议另立小任务。

---

### M1 — 契约与加载层（可安装、可诊断、可重载）✅ **已完成**

**状态**：已实施并验证通过（见本节末的完成记录）

**目标**：插件系统的**基础设施**就位——插件身份可追踪、冲突可见、失败可诊断、
配置可声明、可干净重载。**能力上限与现状相同**（插件仍只是工具+技能+提示词）。

**交付物**

| # | 任务 | 设计文档 | 涉及文件 |
|---|---|---|---|
| M1-1 | 两个加载器产出统一 `LoadedPlugin` | §4.3.2 | `loader.ts` |
| M1-2 | `register(tool, origin)` 记录 `pluginId`；`getToolOrigin` / `listByPlugin` | §5.1 | `registry.ts` |
| M1-3 | 冲突检测与去重（不再出现同名两份）；冲突报告 | §5.1 | `registry.ts` |
| M1-4 | 诊断列表 `getDiagnostics()`；加载失败不再静默 | §7.3 | `loader.ts` |
| M1-5 | 监听器按插件分组持有，重载时退订（修缺陷 4） | §5.5 | `loader.ts` |
| M1-6 | `PluginConfigSchema` + `required` 缺失 → `not-ready` 且不注册工具 | §5.2 | `plugins/types.ts`、`config.ts` |
| M1-7 | 按工作区启停 | §5.3 | `config.ts`、`loader.ts` |
| M1-8 | 依赖拓扑 + `engines` 版本检查（软失败 + 诊断） | §5.4 | `loader.ts` |
| M1-9 | 第三方扩展支持描述符导出形态 | §4.3.2 | `loader.ts`、`.ada/extensions/web-search.ts` |
| M1-10 | 自身诊断接入 `scripts/extension-check.ts` | §7.3 | `scripts/extension-check.ts` |

**验收标准**
- [x] `typecheck` exit 0；`bun test src/agent` 0 fail
- [x] 同名工具冲突：产生可见警告，工具表**不出现同名两份**
- [x] `getToolOrigin` 对内置/工作区/全局工具都返回正确 `pluginId`
- [x] 缺失依赖 → 插件标记 `broken`，其工具**不注册**
- [x] 缺必填配置 → `not-ready`，其工具**不注册**
- [x] 重载后**旧事件监听器已退订**（缺陷 4 回归测试）
- [x] 两种导出形态（描述符 / 函数）产出**等价的 `LoadedPlugin`**
- [x] 内置与第三方加载后**字段结构一致**

**里程碑价值**：插件作者第一次能知道"我的插件为什么没生效"。

#### M1 完成记录（已验证）

**门禁结果**

| 门 | 验收线 | M1 后 | 判定 |
|---|---|---|---|
| `bun run typecheck` | exit 0 | exit 0 | ✅ |
| `bun test src/agent` | 0 fail | **355 pass / 0 fail** | ✅ |
| `bun test` | 0 fail（见下方"验收门收紧"） | **502 pass / 0 fail** | ✅ |

**实际改动**

| 文件 | 改动 |
|---|---|
| `src/agent/plugins/types.ts` | 修掉"已预留 hooks/subagents 字段"的错误注释（字段并不存在）；新增 `PluginDescriptorExport`（第三方声明式导出形态） |
| `src/agent/plugins/registry.ts` | **新增**：已加载插件的索引（`setLoadedPlugins` / `getLoadedPlugins` / `getPluginDiagnostics`）。独立成模块是为了不让管理器反向 import 加载器（那会成环） |
| `src/agent/version.ts` | **新增**：`APP_VERSION` + `parseVersion` / `satisfiesRange`；`version.test.ts` 钉住它与 package.json 一致 |
| `src/agent/tools/registry.ts` | `register(tool, origin)` 记录 `pluginId`/`scope` 并返回冲突；新增 `getToolOrigin` / `listByPlugin` / `getConflicts` / `unregisterPlugin`；`getToolsForWorkspace` 按名字去重；`BUILTIN_TOOLS_CATALOG` 补上漏登记的 `resume_subagent` |
| `src/agent/tools/loader.ts` | 两条加载路径统一产出 `LoadedPlugin`；`parseExtensionModule` 识别描述符/函数/工具/数组四种导出；`finalizePlugins` 集中判定状态（依赖、版本、必填配置、冲突）；监听器改为 `Map<pluginId, Set>` 并在重载时退订；`getDiagnostics()`；诊断写事件日志（`[插件]` 前缀） |
| `src/agent/config.ts` | 新增 `setPluginDisabled`（按作用域写入）与 `savePluginConfig`（配置写入侧）；`createPluginDisabledResolver`（一次读取 + 判定函数）；`readPluginDisabled` 改为它的包装 |
| `src/agent/skills/manager.ts`、`src/agent/prompts/manager.ts` | 改为按工作区判定停用；新增第三方插件**内联** skills/prompts 的归纳（修缺陷 7） |
| `.ada/extensions/web-search.ts` | 迁移到声明式描述符形态（与内置插件同构） |
| `scripts/extension-check.ts` | 增加"插件诊断无 error"检查 |

**新增测试**（29 条）：`tools/registry.test.ts`（溯源/冲突/去重 7 条）、`plugins/loader.test.ts`
（9 条，逐条对应 M1 验收清单）、`version.test.ts`（9 条）、`plugins/equivalence.test.ts`
（+3 条：核心工具目录与真实工具表的一致性守门）、既有测试回归。

**实施中发现并修正的三处偏差**

1. **验收门收紧（推翻本文档 §2 的基线）**：§2 把 `bun test` 的线画在"失败数 ≤ 18"，前提是
   "UI 有 18 条既有失败"。该前提已被证伪——那 18 条是一条泄漏窗口造成的连带污染，修掉后
   全量测试是 0 fail。因此 M1 的验收门改用**全量 0 fail**，比原计划严格得多。
2. **一致性守门测试立刻抓到一处真实漏登记**：`resume_subagent` 是真实核心工具但不在
   `BUILTIN_TOOLS_CATALOG` 里，插件管理页的"内置核心工具"清单缺它。这正是缺陷 9 想防的漂移，
   已补上（新增的两条守门测试会长期盯着目录与工具表的双向一致）。
3. **"必填配置"缺一个真实消费者**：6 个内置插件里只有 decision 读配置，而它**不该**有任何
   必填项（没配密钥时它靠本地自评与启发式照样能用，设成必填会让整个插件消失）。因此
   `configSchema` 落在 decision 上只作**声明**（含 `apiKey` 的 `secret` 类型），`required` 的
   行为由合成插件在 `loader.test.ts` 里覆盖。

**一处刻意留下的产品决定**：`togglePlugin` 写的是**全局**停用表（插件管理页的开关是应用级
偏好），工作区级的 `workspacePluginState` 已可读写并生效，但"仅本工作区停用"的选择权在界面上
（M3）。两侧 API 都已落定，见 `config.ts` 的 `setPluginDisabled`。

**未在本次验证的一项**：`bun scripts/extension-check.ts` 需要联网 + 真窗口，其新增的
"诊断无 error"断言已写好但**未实跑**。

**明确不做**（仍属 M2/M3）：钩子与能力开关（M2）、插件卡状态徽标/诊断详情/配置表单（M3）。
界面上目前的可见改进只有一处：`broken` / `not-ready` 的诊断会汇总进插件卡的"加载告警"一行
（那是 `PluginItem.error` 的既有展示位）。

---

### M2 — 运行时干预 + 能力开关（**MVP 核心**）✅ **已完成**

**状态**：已实施并验证通过（见本节末的完成记录）

**目标**：插件第一次能**影响模型看到什么**。这是本项目的价值拐点，也是设计文档
两条核心原则（成对、开放）的落地。

**交付物**

| # | 任务 | 设计文档 | 涉及文件 |
|---|---|---|---|
| M2-1 | `src/agent/core/events.ts`：`AgentHooks` 成对契约 + 各 `before*`/`after*` 上下文类型 | §6.1 | 新建 |
| M2-2 | `agent-loop.ts`：工具表从"循环外算一次"改为"每轮算"（**行为等价重构**） | §6.2 | `agent-loop.ts` |
| M2-3 | `beforeTurn` / `afterTurn`（**两处 `turn_end` 出口都发**） | §6.2 | `agent-loop.ts` |
| M2-4 | `beforeAgentStart` / `afterAgentEnd` | §6.2.1 | `agent-loop.ts` |
| M2-5 | 能力开关表 `pluginCapabilities` 读取与强制执行 | §6.4.2 | `config.ts`、`CORE` |
| M2-6 | 唯一强制项：钩子返回值**只能收窄工具集** | §6.4.3 | `agent-loop.ts` |
| M2-7 | `ExtensionContext.registerHooks` + `PluginDescriptor.hooks` | §6.1 | `loader.ts`、`plugins/types.ts` |
| M2-8 | 成对性接口断言测试（漏配 `after*` 即红） | §10 | 新建 `core/hooks-pairing.test.ts` |
| M2-9 | `after*` 在 `before*` 被短路时**仍执行** | §6.4.4 | `agent-loop.ts` |
| M2-10 | 无插件注册该钩子时**完全跳过**（不进 try/catch 与计时） | §11 风险 7 | `agent-loop.ts` |
| M2-11 | `store.ts` 挂载（主循环 + 子智能体循环） | §6 | `store.ts` |
| M2-12 | 决策插件作为第一个消费者接入（自动模式 / 工具路由） | §6.5 | `decision/` |

**验收标准**
- [x] `typecheck` exit 0；`bun test src/agent` 0 fail
- [x] **无钩子时行为与改动前逐事件等价**（钉住工具表重构）
- [x] 返回 `tools` → 本轮 `llm_request.tools` 与 `streamModelChat` 收到该数组
- [x] **钩子抛错 → 沿用上一轮工具，循环继续**（不让插件打崩主循环）
- [x] `afterTurn` 在**两个 `turn_end` 出口都触发**（纯文本轮 + 带工具轮）
- [x] `afterTurn.effectiveToolNames` = **实际下发**的工具名，非意图
- [x] `afterTurn` 抛错不影响本轮结果；其耗时不计入 `llmDurationMs`
- [x] `terminate` → **跑完本轮后** break（不是当前批就断）
- [x] 任何开关组合下，钩子返回超集**必被裁回子集**
- [x] 每个 `pluginCapabilities` 开关关掉后确实生效，且**开/关行为可区分**
- [x] `after*` 在 `before*` 被短路时仍执行
- [x] 无插件注册钩子时零额外开销（可用计数断言）

> 两条与设计文档的**明确偏差**（都在完成记录里写了原因）：
> 1. `afterToolCall` 在工具被拦截时不触发——什么都没执行，没有需要清理的状态；
> 2. `afterTurn.replaceText` 与 `afterAgentEnd.appendNote` **未进契约**：本项目没有
>    "改写已渲染回复 / 向已结束的会话追加旁注"的交付通道，声明它们等于静默失效。

**这是 MVP 的最小可用形态**：M0-M2 完成后，一个插件已经能按轮次干预工具表、
能在会话开始时追加提示词、能在轮次结束时校验自己的决策是否生效。

**明确不做**：审批闸门、压缩、会话生命周期（留到 M3）。

#### M2 完成记录（已验证）

**门禁结果**

| 门 | 验收线 | M2 后 | 判定 |
|---|---|---|---|
| `bun run typecheck` | exit 0 | exit 0 | ✅ |
| `bun test src/agent` | 0 fail | **409 pass / 0 fail** | ✅ |
| `bun test` | 0 fail | **556 pass / 0 fail** | ✅ |

**分两个 commit 实施**（按本文档 §5 的风险对策："先重构后加钩子，两步分开"）

1. `2155990 refactor(agent-loop): 工具表改为每轮计算（行为等价）`——先立基线再改：
   新增 `core/loop-equivalence.test.ts`，在**改动前**跑通并把实际值抄下来（第一版凭直觉
   写的期望序列错了 4 处，全靠实跑纠正），然后才动 `agent-loop.ts`。
2. 本提交——钩子契约、运行层、能力开关、两个循环的挂载。

**实际改动**

| 文件 | 改动 |
|---|---|
| `src/agent/core/events.ts` | **新增**：`AgentHooks` 成对契约、各点位上下文/结果类型、`HOOK_PAIRS`、`NON_NEGOTIABLE_TOOL_TAIL`。与 `AgentEvent`（只读事实）刻意分开 |
| `src/agent/plugins/hook-runtime.ts` | **新增**：把多个插件的钩子合成一份给循环用。负责能力开关过滤、按加载顺序串行、`before*` 短路、逐点位超时（超时**放行**不视为拒绝）、耗时打点、**受限必须可见**（关掉的开关会写明原因） |
| `src/agent/core/agent-loop.ts` | 每轮 `beforeTurn` / 两个 `turn_end` 出口都发 `afterTurn`；`beforeAgentStart`（agent_start 之前）/ `afterAgentEnd`（`finally` 里，保证成对）；工具集**只能收窄**（取授权实例，防同名劫持）；按名字去重（防重复声明）；`toolsDurationMs`；钩子调用的 try/catch |
| `src/agent/core/types.ts` | `AgentLoopOptions` 增加 `hooks` / `hookContext` / `onNotice` |
| `src/agent/config.ts` | `PluginCapabilities` 八项开关 + `DEFAULT_PLUGIN_CAPABILITIES`（全开）+ `readPluginCapabilities`（三层：默认 → 全局 → 按插件/按工作区）；取值不合法会记进 `invalid` 由调用方说出来 |
| `src/agent/plugins/types.ts` | `PluginContributions.hooks` |
| `src/agent/tools/loader.ts` | 描述符里的 `hooks`、模块级 `hooks`、`ctx.registerHooks` 三种来源合并进 `contributions.hooks`；**broken / not-ready 的插件不接管决策点** |
| `src/agent/store.ts` | `composeHooks()`（每轮重新合成：中途启停与开关改动下一轮就生效）；主循环与两条子智能体循环都挂上，带 `kind`/`subagentId` |
| `src/agent/subagents/runner.ts` | 同步兜底路径同样挂钩子（插件不该因为"这次没挂会话"被跳过） |
| `decision/hooks.ts`（新）| **第一个真实消费者**：工具路由 + 回执核对，默认不干预 |

**新增测试（54 条）**：`core/loop-equivalence.test.ts`（6，行为基线）、
`core/hooks-pairing.test.ts`（6，成对性：接口层 + 循环层）、`core/turn-hooks.test.ts`（14，
M2 验收清单逐条）、`plugins/hook-runtime.test.ts`（19，开关/顺序/超时/异常 + 加载器集成）、
`decision/hooks.test.ts`（9，工具路由与回执）。

**实施中发现并修掉的两个真实缺陷**（都由新测试当场抓到）

1. **`afterTurn` 抛错会掀掉整个回合**：循环直接 `await` 钩子，异常一路冒到 `for await`，
   后续轮次全没了。设计文档 §6.2 改造点 3 要求的是**机制性**兜底，不是"运行层应该会兜住"
   ——现在循环有自己的 `callHook()`，抛错一律当"没有意见"并记 trace。
2. **钩子递回重复名字会让模型收到两条同名工具声明**：`toolSpecs` 从原始数组生成，
   而 `toolMap` 是 Map（去重）。现在签名与 specs 都**由收好的 map 生成**，工具名按集合语义处理。

**一处刻意的例外**（与设计文档 §6.4.4.1 的偏差，写在配对测试里）
`afterToolCall` 在工具被拦截时**不触发**：什么都没执行，没有需要清理的状态；让 shell 钩子
对一个从未运行的命令触发反而是错的。其余三对（agent / turn / 工具调用链的 before）都严格成对。

**明确未做**（留给 M3 或需要先拍板）

- `afterTurn.replaceText`：文本此刻早已流式送达界面，本项目没有"改写已渲染回复"的通道，
  声明它会变成静默失效——契约里**没有**这个字段，何时做取决于是否新增替换事件。
- `afterAgentEnd.appendNote`：同上，会话已结束、store 也不消费循环的最终消息，没有交付通道。
  （`appendText` 已实现：以再发一条助手消息的方式交付。）
- `beforeSubagentStart` / `afterSubagentEnd`（子智能体门禁）属 M3-4；`beforeApproval` /
  `beforeCompaction` 属 M3-5/6。这三个点位对应的能力开关（`allowThreadDeleteBlock` /
  `allowCompactionReplace`）已在配置里解析，但**钩子本身还没有**，因此开关暂时没有可关的对象。
- **决策插件的"自动模式"（每轮调一次引擎决定工具表）没有做**：那要让每轮多一次引擎调用
  （成本与延迟都翻倍），没有引擎时只能靠启发式——按本项目的原则，拿不到真实判断时不该假装
  有判断。已实现的是**确定性工具路由**（配置白名单，默认关），它是钩子机制的第一个真实消费者。
  要不要做引擎驱动的自动模式，需要先定一个明确的收益场景。

---

### M3 — 管理与完善（可见、可控、可配）🟡 **部分完成**

**状态**：M3-1（前半）、M3-4、M3-5、M3-6 已完成；M3-2、M3-3、M3-1 的后半、
M3-7、M3-8、M3-9 未做（见本节末的进度记录）

**目标**：把 M1/M2 的能力**暴露给用户**，并补齐设计文档 §6.6.2 第一优先的其余点位。

**交付物**

| # | 任务 | 设计文档 |
|---|---|---|
| M3-1 | `PluginsDialog` 插件卡增强：状态徽标、版本、冲突提示、诊断详情、贡献计数 | §7.1 |
| M3-2 | **能力开关面板** + "受限"状态提示（不允许静默失效） | §6.4.2 / §7.1 |
| M3-3 | 配置表单（由 `configSchema` 自动生成，secret 不回显） | §5.2 / §7.1 |
| M3-4 | 子智能体成对钩子 + `gate`（含 `afterSubagentEnd`，**重点验证 `wakeParent` 不漏**） | §6.3 |
| M3-5 | 审批闸门 `beforeApproval` / `afterApproval` | §6.6.2 |
| M3-6 | 压缩 `beforeCompaction` / `afterCompaction` | §6.6.2 |
| M3-7 | 会话生命周期（含 `Thread.pluginData`） | §6.7 |
| M3-8 | 第二优先点位：`beforeLlmRequest` / `beforeSystemPrompt` / `beforeSkillLoad` / `beforePersist` / `afterCheckpoint` | §6.6.2 |
| M3-9 | README 重写「扩展」一节 + `AGENTS.md` 四条约定 | §7 / §11 |

**验收标准**
- [x] UI 测试：插件卡显示状态与诊断原因（沿用既有弹窗用例，见进度记录的说明）
- [x] `gate` 未配 `failOpen` + 无引擎 → **放行**，且有"门禁未生效"提示
- [x] `gate.failOpen: false` + 无引擎 → 拦截
- [x] **gate 拦截路径调用后父会话不被永久挂起**（`wakeParent` 与 `runningThreadIds` 一致）
- [x] `store` 与 `runner` 的工具过滤结果逐工具名一致（去重复成功）
- [x] `afterSubagentEnd` 抛错**不阻断** `wakeParent`
- [ ] 能力开关面板 + "受限"状态提示（M3-2 未做，见进度记录）
- [ ] 配置表单（M3-3 未做，见进度记录）

---

#### M3 进度记录（截至本次）

**已完成**：M3-4（子智能体门禁）、M3-5（审批闸门）、M3-6（上下文压缩）、M3-1 前半（状态徽标 + 诊断详情）。

**门禁**：typecheck exit 0；`bun test src/agent` 426 pass / 0 fail；全量 **586 pass / 0 fail**（连跑三次稳定）。

**M3-4 的取舍**：门禁的失败方向按 §6.4.4.5 实现（未配 `failOpen` = 放行 + 提示），并且**显式
`failOpen: false` 时，判定方只报 `allowed` 而拿不出 `confidence`/`calibrated` 也照样拦**——
"拿不出依据"等于没判断，不能让判定方自说自话。判定本身由 decision 插件提供（核心不内置"怎么判断"）。
另外把三处重复的工具解析收成 `subagents/access.ts` 一份，门禁与工具集因此永远一致。

**M3-5 的一处超出文档的取舍**：`beforeApproval` 的 `allow` 在 **readonly 审批档位下被忽略**
（写日志说明）。文档只写了"插件可实现白名单工具免问"，没写它与用户档位冲突时谁优先。这里按
与 `failOpen: false` 同源的原则处理：**用户明确表态过的事，不让插件悄悄改掉**。`deny` 则总是
被采纳，且一个插件不能推翻另一个插件的否决（`beforeApproval` 链不短路）。

**M3-1 只做了前半**：状态徽标（待配置/版本不兼容/加载失败/工具名冲突）与诊断详情（含可操作建议）
已上卡片，并补了版本号显示。**未做**：贡献计数、冲突双方的对照展示（§7.1 的完整形态）。
UI 测试挂在既有的"内置核心工具"用例里：GPU 测试渲染器开的是真窗口，新开一条用例实测会让
标签栏那组五条用例集体翻红（`store.activeId` 停在别的会话上），所以改为在 `beforeAll` 里准备
一个"缺配置插件"、在已有用例里多切一次页——断言依然是真实的 painted 文本。

**未做（按设计文档 §8 的优先级排序）**
1. **能力开关面板**（M3-2）：八个开关目前只能手改 `config.json`（README 有说明），
   界面上无法逐项关掉，也看不到"某个插件因为哪个开关受限"。设计文档把"受限必须可见"
   列为开放原则的配套要求，所以这是 M3 里最该接着做的一项。
2. **配置表单**（M3-3）：`configSchema` 已经能被读取（M1），`savePluginConfig` 也提供了写入，
   缺的是按 schema 生成表单（含 secret 不回显）。decision 插件已有 7 个配置项可被它渲染。
3. **会话生命周期**（M3-7）：`Thread.pluginData`、`beforeThreadCreate`（title 建议 + data）、
   `beforeThreadDelete`（`block` 与 `archiveBeforeDelete`）、`onThreadSwitch`。
4. **第二优先点位**（M3-8）：`beforeLlmRequest`/`afterLlmResponse`、`beforeSystemPrompt`、
   `beforeSkillLoad`/`afterSkillLoad`、`beforePersist`、`afterCheckpoint`。
5. **收尾**（M3-9）：README 的钩子一节已在 M2 写过，但 M3 的新点位（审批/压缩/门禁）
   与开关面板的说明还没补；`AGENTS.md` 需要一节讲审批的效力不对称与压缩的可替换边界。

---

## 4. 逐里程碑的验收门（统一执行）

每个里程碑结束**必须**跑：

```bash
bun run typecheck          # 门一：exit 0
bun test src/agent         # 门二：0 fail（真实回归线）
bun test                   # 参照：失败数 ≤ 18，且无新增失败用例名
```

M0 与 M2 结束后额外跑 `bun run build`（产出单文件可执行），因为这两步改动核心。

**禁止**：只跑 `typecheck` 就宣布完成。`AGENTS.md` 明确两个门独立，都要过。

---

## 5. 风险与对策（按设计文档 §11 排序）

| 风险 | 里程碑 | 对策 |
|---|---|---|
| **工具表进循环的重构**（触及主循环） | M2-2 | 独立 commit + "逐事件行为等价"测试；先重构后加钩子，两步分开 |
| **`afterTurn` 漏出口**（`turn_end` 有两个） | M2-3 | 两条路径各有测试；这是"成对却不完整"类缺陷，最难发现 |
| **`afterSubagentEnd` 与 `wakeParent` 顺序**（可能永久挂起父会话） | M3-4 | 专门测试；回调抛错不得阻断唤醒；离线兜底路径单独补 |
| **第三方钩子同权**（jiti 无沙箱） | M2-5/6 | 工具集只收窄（唯一强制项）+ 能力开关 + UI 诚实呈现 |
| **改名漏改** | M0-1 | `typecheck` 直接抓住；真正的风险是顺带改错工具名/描述 → 等价性测试 |
| **`casual` 档误降级** | 不在 MVP | 后置；若做必须保守 + 可见 |
| **钩子数量增长的每轮成本** | M2-10 | 无插件注册该钩子时完全跳过 |

**跨里程碑的既有坑**（`AGENTS.md`，改核心必查）
1. **白名单脱钩**：新增面向子智能体的工具必须同步 `subagents/builtins.ts` 的 `allowedTools`，
   `builtins.test.ts` 是守门测试。
2. **`READ_ONLY` 失败安全**：只读新工具必须登记，否则只读子智能体与 plan 模式拿不到。
3. **`terminate` 语义**：是结束**整轮**，不是结束这批。
4. **子智能体唤醒的 `finally`**：见风险 3。
5. **不用轮询代替等待**：钩子里要等结果就 await。
6. **两个门都要过**。

---

## 6. 待确认问题（开工前需要定）

1. **M0 是否单独成 PR？** 建议是——纯重构、行为不变、便于 review 与回滚。
2. **M2 的 `afterTurn` 是否可以延后？** **建议不可以**——设计文档 §8 已论证：
   先只发 `beforeTurn` 会让插件在"以为自己生效"的状态下运行，
   而成对正是为了消除这一点。M2 的成对性是验收条件。
3. **M3 的 8 项任务是否全做？** M3-4（子智能体）与 M3-5（审批）价值最高、
   机制已在，建议优先；M3-8（第二优先点位）可延后。
4. **`plugins/types.ts` 的落点**：新建 `src/agent/plugins/types.ts`，
   原 `builtin-plugins/types.ts` 直接迁过去（不留 re-export）。是否认可？
5. **默认开关全开的确认**：`pluginCapabilities` 八个开关默认全部开放
   （设计文档 §6.4.2）。M2 实施时会以此为准。

---

## 7. 立即开始：M0 执行清单

按以下顺序做，每步跑一次 `typecheck`：

1. **M0-2**（先建新契约）新建 `src/agent/plugins/types.ts`：
   `PluginManifest` / `PluginContributions` / `PluginDescriptor` / `LoadedPlugin` /
   `PluginConfigSchema` / `PluginDiagnostic`，从设计文档 §4.1、§4.3.2、§5.2 抄定义。
2. **M0-1** 改 `builtin-plugins/types.ts` 为从新文件 re-export（**过渡一步**，M0 内删掉），
   然后逐个改 6 个插件文件 + `index.ts` + `decision/index.ts` 的类型标注。
3. **M0-3** `BUILTIN_TOOLS_METADATA` → **改名 `BUILTIN_TOOLS_CATALOG` 并保留**
   （实测修正：它是 **23 个核心工具**的展示目录，不是插件元数据，不能删——
   删了「内置工具」tab 就没有标签与描述了）。
   **只删掉其中 3 条与插件重复的决策工具**（`decide`/`design_decision`/`check_gate`），
   插件工具一律改从 `PluginDescriptor` 读。改 `PluginsDialog.tsx`(3 处) 与 `describeTool`。
4. **M0-4/5** `config.ts` 定稿新结构 + `readPluginConfig`；迁移 `decision/config.ts`。
5. **M0-6** 写等价性测试。
6. 跑三个门 + `bun run build`。

> 第 2 步的"过渡 re-export"是**同一个 commit 内的中间状态**，不是兼容层——
> commit 结束时 `builtin-plugins/types.ts` 应只保留类型定义或直接被删除。
