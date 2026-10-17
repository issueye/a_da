# 脚本系统与插件平台设计（提案）

> **定位**：本文是**提案**，不是第二口径。落地时按 AGENTS.md 的规矩并入
> [docs/agent-base-design.md](agent-base-design.md)（设计）与
> [docs/agent-base-plan.md](agent-base-plan.md)（分期与逐条处置），本文随即可删。
> 现状事实部分全部带 `文件:行号`，可复核；设计部分标注了与既有 INV/门禁的接口。
>
> **范围**：① 埋点方案（地基）② 脚本系统（执行面设计 + `ada-ts-engine` 自身待完善清单）
> ③ 统一贡献模型（系统提示词 / 技能 / 工具 / 钩子 都是插件贡献，并接入脚本系统）
> ④ **点位前后契约**（事前可改写内容、可中断行为）⑤ **审批与权限档位**（新增「完全访问」）
> ⑥ 分期验收 ⑦ 待拍板决策点。
> 涉及脚本运行时一律指 `ada-ts-engine`（原 `ts_engine`，包目录仍为 `crates/ts-engine`）。

---

## 1. 分析：现状事实

### 1.1 五条链各自的现状

**(1) 系统提示词**：全仓只有一个拼接函数 `compose_system_prompt_full`
（`crates/agent-adapter/src/prompt/coding.rs:137`），输出四段硬编码 + 两段可选：

| 片段 | 来源 | 行号 |
|---|---|---|
| 首行人格 | `identity.name` + `persona`（产品 spec）/ 默认常量 | `coding.rs:58`、`:51`、`:56` |
| 工作区根目录 | 构造参数 | `coding.rs:145` |
| 三条编码指引 + 语言指引 | **硬编码字符串** | `coding.rs:146-149`、`:72` |
| 扩展工具段落 | `HostOptions.prompt_extensions` | `coding.rs:156-162` |
| 技能段落（**正文全量内联**） | `SkillResolver` 回调 | `coding.rs:164-180` |

要点三条：
- **技术栈边界是错的**：提示词排版（属于"产品/人格"层）住在适配器 crate 里，加一段话要改适配器。
- **`prompt_extensions` 没有生产者**（只在 `agent-host/src/lib.rs:76/137` 声明与消费，
  全仓无调用方），所以那一段永远不出现——一个已存在的、空的扩展缝。
- **技能是"扫盘 + 全量内联"**：`prompt.system_prompt()` 每次被调用都重新
  `scan_skills` 一遍磁盘，把启用技能的**正文**塞进提示词（`agent-host/src/lib.rs:286-302`）。

**(2) 工具**：装配是静态白名单。
`TOOLKIT_NAMES = ["core","fs","command","decision","git","project"]`（`crates/agent-toolkit/src/toolkits.rs:20`），
`tools_for_toolkits`（`toolkits.rs:47`）把 `spec.toolkits` 变成 `Vec<Arc<dyn Tool>>`。
`Tool` trait 只有三个方法（`crates/agent-base/src/ports/tools.rs:26-44`：`descriptor` /
`execute` / `long_running`），`ToolContext` 只有四个字段（`ports/tools.rs:19-24`：
`scope` / `cancel` / `events` / `thread_id`）——**没有 workspace 路径、没有插件身份、没有检查点**。
描述符唯一真源是 `registry.rs` 的 24 个标准描述符，实例描述符逐字段对账由
`toolkits.rs:145` 的契约测试守着。

**运行时注册入口存在但无人调用**：`CompositeToolCatalog::register/unregister/reload`
（`crates/agent-runtime/src/catalog.rs:44/58/67`）只被测试调用；`ProductBuilder::build`
内部 `Arc::new(CompositeToolCatalog::new(..))`（`crates/agent-runtime/src/builder.rs:126`）
是本地变量，而 `AgentRuntime.tools` 的类型 `Arc<dyn ToolCatalog>` 上**没有** `register`。
即：装配一次，之后不可变。

**(3) 技能**：`crates/agent-node/src/skills/`（manager/types/builtins）。
目录约定 `<ws>/.ada/skills`、`<ws>/.agents/skills`、`~/.a-da/skills`、`~/.agents/skills`
（`manager.rs:192-202`），id 为 `{scope}:{name}`（`manager.rs:219`），状态写
`~/.a-da/skills_state.json`（`manager.rs:49-51`）。`SKILL.md` 的 frontmatter 是**手写
`:` 切分**（`manager.rs:134-178`），只认 `name` / `description` / `whenToUse` / `userInvocable`，
其余键**静默忽略**（`manager.rs:170`）。技能进入模型的唯一方式是上面的全量内联，
**没有**"索引 + 按需加载"。

技能与插件在数据结构上已经交叉、在运行上完全没通：`PluginItem.skills` 字段存在
（`crates/agent-node/src/plugins/types.rs:92`，实为 `Vec<SkillSummary>`），但生产里恒为
`Vec::new()`（`manager.rs:454`）；`SkillSummary.plugin_name/plugin_id` 预留
（`skills/types.rs:14-17`），`scan_skills` 永不填（`manager.rs:231-232`）。

**(4) 插件**：只有管理面。`PluginManager` 的公开方法全是
`scan_plugins` / `toggle_plugin` / `get_capabilities` / `save_capabilities` / `save_config` /
`read_configs` / `save_secret` / `check_secret` / `create_plugin_template` / `delete_plugin`
（`crates/agent-node/src/plugins/manager.rs`），**没有任何"执行插件"或"取出插件工具"的入口**。

三处"看起来装上了"：
- `scan_plugins` 报出的第三方插件工具清单是**文本扫描伪造**的：读源码前 20 行找注释当描述、
  用 `content.find("name: '")` 找工具名、找不到就用文件名兜底、参数 schema 只有
  `web_search` 是硬编码的真 schema（`manager.rs:355-395`）。每个插件只产出 1 条工具。
- `PluginSandbox`（真实现，`plugins/sandbox.rs`：`inspect_plugin:24` / `call_tool:140`）
  的调用点**只有它自己的 `#[cfg(test)] mod tests`**（`sandbox.rs:350/403/441/512/518/532`）。
- `toggle_plugin` 只改 `config.json` 的 `disabledPlugins`（`manager.rs:81-116`），
  禁用**不跳过**扫描（`manager.rs:339`），更不影响任何工具目录或执行路径。

而 `capabilities.plugins` 在握手时**是真报 true 的**（`agent-rpc/src/server/dispatch.rs:1358`
从产品 spec 派生，`products/ada-coding/agent.spec.json` 里 `plugins: true`）。
也就是说：界面与客户端被告知"支持插件"，而它今天只覆盖管理面（列表/开关/配置/密钥/模板），
**执行面不存在**。这正是 `docs/plugin-sdk/v1.md` §6 立的规矩（"未实现的能力位不得声明为 true"）
在 `plugins` 这一位上的失守——`hooks` / `resync` 那一行守住了（`dispatch.rs:1359-1363` 硬编码 false）。

**(5) 钩子**：~~0 个点位。全仓 26 个点位名在 `crates/` + `products/` 零调用点~~
> **已落地（P3a，2026-10-16）**：端口 `HookRunner`（`agent-base/ports/hooks.rs`）+
> 生产实现 `agent-node::hooks::HookRuntime`；首批 6 个点位接了引擎或宿主，成对埋点
> `hook.entered` / `hook.finished` / `hook.skipped` 全在。`hookTimeoutMs` 与
> `capabilities.hooks` 都成了真消费者（后者还能决定"装不装运行时"）。
> **P3b 起**：`beforeToolCall` 的 `Replace`（改写）也开了，且与重校验、风险单调性同批
> （§5.6/§5.8）。仍未接的点位见 §5.8；`capabilities.{allowPlanModeHooks,
> allowThirdPartyHooks}` **仍未消费**（第三方钩子的分级暂由"没有受信档位"这件事兜住：
> 控制面对第三方一律关闭，见 §5.9）。

### 1.2 埋点与可观测性现状

已有的（可复用）：

- **唯一事件出口**：`EventSink::emit`（`crates/agent-base/src/ports/events.rs:8-11`），
  信封 `AgentEvent { seq, at_ms, thread_id, body }`（`domain/event.rs:70-83`），
  `seq` 全局单调 + 账本校验 + 溢出计数（`agent-rpc/src/server/events.rs:62-79`）。
- **12 个事件变体**（`domain/event.rs:28-48`），但 `SubagentStarted` / `SubagentFinished`
  / `QuestionAsked` 三个**无生产发射者**，被 `verify-wiring` 的豁免表显式挂着
  （`tools/xtask/src/gates.rs:885-899`）。
- **工具事件已成对**（`ToolCallStarted` `runtime.rs:503` / `ToolCallFinished` `runtime.rs:492,549`），
  回执自带起止时间（`ToolReceipt` `domain/tool.rs:107-108`，`duration_ms()` 派生 `:140`）。
- **诊断日志范式**：`[slow]`（`agent-rpc/src/server/dispatch.rs:318-338`，≥200ms 才打，
  Drop 守卫覆盖所有返回路径）、`[sync]`（`dispatch.rs:1189-1196`）、前端的
  `[批准]` 四段耗时（`desktop-ui/src/client/ws-client.ts:1362-1378`）。
- 桌面壳日志链：Electron 主进程统一写 `<app_home>/launcher.log`（`electron/main.ts:124-130`），
  网关 stdout 按行转发（`main.ts:337-340`）——因为 `ada-gateway` 的 tracing 写 stdout
  （`crates/agent-gateway/src/main.rs:47-52`）；而 `ada-coding` 宿主的 tracing 写
  `<app_home>/host.log`（`products/ada-coding/src/main.rs:176-197`），**不经** launcher.log。

缺失的（要补的）：

- **没有 trace/span**：全仓无 `#[instrument]` / `span!` / `trace_id`（`agent-base` 引擎与领域层
  **零 tracing 调用**）。跨"钩子 → 插件 → 提示词 → 技能 → 工具"**没有共同关联键**。
- **没有来源标注**：12 个变体里没有 `plugin_id` / `hook_id` / `skill_id` / `prompt_fragment` 维度。
- **没有扩展维度的事件**：插件加载/编译/激活/调用、钩子进出、提示词组装、技能加载，
  全部没有任何输出。`PluginManager` / `skills` / `checkpoint` 目录 **零 tracing**。
- **`diagnostics` 字段有壳无用**：`PluginDiagnostic` / `LoadedPlugin.diagnostics` /
  `PluginItem.diagnostics` 存在（`plugins/types.rs:47/74/90`），生产构造处一律
  `Vec::new()`（`manager.rs:439/452`、`builtins.rs:148/161`），RPC 侧硬编码 `"diagnostics": []`
  （`dispatch.rs:3081`），界面**零引用**。
- **检查点、取消无可观测输出**：回滚只以 RPC 响应字段回界面（`dispatch.rs:347-369`）；
  取消只体现为 `TurnFinished { stop: Aborted }`，无原因/发起者/时间（`runtime.rs:133-134/398-400`）。
- **审批等待无耗时埋点**（超时常量 `crates/agent-node/src/approval/gate.rs:32` 的 1800s，
  超时分支 `gate.rs:144-153` 无日志）。

### 1.3 缺口总表

| 声明在哪 | 声明的意思 | 实情 | 证据 |
|---|---|---|---|
| `capabilities.plugins: true` | 产品支持插件 | 只有管理面；执行面 0 | `agent.spec.json` / `dispatch.rs:1358` vs `sandbox.rs` 无生产调用 |
| `capabilities.hooks: false` | 钩子未落地 | 真实（0 点位） | `dispatch.rs:1362`、26 点位名零命中 |
| `capabilities.streaming: true` | 支持流式 | **无人读**（符号级检查漏网） | `agent.spec.json`、`gates.rs:430` |
| `allowPlanModeHooks` / `allowThirdPartyHooks` / `hookTimeoutMs` | 钩子能力位 | `hookTimeoutMs` **已消费**（P3a：单点位预算）；另两个仍只解析不消费 | `manager.rs:138-148`、`ports/model.rs:23` |
| `PluginItem.tools` | 插件提供的工具 | 文本扫描伪造，无装配消费者 | `manager.rs:355-395`、唯一消费者 `dispatch.rs:3052` |
| `LoadedPlugin.diagnostics` | 插件诊断 | 无写入方、无界面 | `manager.rs:439`、`dispatch.rs:3081` |
| `SkillSummary.plugin_name/plugin_id` | 插件技能 | ~~恒为空~~ **已接通（SP-45）**：插件声明 → 三来源合流 | `skills/manager.rs`、`scripts/assembly.rs` |
| `prompt_extensions` | 扩展工具段落 | ~~无生产者~~ **已接通（SP-44）**：插件工具清单喂上 | `agent-host/src/lib.rs` |
| `tools.register/unregister/reload` | 运行时注册 | ~~无生产调用者~~ **已接通（SP-46）**：插件启停的受控重装配 | `catalog.rs`、`reassembly.rs` |
| `SubagentStarted/Finished` | 子智能体生命周期 | 无生产发射者（豁免表） | `gates.rs:885-899` |

### 1.4 从现状得出的四条设计约束

1. **先有可观测，才有可验证的扩展点**。AGENTS §15 的教训（"钩子声明了却没人调用是静默失效"）
   在本仓已经发生过两次：钩子 0 点位、插件沙箱不可达。而现有门禁只能做**符号级**检查
   （`gates.rs:430`：能力位只查符号出现过没有），所以 `streaming`、`prompt_extensions`、
   `diagnostics` 这类"接进去但没接行为"的空壳一路绿灯。结论：**埋点必须是第一块地基**，
   并且要让"扩展点没被调用"也能被断言（缺席可见）。
2. **执行面只有一个内核，且必须经端口**。INV-1 要求引擎在 `agent-base`、端口无默认实现；
   依赖方向单向 `agent-rpc → agent-node`。所以脚本运行时只能以**端口**形式被引擎认识，
   实现落在 `agent-node`（`ada-ts-engine` 已经在那里），不能在 `agent-base` 引脚本引擎。
3. **不做第二份单源**。工具描述符仍只出自 `registry.rs`（INV-3）；事件流仍只走
   `EventSink`（事件流单一真源）；不新建旁路的统计/日志设施来"补观测"。
4. **产品声明是能力的唯一口径**：任何能力位只有在**有生产消费者**之后才允许为 true
   （`plugin-sdk/v1.md` §6）。这条要用门禁机械化，不能靠自觉。

---

## 2. 埋点方案（地基）

### 2.1 目标

让下面四个问题在**不读代码、只看输出**的情况下可回答：

- 这一轮里，**哪个来源**（内置工具 / 哪个插件 / 哪个钩子点位 / 哪段提示词）参与了？
- 它**什么时候**开始、多久、结果如何（成功 / 拒绝 / 超时 / 失败）？
- 扩展点**为什么没参与**（未启用 / 加载失败 / 无插件 / 被收窄）？
- 一个声明（能力位、插件工具、钩子点位）是否真的在这个进程里被调用过？

最后一个问题是关键：它是把"声明了却没接线"从**人工审查**变成**可执行断言**的唯一手段。

### 2.2 三条例律

1. **埋点必须与执行同源**：埋点写在唯一的执行点上（引擎的 `emit` 点、宿主的调用点），
   不允许另建一份旁路统计（那必然会漂移）。
2. **埋点必须缺席可见**：扩展点被跳过时必须产出显式的"未发生"证据（`skipped` / `absent`），
   而不是什么都不发。静默 = 回到今天的病。
3. **默认只记元数据**：默认记录 `{origin, point, started_at, duration_ms, status, bytes}`
   等元数据；**内容**（提示词正文、工具参数、脚本 stdout）默认不记，需要时按开关 + 截断 + 脱敏。

### 2.3 关联键：`TraceCtx`

现状只有 `seq` / `at_ms` / `thread_id` / `call_id`，跨层不可关联。新增一个信封级的关联上下文，
**加在 `AgentEvent` 信封上而不是 12 个 body 上**（一次改动覆盖全部变体，且 wire 上是可选字段，
老客户端忽略即可）：

```
TraceCtx {
  thread_id: String,        // 已有
  turn_id: String,          // 新：每轮唯一（引擎在 run_turn 开头生成）
  step_index: u32,          // 新：轮内步号（模型调用计数）
  call_id: Option<String>,  // 已有语义
  origin: Origin,           // 新：谁产生的这件事
}

Origin = Builtin | Plugin { id } | Skill { id } | Hook { point } | Subagent { id } | Host
```

`turn_id` 由引擎生成（`run_turn` 起点，`crates/agent-base/src/engine/runtime.rs:126-130`
附近），随信封下发；`origin` 由**产生方**填写，不由消费方猜。

不引入 OpenTelemetry 等外部依赖：`agent-base` 保持零 IO、不引 tracing，
关联键是**领域类型**（`domain/trace.rs`），导出/采样由 `agent-rpc` 决定。

### 2.4 诊断事件族（点位清单）

在 `AgentEventBody` 里新增一族**诊断事件**（面向机器与诊断面板，不是面向会话流的卡片）。
与 Notice 的既有处置一致：**只给人看、永不进模型上下文、不进会话 items 的正文**。

> **实现口径（P0 落地时定的）**：不是"一个点位一个变体"，而是**一个 `Diagnostic(DiagnosticEvent)`
> 变体 + `point` 字段里的点位名**。理由：点位会随分期增长，若每个点位都是变体，加一个点位要在
> `project_to_loop_event`、dispatch 的 UI 投影、`kind()`、契约测试的穷尽匹配里各补一条分支
> ——加一个点位动五个文件。点位词汇表在 `crates/agent-base/src/domain/diag.rs` 的 `DIAG_POINTS`，
> 且**只登记当天真有发射方的点位**（同 `plugin-sdk/v1.md` §6 的规矩：未实现的不许先声明）。
> 下表因此是**目标清单**，不是"已经全部登记"的清单。

| 族 | 事件 | 关键字段 | 发射点 |
|---|---|---|---|
| 插件生命周期 | `PluginDiscovered` / `PluginLoaded` / `PluginLoadFailed` / `PluginSkipped{reason}` / `PluginToggled{from,to,applied}` / `PluginActivated` / `PluginDisposed` | `plugin_id`、`scope`、`transpile_ms`、`reason`、`bytes` | `agent-node::plugins` |
| 脚本调用 | `ScriptInvoked` / `ScriptFinished` / `ScriptTimeout` / `ScriptBudgetExceeded` | `plugin_id`、`export`、`duration_ms`、`status`、`stdout_bytes` | `agent-node::scripts`（新） |
| 钩子点位 | `HookEntered` / `HookCompleted{outcome}` / `HookDenied{reason}` / `HookTimedOut` / `HookReplaced` | `point`、`plugin_id`、`duration_ms` | 引擎点位（§4.6） |
| 提示词组装 | `PromptComposed` | `segments: [{id, source, bytes, sha8}]`、`total_bytes` | `CodingPromptSource` → 或装配期 |
| 技能 | `SkillLoaded` / `SkillSkipped{reason}` / `SkillInvalid{keys}` | `skill_id`、`scope`、`bytes` | `agent-node::skills` |
| 工具来源 | 复用 `ToolCallStarted/Finished`，**补 `origin`** | `origin`（builtin / plugin:<id>） | 引擎已有 emit 点 |
| 决策 | `DecisionMade` | `certainty`、`calibrated` | decision 工具 |
| 审批 | `ApprovalWaited` | `waited_ms`、`timed_out` | `agent-node::approval` |
| 取消 | `TurnAborted`（或补 `TurnFinished.aborted_by`） | `source`（user / timeout / budget） | 引擎 |

> **P0 已落地的点位**（`DIAG_POINTS` 是"已实现"清单，不是上面这张目标表）：
> `prompt.composed`（引擎，每轮一次 + 字节数）、`approval.waited`（引擎，含等待时长与是谁给的答案）、
> `turn.aborted`（引擎，带中止点）、`plugin.discovered` / `plugin.skipped`（扫描，含
> **缺席信号** `reason=no_plugin`；~~如实信号 `reason=execution_not_wired`~~
> 已在 P1 接线后**删除**——见 dev-plan §P4 的注）、
> `skill.loaded` / `skill.skipped`（扫描，聚合一条 + `reason=no_custom_skill`）。
>
> **P1 / P2 追加**：`script.invoked` / `script.finished`（脚本宿主，一进一出**成对** + 耗时 +
> 控制台**元数据**）、`plugin.replaced_builtin`（装配期，插件顶掉同名内置的**审计**）、
> `approval.skipped`（引擎，"没问就放行"的留痕）、`skill.invalid`（`skill.list`，frontmatter 里
> **认不出的键**逐条报 + 键名清单——以前是静默忽略）。
>
> **P3a 追加（钩子）**：`hook.entered` / `hook.finished`（一进一出**成对**，`data.callId` 是配对键，
> finished 带 `outcome`（passed/denied/timed_out/aborted）与耗时）、`hook.skipped`
> （**这个点位没人订阅**——缺席可见，作者最常问的正是"我的钩子为什么没被调用"）。
> 转换点唯一在 `ports::hooks::observation_to_diagnostic`：轮内（引擎）与轮外（宿主管理面）
> 各写一份的话，迟早出现"引擎报 denied、宿主报 deny"这种对不上的账。
>
> **P3b 追加（改写）**：`hook.replace`（改写**真的生效了**，带 `beforeHash`/`afterHash` 与
> `bytesDelta`，按插件归因——安全性在于"谁改的、改前改后是什么"必须可追溯）、
> `hook.replace_rejected`（被重新校验拒了，理由是结构化标签：`not_an_object` /
> `schema_mismatch` / `path_out_of_scope:<字段>` / `risk_downgrade:*`）。
> 这两条是"插件想做但没做成"的记录，插件作者与排查者都靠它。
>
> 两处刻意的实现取舍：
> 1. **扫描类诊断在 RPC 的用户动作点发**（`plugin.list` / `skill.list`），而不是让节点层自己持 sink
>    ——此刻没有轮次在跑，而节点层要发事件就得处理"同步回调 ↔ 异步 store"的桥接；
>    P1 插件真正参与装配时，发射点随装配迁到节点侧。
> 2. **技能只报聚合一条**，不在"每步取提示词"的路径上逐条报：技能解析器的调用频率是
>    **每步一次**，逐条报会把 60 条环形缓冲刷爆。

**成对性是硬约束**（AGENTS §12 的既有口径）：每个 `HookEntered` 必须恰好对应一个终结事件
（`HookCompleted` / `HookDenied` / `HookTimedOut`）；每个 `ScriptInvoked` 同理。
缺口 = 成对性违规，可直接写契约测试断言——这正是 §2.1 第 4 个问题的机械化。

### 2.5 三条消费路径

1. **界面诊断面板**：现有 `DebugPanel`（`desktop-ui/src/components/DebugPanel.tsx:34-77`）
   的数据源 `AgentStore.log` 上限 60 条、内容是 dispatch 手工 `push_log`。改为结构化诊断条目
   （按 `origin` / `point` 过滤、按 `turn_id` 分组）。这里同时消费一直闲置的
   `LoadedPlugin.diagnostics` 字段（§1.3 的"有壳无用"）。
2. **落盘（可选）**：`<app_home>/diag/<date>.jsonl`，滚动 + 条数上限。**不写进会话 jsonl**
   （会话落盘是模型上下文的一部分，诊断不是）。
3. **门禁与测试**：
   - 新增 `verify-wiring` check K：每个诊断点位**常量**必须在生产代码里被引用
     （= 真有发射方），且 `DIAG_POINTS` 与 `POINT_*` 常量一一对应；**豁免表刻意不存在**。
     ✅ **P0 已落地**（含"红能力"验证：临时加一个无发射方的点位会让门禁变红）。
   - 新增 check L：每个能力位必须有生产消费者，否则必须在信息性字段登记表里写明理由
     （且该位名要在 `unfinished-features.md` 里登记）——直接补上 `streaming` 这类漏网
     （现状 `gates.rs:430` 只查符号）。✅ **P0 已落地**。
   - 契约测试：跑一次"带插件 + 带钩子 + 带技能"的回合，断言
     `PluginLoaded` / `ScriptInvoked` / `HookEntered+终结` / `PromptComposed` 四类埋点齐全；
     再跑一次"无插件"的回合，断言产出的是 `PluginSkipped{reason: no_plugin}` 而**不是空**。
     ⏳ P0 只落了引擎侧的半条（`test_diagnostics_carry_trace_and_stay_out_of_conversation`：
     断言 `prompt.composed` 必发、全部事件共享一个 `turn_id`、诊断不进对话面）；
     带插件的那半要等 P1/P3 的发射方。

### 2.6 落地点清单

| 层 | 改动 | 文件 |
|---|---|---|
| `agent-base` | `TraceCtx` / `Origin` 领域类型 | `domain/trace.rs`（新）、`domain/event.rs`（信封加字段） |
| `agent-base` | 引擎填 `turn_id` / `step_index`；工具事件补 `origin` | `engine/runtime.rs:126,503,549` |
| `agent-node` | 插件/脚本/技能/审批的埋点发射 | `plugins/*`、`scripts/*`（新）、`skills/*`、`approval/*` |
| `agent-rpc` | 诊断投影 + 日志桥（诊断事件 → tracing）+ 截断脱敏 | `server/emitter.rs`、`server/dispatch.rs` |
| `agent-proto` | `client-ts` 的 DTO 同步（可选字段） | `dto.ts` |
| `desktop-ui` | 诊断面板结构化 + 过滤 | `components/DebugPanel.tsx` |
| `xtask` | check K / check L | `tools/xtask/src/gates.rs` |

---

## 3. 脚本系统设计

### 3.1 边界：做什么，不做什么

- **内核只有一个**：`ada-ts-engine`（包目录 `crates/ts-engine`，Boa 0.22 + OXC）继续是唯一的 JS/TS 运行时，
  **不新建第二份引擎**（INV/归档门）。
- **脚本系统 = 端口 + 沙箱 + 生命周期 + 隔离策略**，不是新语言层，也不是新的插件格式。
- **不做通用"执行任意函数"的远程入口**（那会把沙箱变成 RPC 攻击面）。

### 3.2 宿主端口 `ScriptHost`

在 `crates/agent-base/src/ports/` 新增端口（第 11 个），引擎只认识它：

```rust
pub trait ScriptHost {
    /// 装一个模块（转译 + 校验 + 实例化），返回可调用的模块句柄
    fn load(&self, req: LoadRequest, ctx: &HostCtx) -> BoxFuture<Result<ModuleHandle, ScriptError>>;
    /// 调用模块导出的一个入口
    fn invoke(&self, h: &ModuleHandle, call: InvokeRequest, ctx: &HostCtx)
        -> BoxFuture<ScriptOutcome>;
    /// 释放（幂等）
    fn dispose(&self, h: ModuleHandle) -> BoxFuture<()>;
}
```

三条设计约束：

- **端口必须携带预算**（`timeout` / `max_bytes` / 指令预算）与**取消**（`CancelToken`）
  与**埋点 sink**。现状是**三套超时互不相干**：`inspect_plugin` 硬编码 15s
  （`plugins/sandbox.rs:116`）、`call_tool` 由调用方给 `timeout_s`（`sandbox.rs:145`）、
  引擎另有 `RunPolicy::tool_timeout`（`crates/agent-base/src/engine/policy.rs:13`，
  生效于 `runtime.rs:526`）。统一为：**引擎给预算，宿主执行预算，超时只有一处判定**。
- **取消必须贯通**：`call_tool` 现在收不到 `CancelToken`，沙箱内那个 `signal` 是自造
  （`sandbox.rs:481-484`）。端口把引擎的 `ctx.cancel` 传进去，与 `TurnFinished{Aborted}` 一致。
- **端口无默认实现**（仓库硬约束），实现放 `agent-node`（`agent-node::scripts`）。

> **实现口径（SP-10/SP-11/SP-15 落地时定的）**：遥测的出口是**宿主级**的
> （`TsScriptHost::with_diag(&Fn(DiagnosticEvent))`，构造时接一次），**不在** `ScriptCtx` 里。
> 理由：出口是"谁在跑脚本"这个**宿主级事实**；放在每次调用的上下文里只会逼每个调用方
> 去凑一个出口，而它们大多凑不出来——事件信封里的 `seq` 与轮次上下文都在引擎手里
> （本仓已有工具为发一条事件写 `seq = 0` 的先例，那会让"seq 严格递增"的账本记下假违约）。
> 没接出口时退回 `tracing`：宁可只进日志，也不伪造信封。
> 已落地的遥测：`script.invoked` / `script.finished`（带结果与耗时、按插件 id 归因）。
>
> 工具侧的最后一段由 `agent-node::scripts::PluginTool` 接上：描述符**取自注册表**（决策 1）、
> 回执映射**只有一处**（`outcome_to_receipt`）、且**不覆写 `long_running()`**
> （插件工具默认受引擎策略超时约束）。

### 3.3 生命周期状态机

```
discover → manifest 解析 → 校验(id/scope/权限声明) → transpile(可缓存)
        → instantiate → invoke* → dispose
```

两个关键决策与理由：

- **每次调用新建实例（默认无状态）**。理由：`ada-ts-engine` 的 `terminate` 是给事件循环发消息
  （`crates/ts-engine/src/runtime/mod.rs:63`，循环在 `:311` break），**打不断正在同步执行的
  JS**；而把插件实例留在内存里等于给引擎引入隐式全局态（INV-8 的反面）。代价是启动开销，
  用 transpile 缓存 + 可选预热带掉（§3.7）。"每会话常驻实例"列为后续优化项，不作为默认。
- **加载失败不炸装配**。这与 `tools_for_toolkits` 对"未知工具包"的处理**刻意不同**
  （`toolkits.rs:53` 返回 `Err` 让装配期炸掉）：工具包是**本项目自己的声明**，不一致必须炸；
  插件是**第三方输入**，失败安全的做法是跳过 + `PluginLoadFailed` 埋点 + 界面可见。

### 3.4 隔离与资源预算（现状的最大缺口）

现状：**无 CPU / 内存 / 指令数上限**（全仓 grep 无此类限制）；fs 只在每次原生调用时做
路径边界检查（`crates/ts-engine/src/sandbox/mod.rs:83`）；`process.exit` 是纯 JS polyfill
只打日志（`env/process.rs`）。一个 `while(true){}` 能挂住 worker 线程。

**关键可行性事实（本次调研新确认）**：Boa 0.22 **已经提供** `Context::runtime_limits_mut()`
（`boa_engine-0.22.0/src/context/mod.rs:594`），可设 `set_loop_iteration_limit(u64)`
（`src/vm/runtime_limits.rs:47`）、`set_recursion_limit(usize)`（`:94`）、
`set_stack_size_limit(usize)`（`:81`）、`set_backtrace_limit(usize)`（`:68`）。
**`ada-ts-engine` 一处都没用**（全仓 grep 零命中）。也就是说：进程内就能获得
"循环次数 / 递归深度 / 栈"三档硬上限，**不必为了打断死循环就上子进程**。

分三层落地（同一端口，可组合）：

- **L1 时间预算**：宿主侧 `tokio::time::timeout`（已有）+ 预算来自引擎（§3.2）。
- **L2 指令预算（新，进程内）**：`ada-ts-engine` 在建 `Context` 时按预算设
  `loop_iteration_limit` / `recursion_limit` / `stack_size_limit`；超限时 Boa 抛错 →
  映射为 `ScriptBudgetExceeded` → 回执 `ToolStatus::Timeout`（沿用引擎既有超时口径）。
  这一层把"同步死循环"从"挂住线程"变成"一条可纠正的工具回执"。
- **L3 进程隔离（可选严格档）✅ 已落地（P4·SP-70）**：把插件执行放进 worker 子进程
  （`runner.exe`），超时 `kill`（`kill_on_drop`：宿主不再等它的那一刻就被回收）。
  档位用 `--script-sandbox subprocess` 选，默认仍是进程内。
  > **⚠️ 两点与本节原始设计的差别，必须一起读**：
  > ① 原始设计写的是"子进程的 stdin 一律 `Stdio::null()`"——那条针对的是**工具的子进程**
  > （AGENTS 记的坑：继承 stdin 会让等输入的命令卡满超时）。这里相反：worker **靠 stdin
  > 读那一条请求**，协议就是一次请求一次结果，所以它必须拿到 stdin。两者不矛盾，但别记混。
  > ② **本档不做内存限额**：给子进程设 RSS 上限要看平台（Windows Job Object / Linux rlimit），
  > 没做。所以它保证的是"进程死掉这件事被隔离住"，不是"它死之前吃多少内存被限制"。

### 3.5 权限清单（capability manifest）

`ada-ts-engine` 的宿主注入是可裁剪的（`env/mod.rs:17-295`：process/fs/crypto/url/timers/fetch +
`require` 虚拟加载器；异步侧 fs 原语 `runtime/mod.rs:321-416`、fetch 原生 `:419-512`）。
设计：插件在 manifest 里**声明 needs**，宿主按声明**只注入被允许的那部分**——

```
needs: { fs: [read|write], net: [host 白名单], process: [], crypto: [sha256] }
```

未声明即不注入（消失的 API 比"注入后再拦"更难绕过）。默认档位：`fs: read`、`net: none`。
这与"路径沙箱由核心侧工具判定、插件自行读写文件不受保护"的现状描述
（`plugin-sdk/v1.md` §5）相反：**默认收紧**，把放宽变成显式声明。

> 这一份清单同时是「完全访问」档位的落点：**完全访问 = `needs` 取最宽**（见 §6.2），
> 不另造第二套权限语言。

> **落地形态（P4 · SP-71，2026-10-16）**
>
> 声明面是插件**源码首部的一行指令**，不是独立的 manifest 文件：
>
> ```ts
> // @ada-needs fs,net
> export default function (api) { /* ... */ }
> ```
>
> 为什么是注释：能力必须在**执行之前**定下来。若等插件跑起来才读它的声明，就等于"先给能力、
> 再按声明收回"——而"注入后再拦"正是本节的立论要否掉的形态。注释能静态读，
> 于是装载期的沙箱**从一开始**就只有声明的那几项。
>
> 三条口径（写在 `agent-node::scripts::needs`，**唯一解析点**；装载期与装配期都调它）：
> ① **没有指令 → 宿主地板**（`fs` 开、`net` 关）：这个产品里读写工作区就是插件存在的意义，
> 把它默认关掉只会逼每个插件都写声明，而声明一旦变成仪式就没人认真读；
> 真正需要显式声明的是**网络**。② **有指令 → 就是列出的那些**（`// @ada-needs net` 连 `fs`
> 都没有）：读者不必在脑子里做"地板 + 声明"的叠加。③ **认不出的 token 一律不授予**
> 并产诊断（`unknown_need:nett`）——拼错绝不能静默通过。
>
> **与本节原始设计的两处收窄**（都记在 `docs/unfinished-features.md`）：
> `fs` 尚无 read/write 分离（只有开关）；`net` 的白名单与超时未做（SP-72）。

### 3.6 错误与回执映射（唯一转换点）

现状有类型断层：`PluginSandbox::call_tool` 返回 `agent_toolkit::ToolResult`
（`sandbox.rs:293`），引擎要 `agent_base::domain::ToolReceipt`（`ports/tools.rs:30`），
**两者之间没有转换代码**。设计：在 `agent-node::scripts` 里做**唯一**转换点，映射表固定：

| 脚本侧 | 回执 | 说明 |
|---|---|---|
| 正常返回 `string` / `{ok,output,patch,details}` | `Success` | 沿用 `plugin-sdk/v1.md` §4 既有形状 |
| 主动拒绝（`{ok:false, deny:true, reason}`） | `Denied` + 理由作为**普通工具结果**回模型 | 沿用 AGENTS 对"拦下即定稿、理由可纠正"的既定口径 |
| 抛异常 | `Error`（output 含插件 id 与截断后的错误） | |
| 超时 / 预算超限 | `Timeout`（文案说清是哪个插件、让模型换路子） | 与引擎既有超时回执口径一致 |
| 未导出 / 名字不匹配 | `Error`（装配期就该发现，靠 `inspect` 校验） | |

### 3.7 缓存与性能

- **transpile 缓存**：key = `sha256(源) + ada-ts-engine 版本`，落 `<app_home>/cache/plugins/<hash>.js`。
  现状是每次调用都重转译（`sandbox.rs:14-21` 每次 `transpile_ts_module`）。
- **预热**：插件列表确定后后台预热（不阻塞首轮）。
- 埋点记录 `transpile_ms`（缓存命中 vs 未命中分别标记），让"插件拖慢首轮"可量化。

### 3.8 协议面

- 新增 RPC 只加**自检类**：`plugin.validate`（转译 + inspect + 权限声明校验，返回诊断）、
  `plugin.reload`、`plugin.diagnostics`（读 §2 的诊断缓冲）。**不加**通用执行入口。
- 方法登记进 `spec/proto/ada-coding.ext.json`（L2），**不碰 base**（base ∪ ext 不得有交集，
  由 `verify-wiring` check 与 `compat` 门禁守着）。
- 能力位语义修正：`capabilities.plugins` 的文档语义扩写为"管理面 + 执行面"；
  `capabilities.hooks` 保持 false，直到 §4.6 的翻位条件满足。

### 3.9 ada-ts-engine 自身要满足要求还需完善什么

> 改名：库名已从 `ts_engine` 改为 **`ada-ts-engine`**（包目录仍为 `crates/ts-engine`，
> 库 crate 名 `ada_ts_engine`，CLI 二进制名暂为 `ts_engine`）。下表是**引擎自身**的缺口，
> 与 §3.2-3.8 的"宿主侧设计"分开看：宿主不补，引擎能力就浪费；引擎不补，宿主只能绕。

| # | 缺口 | 现状证据 | 设计为什么需要它 | 建议做法 | 期次 |
|---|---|---|---|---|---|
| A1 | **无指令/循环/递归/栈上限** | 全仓 grep 无任何资源上限；Boa 0.22 已提供 `RuntimeLimits`（`context/mod.rs:594`、`vm/runtime_limits.rs:47/81/94`）却一处未用 | §3.4 L2 的全部依据；没有它，`while(true){}` 挂住线程 | 按预算建 `Context`：`set_loop_iteration_limit` / `set_recursion_limit` / `set_stack_size_limit`；超限错误映射 `ScriptBudgetExceeded` | ✅ P1 已完成 |
| A2 | **`terminate` 打不断同步 JS** | `runtime/mod.rs:63` 只是往事件循环发消息，循环 `:311` break，消息要排队 | §3.2 取消贯通的硬前提 | A1 落地后"同步段必然返回"，`terminate` 退化为兜底；不再依赖它 | ✅ P1 已完成 |
| A3 | **无内存上限** | Boa 无堆上限 API；宿主 tokio timeout 也拦不住同步段内的内存暴涨 | §3.4 L3 的存在理由 | 子进程档 + 进程级内存限额（`kill` 兜底）；进程内档明确记录"无内存护栏"这一事实 | ◐ P4：**子进程档已落地**（超时即杀、崩溃隔离），**进程级内存限额未做**；进程内档"无内存护栏"已写进 `docs/unfinished-features.md` |
| A4 | **超时只在 await 点生效** | `tokio::time::timeout` 包的是异步等待，同步死循环期间不归还控制权 | 与 A1 配套：两条腿都要有 | A1 负责同步段，宿主 timeout 负责等待段，二者边界写进文档 | P1 |
| B5 | **无模块解析** | `plugin-sdk/v1.md:121` 明说"没有——单文件自包含"；`require` 是虚拟加载器（`env/mod.rs:230-291`） | 插件稍微长大就必须拆文件；现在只能整份塞进一个文件 | 支持"插件目录内 + 相对路径 + 只读"的解析；越界解析给结构化错误 | P1（最小）→ P2（完整） |
| B6 | **无 source map / 行号映射** | 转译后异常行号与源码不对应 | 诊断质量：插件报错要指得回源码某一行 | oxc 生成 sourcemap 或至少保留行偏移表，随 `ScriptFailed` 一起回传 | P2 |
| C7 | **环境是全量注入** | `env/mod.rs:17-295` 一次性注入 process/fs/crypto/url/timers/fetch + `require` | §3.5 权限清单要按 `needs` 裁剪 | 把注入拆成可勾选的模块集合，未声明即不注入 | ✅ P1 已完成（最小版：网络默认关 + 原生桥同裁；fs 读写分离/完整清单见 P4·SP-71） |
| C8 | **网络参数硬编码** | fetch 原生实现 30s 超时（`runtime/mod.rs:454`），无域名/方法限制 | 插件联网是不可控外部副作用，必须可配可限 | 超时与白名单由宿主传入；默认 `net: none` | ✅ P4·SP-72 已完成（`NetPolicy`：超时 + 域名白名单，**策略先于请求**；来源是运维环境变量，配置文件/界面未做） |
| C9 | **`process.exit` 语义与 Node 不一致** | `env/process.rs` 的 `exit` 只打日志，不真退出 | 插件作者会按 Node 语义写代码，静默不一致会产生"看起来成功了"的假象 | 要么真终止当前调用并回 `Error`，要么把语义写进 SDK 契约 | P2 |
| C10 | **无 stdout/stderr 捕获与限长** | `api.trace` 直接 `console.log`（`sandbox.rs:67-68`）进宿主日志，无插件归属、无截断 | §2.4 的 `ScriptFinished.stdout_bytes` 与诊断面板都要它 | 结构化捕获 + 截断 + 带 `plugin_id` 归属；超限截断并产诊断事件 | P1 |
| D11 | **异步桥语义未约定** | `register_async_fn` / `call_async_fn`（`runtime/mod.rs:555/565`）是宿主↔JS 唯一桥梁，但错误/超时/取消没有统一约定 | §3.2 端口把预算与取消交进来之后，桥必须有一致的异常与取消语义 | 约定：桥负责把宿主错误转成 JS 异常、把预算超限转成中止、取消经 `EventLoopMsg` 传达 | P1 |
| D12 | **事件循环只支持整机 Terminate** | `EventLoopMsg::Terminate` 语义是关掉整个 runtime | 需要"中止当前这一次调用"而不销毁运行时（否则每次超时都要重建） | 增加 `AbortCurrent` 消息；`wait_idle` 与宿主预算对齐 | P2 |
| E13 | **无转译缓存** | 每次调用都 `transpile_ts_module`（`sandbox.rs:14-21`） | §3.7 性能 | 内容哈希 → 产物缓存，落 `<app_home>/cache/plugins/` | ✅ P1 已完成（内容寻址 + 有界 64 + 失败不缓存；**进程内**而非落盘：跨进程复用等 P4 的子进程档） |
| E14 | **转译错误不可结构化** | `sandbox.rs:18-19` 把错误包成一段字符串 | 诊断要能指向行列，界面要能高亮 | 错误结构带 `line/col/message`，经同一条诊断通道上抛 | P2 |
| F15 | **没有独立的插件执行契约测试** | 沙箱用例写在 `sandbox.rs` 自己的 `mod tests` 里 | 契约测试要能在引擎之外跑（`agent-conformance` 已有端口契约测试范式） | 在 `agent-conformance` 加 `ScriptHost` 契约测试（含超时/取消/预算三条） | P1 |
| F16 | **引擎版本与 SDK 版本无协商** | `plugin-sdk/v1.md` §6 只说"v1 随归档生效" | 插件声明 `needs` 的 API 面时，得有个版本对账点 | 引擎暴露能力清单（可用模块/版本/上限），装配期对账并在不满足时诊断 | P2 |
| F17 | **无自检入口** | 没有任何"校验这个插件"的路径 | §3.8 的 `plugin.validate` | 转译 + inspect + 权限声明 + 预算校验，返回结构化诊断 | P1 |

**分档结论**：P1 的最小集是 `A1 / A2 / A4 / B5(最小) / C7 / C10 / D11 / E13 / F15 / F17`——
没有这些，"插件工具被模型调用一次并拿到成功回执"这件事根本跑不起来（或跑起来就要挂线程）。
P4 的 `A3 / C8` 是"敢让不可信第三方插件进生产"的门槛。

> **P1 实况（SP-10 / SP-12 落地后）**：`A1` / `A2` 已完成——限额接上之后，
> `while(true){}` 在**进程内**被打断（脚本宿主 3.4 秒返回预算超限，此前是等到墙钟超时）。
>
> 🔴 **接通这条路径时实地踩到两个真 bug**，都记在这里以免重新引入（它们的共同特征是
> "只有真的执行一遍才会暴露"——正是本文开头判定"插件执行面从未上线"的佐证）：
> 1. **行级转译器给未完成的表达式补分号**：`transpile_ts_module` 按行做文本替换，
>    而 oxc 的 codegen 会把对象的第一个属性留在 `export default {` 同一行 →
>    产出 `{ tools: [{;` → 语法错误。**只在第一个属性是数组/对象时复现**。
>    已改为按括号平衡判断（`bracket_delta`，跳过字符串与注释），并有回归用例。
> 2. **`CallAsyncFunction` 吞掉 eval 错误**：`let _ = ctx.eval(...)` 让撞限额的那次调用
>    Promise 永不 resolve，调用方白等到墙钟超时（实测 120 秒），真因不可见。已改为立刻回执。

---

## 4. 统一贡献模型：系统提示词 / 技能 / 工具 都是插件贡献

### 4.1 抽象：`Contribution`

四类贡献（第五类可选），每类都带同一套元数据：

```
Contribution {
  id: String,              // "tool:read_file" / "prompt:guidelines" / "skill:tdd" / "hook:beforeToolCall"
  kind: Tool | PromptFragment | Skill | Hook,      // (+ Decision 可选)
  origin: Origin,          // Builtin | Plugin{id}
  order: i32,              // 稳定排序（内置优先，插件按声明）
  budget: Option<Budget>,  // timeout / max_bytes
  failure: FailSafe | FailFast,   // 插件默认 FailSafe，内置默认 FailFast
}
```

**逐条对应今天的三个真源**（这是本设计的核心：把三条各写一份的装配合成一条）：

| 今天 | 位置 | 变成 |
|---|---|---|
| 工具声明 | `registry.rs` 24 个描述符 + `toolkits.rs` 白名单 | `Tool` 贡献（描述符仍只出自 registry，INV-3） |
| 提示词 | `compose_system_prompt_full` 硬编码段落 | `PromptFragment` 贡献（有序片段） |
| 技能 | `SkillManager::scan_skills` + 适配器渲染 | `Skill` 贡献（索引 + 按需正文） |
| 钩子 | 不存在 | `Hook` 贡献（点位 + 处理函数） |

### 4.2 装配管线（一个函数，一次装配）

```
contributions = collect_builtin() ∪ collect_plugins()
             → validate(重名 / 冲突 / 权限 / 预算)
             → order(内置优先，插件按 order，稳定 tie-break)
             → { ToolCatalog, PromptPlan, SkillIndex, HookTable }
```

- **运行期只读**，与今天一致（引擎依赖 `Arc<dyn ToolCatalog>` 不可变）；
  若要热插拔，走既有的 `CompositeToolCatalog::register`（§1.1），但那是**装配期之后的显式操作**，
  不是默认路径。
- 装配产物一次性写入埋点（`PromptComposed` / `PluginLoaded` / `SkillLoaded`），
  于是"这轮到底装了什么"有据可查。

### 4.3 提示词即贡献

- 把 `compose_system_prompt_full` 的段落拆成**有序片段**（id / source / 预算 / 内容）：
  `persona`、`workspace`、`guidelines`、`language`、`extensions`、`skills`、未来的 `plugin:*`。
- **收益**：① 插件加提示词不用改适配器；② 埋点能报"这段是谁加的、占多少字节"
  （与现有 `stats.promptChars` 方法天然合流）；③ `prompt_extensions` 那个空缝自然被填上。
- **兼容约束（硬）**：现有测试要求"身份未声明时输出与搬运前逐字节一致"
  （`coding.rs:126-127` 注释与相关断言）。装配管线必须能产出**与今天完全相同**的字符串——
  以 `compose_system_prompt_full` 的现有输出为黄金样本写回归测试钉住。
- **技能从"全量内联"改"索引 + 按需"** 是一个**行为变更**（会改变模型的可见信息），
  建议先用 §2 的埋点量出"技能正文占提示词的 token/字节比"，再决定是否改（见 §9 决策点 4）。

### 4.4 技能即贡献

- 来源扩为三处：**工作区/全局目录**（现状）、**插件声明的技能**（新，把
  `SkillSummary.plugin_name/plugin_id` 接通）、**内置**（现状 `builtin:`）。
- `SKILL.md` frontmatter：现状手写切分 + 未知键静默忽略（`manager.rs:170`）。设计改为
  **已知键白名单 + 未知键产 `SkillInvalid{keys}` 诊断**（不静默），键集合保持现有四个
  以维持向后兼容。
- 技能索引进提示词、正文按需注入（若决定采纳 §4.3 的变更）；技能加载产出
  `SkillLoaded` / `SkillSkipped{reason}` 埋点。

> **落地口径（SP-45，2026-10-16）**
>
> - **同名顺序：目录 → 插件 → 内置**（先到先得）。与工具那条"插件胜出"**刻意不同**：
>   工具名是注册表先占的、插件只能"重实现"；技能同名的语义是"这份规范谁更权威"，
>   用户显式写下的那份最权威，插件再胜过通用兜底。
> - **插件技能的声明面是 `api.registerSkill`**（与 `registerPrompt` 同形）与对象形态
>   `skills: [...]`；收下之后**空名字/空正文的声明被丢弃**（留着只会占位置不说话）。
> - **归因**：`plugin_id` / `plugin_name` 被填上（此前恒空），提示词那条技能片段在
>   `prompt.composed` 里标 `plugin:<id>`，界面的技能库显示插件名与 `plugin` scope。
> - **决策 4 的处置：暂不索引化**，保持正文全量内联。索引化会改变模型可见信息（行为变更），
>   而量化它所需的埋点（§4.3 的逐片段字节报表）此刻才刚可用——先收集数据再决定。
>   回退点：`PromptPlan` 的片段边界已经就位，将来把"正文片段"换成"索引片段 + 按需注入"
>   是一次局部改动，不需要再动提示词以外的任何东西。

### 4.5 工具即贡献

要让插件工具被引擎执行，**缺的连接**（逐条，全部来自调研）：

1. **缺 `Tool` 适配器**：全仓没有任何 `impl Tool` 桥接 `PluginSandbox::call_tool`。
   `crates/agent-toolkit/src/plugin_tools.rs:260` 的 `PluginTool` 名字像插件工具，
   实际只 match 内置 `Kind`（git/project/tests 的本地 async fn），与脚本沙箱无关。
2. **缺装配点**：`scan_plugins` 的产物只到 UI（唯一消费者 `dispatch.rs:3052`），
   装配层（`agent-host/src/lib.rs:324`、`builder.rs:94`）从不读 `PluginItem.tools`。
3. ~~**缺运行时注册**：`catalog.register` 无生产调用者（§1.1）。~~
   **已补（SP-46）**：插件启停走 `LiveContributions::apply` → `register` / `unregister`，
   生产调用者是产品的组合根（`plugin.setEnabled` → `EngineInjection.reassembler`）。
4. **缺上下文**：`ToolContext` 没有 workspace 路径、没有插件身份（`ports/tools.rs:19-24`）；
   而沙箱需要 `plugin_path` + `workspace`（`sandbox.rs:140-146`）。

**默认保守（安全默认值）**，与 AGENTS §2 的失败安全一致：

| 描述符字段 | 插件工具的默认 | 理由 |
|---|---|---|
| `access` | 未知 → 视为写（`is_readonly()` 由 access 派生） | 现状已是如此（`plugins/mod.rs:20-24`：查不到描述符 → `is_write = true`） |
| `approval` | `Always`（或 `DangerScan`） | 第三方代码默认要人点头 |
| `rollback` | `None` | 插件自己负责（引擎无检查点调用点，§1.2） |
| `termination` | `ContinueTurn` | 不给插件结束整轮的能力 |
| `long_running` | `false` | 于是**受引擎超时约束**（不能拿"我是长任务"绕过） |

**子智能体白名单**：`filter_subagent_tools`（`crates/agent-node/src/subagents/runner.rs:61-98`）
默认**排除第三方插件工具**（插件不得借白名单绕过只读约束）；要放行必须显式声明。
这守的是"工具集只能收窄"（AGENTS §12）在插件维度上的成立。

### 4.6 钩子即贡献（与 §2 埋点闭环、§5 点位契约）

- **每个点位都是 before / after 一对**，且**事前可改写内容、可中断行为**；完整的载荷、
  能力、实施位置与安全序见 **§5**。
- **首批 3 个点位**（与既有设计文档口径一致）：`beforeToolCall` / `afterToolCall` /
  `beforeApproval`。实施位置就是引擎里已有的可插入点：`runtime.rs:410`（拿到 tool 实例后）、
  `:437-441`（审批判定前）、`:549-553`（工具事件收尾后）。
- **成对与缺席**：每个点位 emit `HookEntered` + 恰好一个终结事件（§2.4）；
  无插件时 emit `HookSkipped{reason:no_plugin}`，**不留空**。
- **只收窄不放宽**：钩子可以拦下工具调用、替换清单、追加旁注，**不能**改动描述符字段
  （`access` / `approval` 只能更严），也不能替用户放行审批（§5.6）。
- **翻位条件（写进设计，防再次"声明先行"）**：`capabilities.hooks` 只有在
  ① 至少 1 个点位有生产调用、② 埋点成对齐全、③ 契约测试断言"缺席可见"三条同时满足后才允许
  从 false 改 true；同时把 `hookTimeoutMs` 从"只解析"变成真消费者。

### 4.7 冲突与收窄规则

- 重名工具：**拒绝加载该插件**（不是覆盖）。这与"注册表即真源、名字唯一"一致，
  也是 `tools_for_toolkits` 对重名的既有态度（`toolkits.rs:62-68`）。
- 提示词片段：内置优先，插件按 `order` 稳定排序；插件**不能**替换内置片段（只能追加）。
- 插件不能移除内置贡献；能做的只有"更严"（拒绝、收窄、旁注）。

---

## 5. 点位设计：事前 / 事后契约

> 本节回应两条硬要求：① 点位必须**成对**（事前 / 事后）；② **事前必须能改写内容、能中断行为**。

### 5.1 四条原则

1. **成对是硬约束**：每个点位都是 `before` / `after` 一对。理由是"有状态延续的东西必须成对"
   ——事前的改写会改变后续状态，事后必须有一次收尾把"实际生效的是什么"反馈回来，
   否则插件作者无法知道自己的改写是否被采纳、被谁覆盖。
2. **事前只有两种能力：改（改写内容）、拦（中断行为）**；**事后只有一种能力：观察 + 追加**
   ——事后不得改写"已经发生的事实"（已渲染的正文、已落盘的回执状态）。
3. **只收窄、不放宽**（沿用 AGENTS §12 的既有口径）：事前可以更严（拒绝、缩小工具集、
   补上验收项），不能更松（不得把写工具变只读、不得替用户批准、不得降低风险等级）。
4. **控制面点位分级开放**（§5.9）：能"锁死系统"的拦下权（插件启停、会话创建/删除）不给第三方。

### 5.2 事前的两个动作（规范化，不给可变引用）

插件跑在沙箱里、跨 JSON 边界，所以"改写"不能是共享内存里的可变引用，必须是一份
**结构化结果**，由核心侧在**唯一一处**应用：

```
BeforeOutcome<T> =
  | Pass                    // 无意见，继续（默认，等价于没装插件）
  | Replace(T)              // 改写内容：必须是合法的完整载荷或补丁，核心会重新校验
  | Deny   { reason }       // 拦下这一次行为：理由作为**普通工具结果**回模型（可纠正）
  | AbortTurn { reason }    // 结束整轮（比 Deny 强，需要显式声明能力）
```

- `Replace` 之后核心**必须重新校验**（schema / 路径沙箱 / 风险重判 / 重跑审批），
  不是"信插件"。**改写不是授权**。
- `Deny` 的语义沿用既有约定：拦下即定稿，**后面的插件不再被问**；理由走普通工具结果，
  不写成"执行失败无从纠正"。
- `AbortTurn` 比 `Deny` 强（结束整轮而不是这一次），只给少数点位开，且必须显式声明。

### 5.3 事后的一个动作

```
AfterOutcome =
  | Note { text }              // 往工具结果/回执**追加一句旁注**（复用模型已在读的那段文本）
  | Diagnostic { kind, data }  // 只进诊断面，不进模型上下文
```

不许做的三件事：改写已渲染的助手正文；改写工具回执的 `status`；把内容塞进模型上下文之外的
"影子通道"（`Note` 进的是工具结果，模型**能**看到——这点必须在 SDK 里写明，避免插件作者
误以为旁注是"只给人看的"）。

### 5.4 点位清单（配对 / 事前能力 / 事后能力 / 实施位置）

能力列缩写：`R` = 改写内容，`D` = 拦下（Deny），`A` = 结束整轮（AbortTurn），`N` = 追加旁注。

| 点位对 | 事前的载荷（可改写什么） | 事前能力 | 事后能做什么 | 实施位置（现有代码） |
|---|---|---|---|---|
| `beforeToolCall` / `afterToolCall` | `ToolCall { name, args }`（**args 原文**） | R / D / A | N：给工具结果追加旁注 | 引擎 `runtime.rs:410` 之后、`:434` 之前（改写后重判审批）；事后在 `:547` 之后、`:555` 之前 |
| `beforeApproval` / `afterApproval` | `ApprovalRequest { tool, args, 风险说明 }` | D（**只能更严**：可强制升级为"要问"，不可替用户放行） | N：记录用户决定与等待时长 | `runtime.rs:437-441` 之前；事后在 `:464` 之后 |
| `beforeLlmRequest` / `afterLlmResponse` | `CompletionRequest { messages, tools, 预算 }` | R（插入系统片段 / 裁剪消息 / **只收窄**工具表）/ D（放弃这次请求→结束本轮） | N：可建议压缩；**不可**改写已收到的响应正文 | 请求组装 `runtime.rs:229-249` 之前；事后在 `:383`（`UsageReported`）附近 |
| `beforeSystemPrompt` / `afterSystemPrompt` | 提示词片段集合（§4.3 的 `PromptPlan`） | R（追加片段 / 移除**自己**的片段）/ — | N：报告片段字节数与来源 | `runtime.rs:157` 调用提示词端口前后 |
| `beforeCompaction` / `afterCompaction` | 待压缩消息集合 + 保护名单 | R（保护某些消息不被裁）/ D（放弃本次压缩） | N：压缩前后字节、保留条数 | 压缩目前无引擎调用点（§1.2 缺口） |
| `beforeTodoUpdate` / `afterTodoUpdate` ✅ | 清单（`todos` 全量替换） | R / D | N：变更 diff（新增/删除/改标题/改状态，**完成又被改回未完成**单列） | todo 工具内部（P3b 已落地）：`ToolContext.hooks` 把运行时交给工具；`diffTodos` 是纯函数 |
| `beforePersist` / `afterCheckpoint` | 即将落盘的消息/通知 | R（**唯一**允许改落盘内容的点，且必须留审计） | N：落盘字节与路径 | `runtime.rs:143`（用户输入）、`:349-380`（助手消息） |
| `beforeSubagentStart` / `afterSubagentEnd` | 子智能体 `profile` + 任务描述 | R（**只能收窄**工具集）/ D（不派活） | N：子智能体结果摘要 | `agent-node` 委派入口（`subagents/runner.rs`） |
| `beforeSkillLoad` / `afterSkillLoad` | 技能 id + 正文 | D（禁用该技能）；**不开放 R**（技能正文是权威指引，允许改写等于允许插件伪造规范） | N：实际注入的字节数 | 技能解析（`agent-node::skills`）与提示词装配之间 |
| `beforeThreadCreate` / `afterThreadCreate`（**创建会话**） | `ThreadCreateRequest { title, mode, agent_id, workspace, is_subagent, delegation_depth, parent_id, subagent_id }`——即 `dispatch.rs:1544-1565` 的全部入参 | R（**只允许改 `title` / `mode`，以及追加初始片段**；`workspace` 与委派结构 `is_subagent` / `parent_id` / `delegation_depth` **不可改**）/ D | N：实际 `thread_id`、落盘位置、初始上下文来源（模式 / 工作区 / 是否子智能体） | 事前在 `dispatch.rs:1543` 臂内、`create_thread_with_details`（`:1573`）**之前**；事后在 `SessionManager::create_session`（`crates/agent-node/src/session/manager.rs:167`）**落盘成功之后** |
| `beforeThreadDelete` / `afterThreadDelete`、`onThreadSwitch` | 线程元信息 | D（拦下删除） | N：观察 | `agent-rpc` 的 session 方法臂 |
| `beforePluginToggle` / `afterPluginToggle`（**启用 / 停用**） | `PluginToggleRequest { plugin_id, scope, from, to, contributions 摘要 }` | R（**只允许收紧**：把"启用"改成"停用"，或把"启用全部贡献"收窄为白名单）/ D | N：**实际生效的贡献集合**（工具 / 提示词片段 / 技能 / 钩子点位各多少，以及与切换前的差异），外加加载失败与重名被忽略 | 事前在 `dispatch.rs:3111` 臂内、`PluginManager::toggle_plugin`（`crates/agent-node/src/plugins/manager.rs:81`）之前；事后在落盘与装配刷新之后 |
| `beforeTurn` / `afterTurn` | `TurnRequest { 消息, images, 模式, 预算 }` | R（**只允许追加**额外指令，不允许改写用户原文）/ D（拒绝本轮） | N：整轮耗时、步数、用量 | `run_turn` 起点 `runtime.rs:126`；收尾 `:574` |

> **两个新点位各自带一条硬约束**：
>
> ① **原子性（会话创建）**：事前必须在**任何状态变更之前**判定——`Deny` 之后不得留下"内存里建了、
> 磁盘没建"的半成品；`afterThreadCreate` 只在**落盘成功之后**触发，否则"创建成功"是假的
> （和 §2.2 第 2 条律同源：宁缺勿假）。
>
> ② **回执依赖装配管线（插件启停）**：~~现在启停只改 `config.json` 的 `disabledPlugins`，
> **不影响任何工具目录或执行路径**（§1.1）~~。**已消除（SP-46，2026-10-16）**：
> `plugin.setEnabled` 现在会驱动一次**受控重装配**（`agent_runtime::reassembly` +
> `agent-host::LivePluginContributions` / `LivePluginReassembler`），并回一份 `ReassemblyReport`
> ——正是本行要的那份"实际生效的贡献集合与差异"。事前点位（`beforePluginToggle` 的
> `R`/`D`）仍属 P3a，届时把这次收据接到 `afterPluginToggle` 上即可；
> `CompositeToolCatalog::register/unregister` 也因此第一次有了生产调用者（§4.5 的那条缺口）。

**能力声明（manifest 侧）**：插件必须显式声明它要用哪些点位与动作，未声明即不生效并产诊断：

```
hooks: {
  beforeToolCall: ["deny", "replace"],   // 声明才允许改写
  beforeApproval: ["deny"],
  afterToolCall:  ["note"],
  beforePluginToggle: ["observe"]        // 控制面点位：第三方只能观察/旁注（§5.9）
}
```

> **落地形态（P3a，2026-10-16；入口统一于 2026-10-17）**：实际用的是插件侧的
> `api.on(point, handler, { actions, order })`（对象形态 `export default { hooks: [...] }` 同效）
> ——上面那个 manifest 块是**声明形状的示意**，
> 本仓尚未引入独立的 manifest 文件（`LoadedPlugin.manifest` 只描述插件本身）。
> 语义一样：**声明即授权**（未列出的动作运行期不生效并留痕），越级在装配期被拒并产诊断。
> `api.registerHook` 已删除：它与 `api.on` 等价但多一个入口，且对象形态的 `handlerExport`
> 在执行期找不到函数（只认 `handler`），属于"声明了却不跑"的假入口。

### 5.5 执行语义：顺序、链式、定稿、超时

- **顺序**：内置优先，插件按 `order` 稳定排序（与 §4.7 的装配顺序一致）。
- **链式（改写折叠）**：前一个插件的 `Replace` 结果就是后一个插件的输入；这样"多个插件共同
  完善一份清单"能自然成立。
- **定稿**：任一插件返回 `Deny` / `AbortTurn` 即定稿，**后续插件不再被问**（沿用既有约定）。
- **超时**：单点位预算来自 `hookTimeoutMs`（P3a 起**已是真消费者**，默认 500ms）；
  超时一律**视为 `Pass`（无意见）** + 一条 `hook.finished{outcome=timed_out}` 诊断。
  **绝不阻塞整轮**——这是失败安全的选择：插件是外部代码，它卡住不该让用户等。
- **应用一次**：一次调用只应用一次改写，且只在核心侧那一处应用；改写前后的 `sha8` 与字节差
  记进埋点（`HookReplaced`），于是"谁改了、改了什么"可追溯。

### 5.6 安全序（本节最关键）

**审批与路径校验必须发生在 `beforeToolCall` 的改写之后**，否则插件可以在审批通过后改写参数，
把闸门架空：

```
解析 tool_call
  → beforeToolCall（可改写 / 可拦下）        ← 插在这里：runtime.rs:410 之后
  → 重新校验：schema / Scope::resolve_path / 风险重判
  → needs_approval(改写后的 args)            ← 现有 :437-441 保持不变，但读的是改写后的 args
  → 用户决定 → 执行（改写后的 args）
```

配套三条硬规则：

1. **风险单调性**：改写不得降低风险等级。插件不能把危险命令改成无害命令来"骗过审批"，
   再指望事后把原始命令执行掉——因为**执行的始终是改写后的 args**，事后点位也没有发起副作用
   的能力（§5.3 的 `Note` / `Diagnostic` 只写文本）。
2. **只读档位不可绕过**：描述符（`access` / `approval` / `termination`）**不属于可改写载荷**；
   计划模式与只读子智能体拿到的工具集仍是收窄后的。
3. **落盘可改必须留痕**：只有 `beforePersist` 允许改落盘内容，且改写必须产一条审计埋点
   （改了什么、改前改后的 `sha8`）。这是隐私脱敏的正当用途，也是唯一正当用途。
4. **与「完全访问」档位的关系**：完全访问放开的是**边界**（工作区外、网络），**不放开顺序**
   ——改写后重校验、风险单调性、钩子只收窄这三条在任何档位下都成立；改写出的越界路径在 `full`
   档下是**允许**的（那正是它的语义），但必须逐次留痕（§6.3）。

### 5.7 埋点闭环（接 §2.4）

- 每次事前调用 → 恰好一条 `HookEntered` + 恰好一条终结事件：
  `HookCompleted{outcome: pass|replace}` / `HookDenied{reason}` / `HookAborted{reason}` /
  `HookTimedOut{limit_ms}`。**缺口即成对性违规**，可直接写契约测试断言。
- `HookReplaced` 额外带 `before_sha8` / `after_sha8` / `bytes_delta`。
- 无插件/未声明该点位 → `HookSkipped{reason}`（**不留空**，§2.2 第 2 条律）。
- 样本切片（默认关）：需要排查时按开关记录改写前后**内容**，并截断 + 脱敏。

### 5.8 分批：先"能拦"，再"能改"

`Replace` 牵扯审批重跑与路径重校验，是安全敏感变更，所以拆两批：

- **P3a**：首批 3 个点位先只支持 `D`（拦下）与 `N`（旁注），配齐成对埋点。
  **管理面点位（`beforePluginToggle` / `beforeThreadCreate`）与 §5.9 的订阅分级必须同批交付**——
  分级不落地就等于把"锁死系统"的开关交给第三方，这两件事不许分两次做。
  > **已交付（2026-10-16）**：`beforeToolCall` / `afterToolCall`（引擎）+ `beforePluginToggle` /
  > `afterPluginToggle` / `beforeThreadCreate` / `afterThreadCreate`（宿主）共 6 个点位；
  > 分级在 `allows(level, trust, action)` 一处判、越级在装配期被拒并产诊断。
- **P3b**：再开 `beforeToolCall` / `beforeTodoUpdate` 的 `R`（改写），并**同时**落地
  §5.6 的重校验与风险单调性测试（用例：改写后审批必须重新触发、路径越界必须被拦）。
  在此之前插件声明 `replace` 会在**装配期被拒**（理由 `replace_not_implemented`），
  绝不"收下但不用"。
  > **已交付（2026-10-16）**：`beforeToolCall` 的 `R` 开了，重校验
  > （`engine::hook_guard`：形状 / schema / 路径沙箱）与**风险单调性**同批落地，
  > 三条 DoD 用例全绿；载荷契约是 `{ action: 'replace', args: {...} }`（**只有 args**，
  > 描述符字段改不动）。`beforeTodoUpdate` 未接（声明它会被如实拒）。

### 5.9 点位的订阅分级（谁有资格拦下什么）

新加的两个点位暴露了一个结构性问题：**控制面的拦下权等于"谁能锁死系统"**。

- `beforePluginToggle` 若对第三方开放 `Deny`：插件可以**拒绝自己被停用**（自我加冕），
  或**阻止别的插件启用**（互斥锁定）。
- `beforeThreadCreate` 若对第三方开放 `Deny`：插件可以**拒绝用户新建会话**（拒绝服务）。

所以点位要按**订阅级别**分级，不能一视同仁：

| 级别 | 谁能产生效果 | 点位 |
|---|---|---|
| `data-plane`（行为面） | 内置 + 受信 + 第三方；第三方可 `R` / `D` | `beforeToolCall` / `afterToolCall`、`beforeLlmRequest` / `afterLlmResponse`、`beforeTodoUpdate` / `afterTodoUpdate`、`beforeCompaction` / `afterCompaction`、`beforeSkillLoad` / `afterSkillLoad`、`beforeSubagentStart` / `afterSubagentEnd` |
| `control-plane`（控制面） | **仅内置 / 受信**可 `R` / `D`；第三方只能 `Observe` / `Note` | `beforePluginToggle` / `afterPluginToggle`、`beforeThreadCreate` / `afterThreadCreate`、`beforeThreadDelete` / `afterThreadDelete`、`beforePersist` / `afterCheckpoint`、`beforeTurn`（否决整轮 = 拒绝服务）、`beforeSystemPrompt` 的**删除他人片段**动作、`beforeApproval` 的**加严**动作 |
| `observe-only` | 任何级别都只能 `Observe` / `Note` | `afterTurn`、`onThreadSwitch`、`afterApproval` |

三条配套规则：

1. **分级挂在"点位 × 动作"上，不只在点位上。** `beforeSystemPrompt` 就是例子：第三方可以**追加自己的**
   片段（`R`），但不能删除别人的片段——删除要求 `control-plane`。同理 `beforeApproval`：
   第三方可以观察、可以**加严**（强制升级为"要问"），但**永远不能放行**（§5.4 已定，且不受档位影响）。
2. **声明即授权。** 订阅 `control-plane` / `observe-only` 点位必须在 manifest 里显式声明
   （§5.4 的 `hooks: {...}`），未声明即不调用（缺席可见，§2.2）；**越级动作在装配期就被拒并产诊断**，
   而不是运行期静默忽略——静默忽略会让插件作者以为自己的拦截生效了。
3. **与档位的关系。** §6.3 第 2 条"完全访问不解除插件钩子的 `Deny`"与本节对齐后更精确：
   `data-plane` 的 `Deny` 可以给一个显式开关（默认仍拦），**`control-plane` 的 `Deny` 不给开关**。

一句话总结分工：**分级回答"谁能拦"，§5.2 回答"拦了之后发生什么"。**

---

## 6. 审批与权限档位：新增「完全访问」

> 需求：审批权限再加一档「完全访问」，选中后**完全放行**，包括脚本执行、文件操作、网络访问。

### 6.1 现状：三档各自拦在哪

档位定义在 `crates/agent-proto/src/dto.rs:152-157`（`Auto` 默认 / `Ask` / `Readonly`），
入盘解析在 `crates/agent-rpc/src/state/store.rs:157-162`，判定在
`crates/agent-adapter/src/approval/policy.rs` 的 `should_ask_approval`，闸门在
`crates/agent-node/src/approval/gate.rs:78`（读 `StoreBackedNodeConfig::approval_mode()`）。
界面选项在 `desktop-ui/src/components/Composer.tsx:105-109`，下发方法是 `config.setApproval`。

| 档位 | 界面文案 | 拦在哪（`should_ask_approval` 的规则序，`crates/agent-adapter/src/approval/policy.rs:141-184`） |
|---|---|---|
| `auto`（默认） | 自动批准——读写文件和执行命令都不再询问 | **规则 0：无条件短路**（`:151-153`），连危险命令的二次确认也一并免掉 |
| `ask` | 每次询问——每次敏感工具调用都等你确认 | 规则 1 危险命令二次确认（`:158-164`）→ 规则 4 其余一律问（`:179`） |
| `readonly` | 严格只读——只允许读取操作，修改需手动确认 | 规则 2：**写工具一律问**（`:168-170`）。注意它拦的是"问"不是"禁"——写操作仍可由用户放行 |

而下面这些边界**与档位无关**，任何档位都不放宽——这正是「完全访问」要动的地方：

| 边界 | 拦在哪 | 与档位的关系 |
|---|---|---|
| **工作区路径沙箱** | `WorkspaceScope::resolve_path` → 越界一律 `DenialKind::Sandbox`（`crates/agent-adapter/src/scope/workspace.rs:48-51`，用例 `:89`） | 独立于档位；`auto` 也写不了工作区外 |
| **插件/脚本的 fs / net / process 注入** | §3.5 的 `needs`，默认 `fs: read`、`net: none` | 现在是"全量注入"，尚无清单 |
| **工具超时与取消** | `RunPolicy::tool_timeout` + `long_running` 豁免名单 | 防挂死，不是权限 |
| **子智能体收窄 / 只读强制** | profile 白名单 + `ReadonlyEnforcingGate` | 派活时的契约 |
| **模式（code / plan / create）** | 正交维度 | 与档位互不覆盖 |

🔴 **必须写清的诚实点**：`auto` 已经等价于"任意命令执行"（规则 0 短路一切，包括危险命令）。
所以「完全访问」对**命令执行没有任何新增放行**。它的真实增量只有三项，需求里的三件事正好对应：

| 需求原话 | 现状 | 完全访问的增量 |
|---|---|---|
| 脚本执行 | 脚本沙箱**根本没有生产调用点**（§1.1） | 插件脚本可运行（**依赖 P1**，之前无效） |
| 文件操作 | 工作区内可读写，**工作区外一律拒** | 工作区**外**也可读写 |
| 网络访问 | 插件默认 `net: none`；联网工具另算 | 插件 `fetch` 与联网工具放行 |

不这么写，界面就会承诺一个本来已经存在的差异——那正是 §1.3 那份"看起来装上了"清单的病根。

### 6.2 模型：界面一个选项，内部两层

不把"能不能越界"塞进审批档位（两者正交），而是复用 §3.5 已经引入的 `needs` 清单：

- **层一 审批档位**（问不问）：`auto | ask | readonly`
- **层二 权限清单**（能碰什么）：`needs { fs: read|write|outside, net: …, process: … }`
- **完全访问 = 审批档位取 `auto`（不再询问）+ 权限清单取最宽（fs 读写含工作区外、net 全开、process 可执行）**

好处：一条权限语言管所有消费者（fs 工具、命令工具、插件脚本、联网工具），将来还能自然组合出
"ask + 全盘"这种档位，不必再往审批枚举里塞维度。

**协议面改动**：`ApprovalMode` 加 `Full`（serde 输出 `"full"`），`client-ts/dto.ts:6` 的联合类型加
`'full'`，`config.setApproval` 方法本身不变（值域扩一个），`store.rs:157-162` 解析加 `"full"`。

🔴 **顺带修一个既有的失败方向问题**：现在未知字符串落到 `_ => Auto`（**放开**）。
新增档位后，"老版本读到 `full`" 会静默变成 Auto。建议把兜底改成**更严的一档（`Ask`）**并加用例
——这与 AGENTS 的失败安全口径一致：读不懂的值应当收紧，而不是放开。

### 6.3 放行什么 / 不放行什么

**放行**（三类，与选择语义一一对应）：脚本执行（P1 后生效）、工作区内外的文件操作、网络访问。

**不放行**（硬边界，写进契约，不随档位变化）：

1. **工具超时与取消**——防挂死，不是权限；`long_running` 豁免名单不因档位改变。
2. **插件钩子的 `Deny`**——§5.1 已定"只收窄不放宽"。一个档位开关不该静默关掉用户装的扩展策略；
   若确实要"连插件也别拦"，那是**另一个显式开关**（默认关），而不是复用这一档。
3. **只读子智能体与 `plan` 模式的收窄**——正交维度，冲突时取更严。
4. **审计留痕**——每次自动放行产埋点（新增 `AutoApproved{mode, tool, path?}`，接 §2），
   越界路径**逐次记录**；用户要能在诊断面板里回答"刚才它到底动了什么"。
5. 🔴 **应用自身凭据目录 `~/.a-da` 不可越**——那不是"用户的项目文件"，是 agent 自己的配置与
   API 密钥；放行它等于把密钥交给任意脚本。建议做成**不可解除**的红线（不进"再点一次就放行"的清单）。

### 6.4 交互与护栏

- **会话级**，不是全局默认：现状 `config.setApproval` 写的是全局配置（`spec/proto/ada-coding.ext.json:17`
  的描述就是"设置全局审批策略"）。完全访问建议**只作用于当前会话**，进程重启回落 `auto`。
- **开启时一次确认**：弹窗逐条列出将放行的三类能力——这正是"选择之后完全放行"的落点：
  确认在**开启**时，不在每次动作时。
- **常驻可见**：输入框上方与标题栏各一处指示当前处于完全访问，一键撤回 `auto`。
  （不常驻可见的宽松档位 = 用户过十分钟就忘了自己开过。）
- **埋点闭环**：`AutoApproved` + 越界路径逐条 + 诊断面板可按 `mode` / `origin` 过滤（接 §2.5）。

### 6.5 实现落点（文件级）

| 层 | 改动 | 文件 |
|---|---|---|
| 协议 | `ApprovalMode::Full`；TS 联合类型；`config.setApproval` 描述扩写 | `crates/agent-proto/src/dto.rs:152`、`crates/agent-proto/client-ts/dto.ts:6`、`spec/proto/ada-coding.ext.json:17` |
| 配置 | 解析 `"full"`；**兜底从 `Auto` 改为更严的一档** | `crates/agent-rpc/src/state/store.rs:157-162` |
| 审批 | 规则 0 覆盖 `Full`（与 `Auto` 同为短路），权限档位单独携带 | `crates/agent-adapter/src/approval/policy.rs:138-176` |
| 路径 | 按权限档位装配 `Scope`：新增"允许越界 + 硬红线"语义的实现，**`id()` 必须与工作区档一致**（落盘分组/事件归属不能因档位而变） | `crates/agent-adapter/src/scope/workspace.rs`、组合根 `crates/agent-host/src/lib.rs:274` |
| 脚本 | `HostCtx` 的 `needs` 取最宽（§3.5） | §3.2 的 `ScriptHost` |
| 界面 | 第四个选项 + 开启确认弹窗 + 常驻指示 + 一键撤回 | `desktop-ui/src/components/Composer.tsx:105` |
| 门禁 | 非 `Full` 档越界必须被拒（用例）；未知档位值必须落到更严的一档（用例） | `tools/xtask/src/gates.rs`、`crates/agent-adapter/src/scope/workspace.rs` 用例 |

### 6.6 验收判据

- `full` 档：工作区**外**写文件成功，且产出带路径的 `AutoApproved` 埋点；
- 切回 `auto`：同一路径立刻得到 `DenialKind::Sandbox`（证明增量确实挂在档位上，而不是"一直都能写"）；
- **任何档位**：`~/.a-da` 越界一律被拒（红线用例）；
- `readonly` 档：写工具仍被拦（既有测试不放宽）；
- 未知档位字符串 → 落到更严的一档（新用例）；
- P1 之前：界面对"脚本执行"一项必须标注**尚未生效**（不得承诺做不到的事，§1.3 的规矩）。

---

## 7. 分期落地与验收

每期都能独立验收，前一期是后一期的基础。

| 期 | 内容 | 验收判据（可执行） |
|---|---|---|
| **P0 埋点地基** | `TraceCtx` + 诊断事件族 + 三条消费路径 + 门禁 check K/L | `cargo xtask verify-wiring` 新增两条 check 通过；`DebugPanel` 能按 `origin` 过滤；契约测试：一次回合的埋点四类齐全，且"无插件"时产出 `Skipped` 而非空 |
| **P1 脚本宿主接通** | `ScriptHost` 端口 + `PluginTool` 适配器（含 `ToolReceipt` 唯一转换）+ 装配期收集 + L1/L2 预算 | 插件工具能进 catalog 并被模型调用，回执 `status == success`（即 `plugin-sdk/v1.md` §7 的冒烟检查在 Rust 上真正成立）；`while(true){}` 得到 `Timeout` 回执而非挂住 |
| **P1b 完全访问档位** | 第四档（审批档位 + `needs` 权限清单两层）+ `Scope` 按档装配 + 开启确认与常驻指示 + 未知值兜底收紧 | §6.6 的六条判据；其中"切回 `auto` 后同一路径立刻被拒"是证明增量真的挂在档位上的关键用例 |
| **P2 统一贡献装配** ✅ | `PromptPlan` / `SkillIndex` / `HookTable` + 插件提示词片段 + 插件技能 + 受控重装配 | 四条判据全部达成（2026-10-16）：逐字节回归全绿；`PromptComposed` 按片段报来源与字节；插件技能进扫描结果且 `plugin_id` 非空；重装配有生产调用者（`plugin.setEnabled` 回执带差异，端到端断言"停用后下一轮提示词里就没有了"） |
| **P3a 钩子（先能拦）** ✅ | 6 个点位（引擎 2 + 管理面 4），只开 `Deny` / `Note`，配齐成对埋点 + 订阅分级 + hooks 能力位翻 true | 四条判据全部达成（2026-10-16）：成对埋点契约测试绿、缺席可见非空；真插件在 `beforeToolCall` 拦下且理由作为普通工具结果回模型；第三方订阅 `beforePluginToggle` 的 `Deny` 被装配期拒并产诊断；`capabilities.hooks` 从 false 变 true 且有生产消费者 |
| **P3b 钩子（再能改）** ✅ | 开 `beforeToolCall` / `beforeTodoUpdate` 的 `Replace` + §5.6 的重校验与风险单调性 | 三条 DoD 全部达成（2026-10-16）：改写 args 后审批**必须重新触发**；改写出的越界路径在 `auto` 档被拦、`full` 档放行但留痕；风险等级不得被降低（降级企图被拒 + 用回原参数）。`beforeTodoUpdate`/`afterTodoUpdate` 同样落地（工具自己跑，`diffTodos` 纯函数） |
| **P4 隔离强化** ◐ | L3 子进程档 ✅ + 权限清单与网络策略（`@ada-needs` 声明裁剪注入 + `NetPolicy` 超时/白名单）✅ + 诊断面板 ✅ | 三条全部达成（2026-10-16）：不声明 `net` 的插件拿不到 `fetch`（两个方向都验）；子进程档下吃内存的插件被 `kill` 且宿主存活、下一次调用照常；白名单之外的域名**在建连之前**被拒；插件卡渲染自述诊断、诊断日志按来源聚合（条数 + 最近点位）。**未做**（见 `docs/unfinished-features.md`）：进程级内存**限额**（只做到崩溃隔离，没有 RSS 上限）、网络策略的配置文件/界面入口 |

---

## 8. 风险与取舍

- **兼容性**：提示词是**逐字节敏感**的（现有断言）；协议新字段一律可选；老客户端忽略未知事件。
  技能"索引化"会改变模型可见信息，属行为变更，需单独决策（§9 决策点 4）。
- **安全**：插件是第三方代码。默认保守 + 权限声明 + L2 指令预算 + 可选 L3 进程隔离；
  注意现状 fs 沙箱只覆盖 `ada-ts-engine` 的原生调用，插件绕过原生 API 自造 IO 的路子要靠
  "不注入即不存在"来堵（§3.5）。
- **性能**：每次调用新建沙箱 → transpile 缓存 + 预热；技能全量注入 → 索引化（用埋点先量化）。
- **INV 一致性**：不引第二份引擎（`ada-ts-engine` 仍是唯一）、不建第二份事件流（仍走 `EventSink`）、
  端口无默认实现、不引入共享全局态（插件实例默认不常驻）、依赖方向不变
  （`ScriptHost` 的实现在 `agent-node`，引擎只认识端口）。
- **门禁强度**：check K/L 只能证明"有这个符号且有发射者"，**不能**证明"行为正确"。
  真正的强度来自 P0 的契约测试（断言副作用 + 缺席可见）。

## 9. 需要拍板的决策点

1. **插件工具的描述符来源**：(a) 只认注册表——插件工具必须先在 `registry.rs` 登记名字，
   否则拒绝加载；(b) 允许插件声明 schema，`access` / `approval` / `rollback` 由宿主按
   §4.5 的保守默认判定。**(a) 更安全、与 INV-3 更一致，但第三方插件无法自带新工具名，
   等于插件只能"重新实现已有工具"。(b) 才是真正的插件生态，代价是描述符出现第二个来源。**
   建议：先 (a) 跑通执行面，再按"名字前缀 + 宿主保守默认"放开 (b)。
2. **沙箱实例策略**：默认每调用新建（无状态、可回收）vs 每会话常驻（性能，但要处理状态）。
   建议按 §3.3 取前者，把常驻留作优化项。
3. **严格隔离档（L3 子进程）**是否排期：不做的话，内存暴涨类插件仍然只能靠 L2 之外的手段
   （目前没有）。建议 P4 做，因为它是"敢让第三方插件进生产"的前提。
4. **技能是否从"全量内联"改"索引 + 按需"**：这会改变模型行为。建议先用 P0 埋点量化
   技能正文的字节/占比，再决定。
5. **钩子首批点位**确认为 `beforeToolCall` / `afterToolCall` / `beforeApproval` 三个？
   （与既有设计文档一致，但归档时代是 26 个点位，需要明确"只做 3 个"是阶段目标还是终态。）
6. **改名的范围**：库名已改为 `ada-ts-engine`（`use ada_ts_engine::…`），但**包目录仍是
   `crates/ts-engine`**、**CLI 二进制名仍是 `ts_engine`**（`[[bin]] name = "ts_engine"`，
   help 文本与 `argv[0]` 也用它）。要不要一并改成 `crates/ada-ts-engine` / `ada-ts-engine`？
   改目录会牵动 AGENTS.md、README、`docs/agent-base-wiring-plan.md` 等处的路径引用（都是"口径"文档），
   所以这次只动了库名。改二进制名则影响 `ts_engine run <script.ts>` 这个 CLI 用法。
7. **`beforeToolCall` 的 `Replace` 是否允许改工具名**（不只是改 args）：允许的话，
   一次调用可以被"换成另一个工具"，表达力更强但也更难审计。建议**先只允许改 args**，
   工具名保持不可改（名字变了等于换了一次调用，与"写在卡片上的工具名"不一致）。
8. **「完全访问」是否连插件钩子的 `Deny` 一起放行**：建议默认**仍拦**（§6.3 第 2 条），
   另给一个显式开关承担"连扩展策略也别拦"的场景——不要把它并进这一档，否则一个档位开关会
   静默关掉用户装的扩展策略。
9. **`~/.a-da` 红线是否可解除**：建议**不可解除**（§6.3 第 5 条）。把 agent 自己的 API 密钥
   交给任意脚本，不属于"访问用户文件"的范畴，给个"再点一次就放行"的按钮等于迟早会被点掉。
10. **「完全访问」是会话级还是全局持久化**：现状 `config.setApproval` 是全局且入盘
    （`spec/proto/ada-coding.ext.json:17` 的描述就是"设置全局审批策略"）。建议完全访问
    **只作用于当前会话**、进程重启回落 `auto`（§6.4），避免"两周前开过一次，一直开着"。
11. **权限清单的默认值是否要按产品区分**：`needs` 默认 `fs: read`、`net: none`（§3.5）。
   生活类助手（非编码产品）可能希望默认就能联网。这属于产品声明（`capabilities`）该管的事，
   但要先定：**默认值住在产品 spec 里，还是住在引擎里**？建议住产品 spec（能力优先，INV-10）。
12. **管理面点位的"受信"门槛**（§5.9）：`control-plane` 点位只给内置/受信来源拦下权。
   什么算"受信"？候选三条：① 只认内置（最保守，第三方连 `beforePluginToggle` 的 Observe 都不给）；
   ② 工作区本地的插件算受信（用户自己放的 = 用户自己负责）；③ 需要签名或用户逐项授权。
   建议先 ①，把"受信"留到有真实生态需求时再开。
