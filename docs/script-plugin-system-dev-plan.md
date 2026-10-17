# 脚本系统与插件平台 开发计划

> **依据**：[docs/script-plugin-system-design.md](script-plugin-system-design.md)（设计与现状分析）、
> [docs/plugin-sdk/v1.md](plugin-sdk/v1.md)（插件契约）。
> 本文只给**开发计划**：前置决策门、任务分解、依赖顺序、验收门、风险与回退、不做清单。
> **定位**：与设计文档同一规矩——本文是**提案**，落地时并入
> [docs/agent-base-plan.md](agent-base-plan.md) 的里程碑；设计口径仍以
> [docs/agent-base-design.md](agent-base-design.md) 为准，本文不构成第二口径。
> 所有现状数字均可复核（命令与文件行号在设计文档里标注）。

---

## 0. 计划口径

| 项 | 口径 |
|---|---|
| 目标 | 把"插件系统"从**只有管理面**推进到**有执行面**，并让每个扩展点可观测、可验证 |
| 范围 | 设计文档 §2 埋点、§3 脚本系统（含 §3.9 引擎待完善）、§4 统一贡献模型、§5 点位契约与订阅分级、§6 完全访问档位 |
| 不做什么 | 见 §7；其中最重要的一条：**不做 26 个点位全量**，只做设计文档确认的首批 |
| 门 | 每阶段都要过 §4 的全部门（两个独立门 `bun run typecheck` 与 `cargo test` 都不许跳） |
| 估算 | 相对人日（粗估，**非承诺**；本仓历史 MVP 估算是 37 人日量级，本计划约 **55–80 人日**） |
| 前置 | §1 的 12 个决策点里，标记为「阻塞」的必须先定，否则对应任务会返工 |
| 交付形态 | 每阶段**可独立验收、可独立回退**（能力位保持 `false` 即对外不声明，见 §6.2） |

**一条贯穿全程的纪律**：任何"看起来装上了"的东西不许留在主干——要么接通并产出可断言的行为，
要么显式登记为缺口（`docs/unfinished-features.md`）。本计划的 P0 就是为了让这件事**机械可查**。

---

## 1. 前置决策门（开工前必须定）

编号沿用设计文档 §9。**「阻塞」= 不定就没法开工对应任务**。

| # | 决策 | 阻塞 | 建议 | 出处 |
|---|---|---|---|---|
| 1 | 插件工具的描述符来源：只认注册表 vs 允许插件自带 schema | **P1**（SP-15/16） | 先只认注册表跑通执行面，再按"名字前缀 + 宿主保守默认"放开 | §4.5 / §9.1 |
| 2 | 沙箱实例策略：每调用新建 vs 每会话常驻 | **P1**（SP-11） | 每调用新建（无状态），常驻留作优化 | §3.3 / §9.2 |
| 5 | 钩子首批点位确认（3 个） | **P3a**（SP-50） | 确认 `beforeToolCall` / `afterToolCall` / `beforeApproval` | §5.4 / §9.5 |
| 7 | `beforeToolCall` 的 `Replace` 是否允许改工具名 | **P3b**（SP-60） | 先只允许改 args | §9.7 |
| 12 | 管理面点位的"受信"门槛 | **P3a**（SP-54） | 先只认内置 | §5.9 / §9.12 |
| 8 | 完全访问是否连插件钩子的 `Deny` 一起放行 | P1b（SP-34） | 默认仍拦，另开显式开关 | §6.3 / §9.8 |
| 9 | `~/.a-da` 红线是否可解除 | P1b（SP-33） | 不可解除 | §6.3 / §9.9 |
| 10 | 完全访问是会话级还是全局持久化 | P1b（SP-35） | 会话级，重启回落 `auto` | §6.4 / §9.10 |
| 11 | `needs` 默认值住产品 spec 还是引擎 | P1b / P2 | 住产品 spec（能力优先） | §3.5 / §9.11 |
| 3 | L3 子进程隔离是否排期 | P4（SP-70） | 排期，它是"敢让第三方插件进生产"的门槛 | §3.4 / §9.3 |
| 4 | 技能是否改"索引 + 按需加载" | P2（SP-45） | 先用 P0 埋点量化技能正文占比再定 | §4.3 / §9.4 |
| 6 | 改名范围（包目录 / CLI 二进制名） | **不阻塞** | 库名已改；目录与二进制名可随手一起改 | §9.6 |

---

## 2. 任务分解

任务编号 `SP-xx`（Script/Plugin）。「大小」S ≈ 半天，M ≈ 1–2 天，L ≈ 3–5 天（粗估）。
每阶段的**验收**列的是可执行判据，不是"看起来对"。

### P0 埋点地基（约 8–12 人日）

**目标**：让"扩展点是否真被调用"从人工审查变成机械断言。这是后面所有阶段的地基——
没有它，P1/P2/P3 的"接通了"都不可证（设计文档 §1.4 第 1 条）。

| ID | 任务 | 目标文件 | 依赖 | 大小 |
|---|---|---|---|---|
| SP-01 | `TraceCtx` / `Origin` 领域类型；`AgentEvent` 信封加字段（wire 可选） | `crates/agent-base/src/domain/trace.rs`（新）、`domain/event.rs` | — | M |
| SP-02 | 引擎填 `turn_id` / `step_index`；工具事件补 `origin` | `crates/agent-base/src/engine/runtime.rs:126,503,549` | SP-01 | M |
| SP-03 | 诊断事件族（插件 / 脚本 / 钩子 / 提示词 / 技能 / 决策 / 审批 / 取消） | `domain/event.rs` | SP-01 | L |
| SP-04 | 节点侧埋点发射：插件生命周期、技能、审批等待、脚本调用 | `crates/agent-node/src/plugins/*`、`skills/*`、`approval/*` | SP-03 | M |
| SP-05 | 诊断投影 + 日志桥（诊断事件 → tracing）+ 内容截断与脱敏 | `crates/agent-rpc/src/server/emitter.rs`、`server/dispatch.rs` | SP-03 | M |
| SP-06 | 诊断面板结构化 + 按 `origin` 过滤；接通一直闲置的 `diagnostics` 字段 | `desktop-ui/src/components/DebugPanel.tsx` | SP-05 | M |
| SP-07 | 门禁 **check K**（每个诊断事件变体必须有生产发射者，**新族不许进豁免表**）+ **check L**（每个能力位必须有生产消费者，顺带修 `capabilities.streaming`） | `tools/xtask/src/gates.rs` | SP-03 | M |
| SP-08 | 契约测试：一次回合埋点四类齐全；**无插件时产出 `PluginSkipped` 而非空** | `crates/agent-rpc/tests/`、`crates/agent-node/src/**` | SP-03/04 | M |

**验收（P0 DoD）**

- `cargo xtask verify-wiring` 含 check K / check L 且全绿；
- `cargo xtask verify-spec` 全绿（`streaming` 要么有消费者、要么从声明里删掉）；
- 契约测试断言：① **当天存在的**全部埋点族齐全；② **缺席可见**（无插件 → `Skipped`，不是空）。
  ——①里的"全部"以 `DIAG_POINTS` 为准（已实现的点位），钩子与脚本两类点位是 P3/P1 的范围，
  P0 不为它们造假发射方；
- `DebugPanel` 能按 `origin` 过滤出一段回合的全部扩展点调用。

> **P0 进度（首次落地）**
>
> 已交付并可复核：`TraceCtx` / `Origin`（`crates/agent-base/src/domain/trace.rs`）、
> `Diagnostic` 变体 + 点位词汇表（`domain/diag.rs`）、事件信封带 `trace`
> （`domain/event.rs`）、引擎填 `turn_id` / `step_index` / 工具归因
> （`engine/runtime.rs`）、唯一投影点（`crates/agent-rpc/src/server/diag.rs`）、
> 扫描类发射方（`crates/agent-rpc/src/server/dispatch.rs` 的 `plugin.list` / `skill.list` 动作点）、
> 插件自述诊断（`crates/agent-node/src/plugins/manager.rs`）、面板过滤
> （`desktop-ui/src/components/DebugPanel.tsx`）、check K / check L（`tools/xtask/src/gates.rs`）。
>
> **7 个已登记点位都有真发射方**：`prompt.composed`（每轮一次 + 字节数）、
> `approval.waited`（**补掉"审批等待无耗时埋点"这个 §1.2 缺口**）、`turn.aborted`（带中止点）、
> `plugin.discovered` / `plugin.skipped`（含**缺席信号** `no_plugin` 与如实信号
> `execution_not_wired` → **后者已在 P1 接线后删除**，见 §P4 的注）、
> `skill.loaded` / `skill.skipped`（聚合 + `no_custom_skill`）。
>
> **实现口径的三处偏离（都已同步进设计文档）**：
> 1. 不是"一个点位一个变体"，而是**一个 `Diagnostic` 变体 + `point` 字段**——加一个点位
>    不该动五个文件的穷尽匹配；
> 2. `DIAG_POINTS` **只登记真有发射方的点位**，点位随发射方一起长，不许先声明后实现；
> 3. **扫描类诊断在 RPC 的用户动作点发**（`plugin.list` / `skill.list`），而不是让节点层自己持
>    sink：此刻没有轮次在跑，节点层要发事件就得先处理"同步回调 ↔ 异步 store"的桥接。
>    P1 插件真正参与装配时，发射点随装配迁到节点侧（那时它才需要成为一等发射方）。
>    技能**只报聚合一条**：技能解析器每步跑一次，逐条报会把 60 条环形缓冲刷爆。
>
> **SP-06 顺带补了一条此前不存在的链路**：服务端诊断缓冲（`AgentStore.log`）虽然随快照下发，
> 但面板只读客户端自己记的条目——**诊断根本没有读出路径**，写进去就没人读。
> 现在 `ws-client` 把快照的 `log` 并入面板数据源，并按 `payload.origin` 过滤（来源清单**派生**，
> 不写死：来源会随分期增长）。
>
> **仍未做**（按计划属于后续阶段，不是 P0 缺口）：`ScriptInvoked` 系列的发射方（P1）、
> 钩子点位的发射方（P3）、`DebugPanel` 的插件维度聚合（SP-74，P4）。

**风险与回退**：事件膨胀 → 内容字段默认不记（只记元数据）+ 采样开关（§2.2 第 3 条律）；
新增 wire 字段一律可选，老客户端忽略即降级，回退点=保持 `TraceCtx` 不出引擎。

---

### P1 脚本宿主接通（约 15–20 人日）

**目标**：插件工具**真的能被模型调用**，且超时/取消/预算只有一处判定。
这一步做完，`plugin-sdk/v1.md` §7 的冒烟检查才第一次在 Rust 上真正成立。

| ID | 任务 | 目标文件 | 依赖 | 大小 |
|---|---|---|---|---|
| SP-10 | `ScriptHost` 端口（`load` / `invoke` / `dispose`，无默认实现） | `crates/agent-base/src/ports/scripts.rs`（新） | — | M |
| SP-11 | `HostCtx`：预算（timeout / bytes / 指令数）+ `CancelToken` + 埋点 sink；**收敛三套超时** | 同上 + `crates/agent-node/src/scripts/*`（新） | SP-10 | M |
| SP-12 | `ada-ts-engine` 预算化 `Context`：`set_loop_iteration_limit` / `set_recursion_limit` / `set_stack_size_limit`（Boa 0.22 已提供，现零使用） | `crates/ts-engine/src/runtime/mod.rs` | — | M |
| SP-13 | 环境注入可裁剪（最小版 `needs`：`fs: read`、`net: none`） | `crates/ts-engine/src/env/mod.rs` | — | M |
| SP-14 | 沙箱 stdout/stderr 结构化捕获 + 截断 + 带 `plugin_id` 归属 | `crates/ts-engine/src/runtime/mod.rs`、`crates/agent-node/src/scripts/*` | SP-11 | S |
| SP-15 | `PluginTool` 适配器：**`ToolReceipt` 唯一转换点**（错误/超时/拒绝/预算超限映射表） | `crates/agent-node/src/scripts/*` | SP-10 | M |
| SP-16 | 装配期收集插件工具进 catalog（描述符按决策 1 的结论，默认保守） | `crates/agent-toolkit/src/toolkits.rs`、`crates/agent-host/src/lib.rs:324` | SP-15 | M |
| SP-17 | transpile 缓存（`sha256(源)+引擎版本` → `<app_home>/cache/plugins/`）+ 预热 | `crates/agent-node/src/scripts/*` | SP-10 | S |
| SP-18 | `plugin.validate` RPC（转译 + inspect + 权限 + 预算校验）；登记进 ext spec | `crates/agent-rpc/src/server/dispatch.rs`、`spec/proto/ada-coding.ext.json` | SP-15 | M |
| SP-19 | `agent-conformance` 的 `ScriptHost` 契约测试（超时 / 取消 / 预算三条） | `crates/agent-conformance/src/ports/scripts.rs`（新） | SP-10 | M |
| SP-20 | 端到端：插件工具被模型调用 → 回执 `success`；`while(true){}` → `Timeout` 而非挂住 | 集成测试 + `tools/` 下验收脚本（可选） | SP-16/12 | M |

**验收（P1 DoD）**

- 插件工具出现在模型工具目录里，被调用一次并拿到 `status == "success"`（对应 `plugin-sdk/v1.md` §7）；
- `while(true){}` 得到 `ToolStatus::Timeout` 回执，**进程不挂**（这是 SP-12 的直接证据）；
- 打开 P1 之前**不成立**的那条：`inspect_plugin` / `call_tool` 从"只有测试调用"变成有生产调用者；
- `cargo test --workspace` 含 SP-19 三条契约测试全绿。

> **P1 进度（首批不阻塞任务：SP-10 / SP-12，已完成）**
>
> 已交付：
> - **`ScriptHost` 端口**（`crates/agent-base/src/ports/scripts.rs`）：`load` / `invoke` / `dispose`，
>   **无默认方法体**；预算（`ScriptBudget`）、取消（`CancelToken`）、事件出口（`EventSink`）
>   都**随端口带进来**（§3.2 的三条硬约束），不再让沙箱自己造一套。
> - **生产实现** `TsScriptHost`（`crates/agent-node/src/scripts/mod.rs`）：装载期就挡掉
>   "读不到 / 转译不过"；调用期把**撞限额 / 超时**与"脚本自己的错"分开（前者可纠正）；
>   输出按预算截断且**如实标注**（`truncated`）。
>   —— 端口零实现会被 `verify-wiring` 的 check D 判红，所以**端口与最小实现必须同批交付**。
> - **SP-12**：`ada-ts-engine` 的 Boa `Context` 接上 `RuntimeLimits`（`Limits`：循环 / 递归 / 栈，
>   默认**宽但有限**），限额错误带统一标记 `LIMIT_MARKER`，宿主据此映射成 `BudgetExceeded`。
>
> **接通时实地踩到并修掉两个真 bug（都在 `ada-ts-engine`）**——这是"第一次真的执行这条路径"
> 才会暴露的形态，也解释了为什么之前的 SDK 冒烟检查从没发现：
> 1. **行级转译器给未完成的表达式补分号**：oxc 的 codegen 会把对象的第一个属性留在
>    `export default {` 同一行，于是产出 `{ tools: [{;` → "TypeScript 解析错误"。
>    表现为"插件写得好好的却装载失败"，且**只在第一个属性是数组/对象时复现**
>    （第一个属性是 `name` 就恰好没事）。修法：按括号平衡判断表达式是否写完，而不是假设
>    `{` 独占行；回归用例 `test_export_default_with_inline_nested_opening_is_valid` 钉住。
> 2. **`CallAsyncFunction` 吞掉 eval 错误**（`let _ = ctx.eval(...)`）：撞限额时 Promise
>    永不 resolve，调用方只能白等到墙钟超时——实测 **120 秒**才返回，报的还是"沙箱执行超时"，
>    真因完全看不见。修法：把错误立刻回给等待方。
>
> 仍未做（按计划等决策或排在后面）：**SP-11**（预算逐项收紧 + 三套超时收敛，等决策 2）、
> **SP-13/14**（注入裁剪、stdout 捕获）、**SP-15/16**（插件工具进 catalog，等决策 1）、
> **SP-17/18/19/20**。
>
> **第二批不阻塞任务（SP-19 / SP-13 / SP-17，已完成）**：
> - **SP-19**：`agent-conformance` 新增 `verify_script_host_contract`
>   （`crates/agent-conformance/src/ports/script.rs`）：装载→调用→释放往返、**未知入口**、
>   **预算**（死循环必须秒级返回）、**取消**、释放幂等；`agent-node` 用它验自己的实现
>   （P4 的子进程档接进来就自动受同一套断言约束）。探针是契约自己造的，不依赖调用方。
>   顺带让 `ScriptError::UnknownExport` 有了**真实产生方**（`load` 期探明入口清单，
>   而不是等调用失败再去匹配沙箱的中文文案）。
> - **SP-13**：`ada-ts-engine` 新增 `Needs`（`fs` / `net`）与 `inject_environment`，
>   **未声明即不注入**：`needs.net == false` 时沙箱里 `typeof fetch === 'undefined'`。
>   两处口径写进 SDK 文档（`plugin-sdk/v1.md` §5.1）：
>   ① **原生桥也要裁**——`__native_*` 是挂到 `globalThis` 的，只关包装脚本挡不住直呼；
>   ② 未声明的模块给**说清原因**的错误，而不是 `undefined`/崩溃。
>   🔴 接通时又踩到一个"从没被缺省过"的耦合：require 加载器在对象字面量里直接取
>   `globalThis.fs.promises`，**注入期就崩**（`cannot convert 'null' or 'undefined' to object`）。
> - **SP-17**：转译缓存，**内容寻址**（`sha256(源)+文件名+引擎版本`）+ **有界**（64 条）
>   + **失败不缓存**（否则一条语法错误会僵在缓存里）。带 `transpile_cache_stats()`
>   ——没有它，缓存测试等于空转（纯函数"同样输入同样输出"本来就成立，分不出"命中了"与"又算了一遍"）。
>   实现为**进程内**缓存而非设计里的落盘缓存：跨进程复用要等真有需求（P4 子进程档），
>   在那之前内存命中比读盘还快，没有理由先引入 IO。
>
> **第三批（SP-11 部分）**：两个决策未获答复，按本计划 §6 风险表已有的口径
> "未定的按**建议**值走并记录"先行——**决策 1 = 只认注册表**、**决策 2 = 每调用新建**
> （后者即当前实现）。据此开工 SP-11：
> - **预算真的收紧到沙箱**：循环上限来自 `ScriptBudget.loop_iterations`
>   （用例：50k 上限在 60s 墙钟之前就把死循环打断）；
> - **取消**进出都查（进沙箱前 fail-fast，进了同步段靠进程内限额兜底）；
> - 🔴 **诊断出口的形状改了**：从 `&dyn EventSink` 改成
>   `&(dyn Fn(DiagnosticEvent) + Send + Sync)`。原因：事件信封里的 `seq` 与轮次上下文在
>   引擎手里，脚本宿主伪造不出来——本仓已有工具为了发一条事件而写 `seq = 0` 的先例，
>   那会让"seq 严格递增"的账本记下一堆**假违约**。现在宿主只要"能报一条遥测"，
>   信封由拿得到它的人去套（用例：`script.invoked` / `script.finished` 各一条、
>   带结果与耗时、按插件归因）。
> - 仍缺：**三套超时收敛**（要等 SP-15 把沙箱与引擎的超时并到一处）与 SP-14 的 stdout 捕获。
>
> **第四批（SP-15，按决策 1 的建议值「只认注册表」）**：
> - **`PluginTool` 适配器**（`crates/agent-node/src/scripts/plugin_tool.rs`）：把 `ScriptHost`
>   的调用包装成引擎认得的 `Tool`。三处口径值得记着：
>   ① **描述符取自注册表**（`agent_toolkit::find_tool_descriptor`），名字不在册就**拒绝构造**
>   ——插件不可能自述"我是只读的"来绕过只读档位与只读子智能体（INV-3 + AGENTS §2）；
>   代价如实记着：这一档下插件只能实现**已有名字**的工具，放开"自带 schema"是决策 1 的 (b)；
>   ② **回执转换只有一处**（`outcome_to_receipt`）：成功 → `Success`；撞预算/超时 → `Timeout`
>   且文案**明确告诉模型换路子**（"不要原样重试"）；中止 → `Aborted`；入口不存在 → `Error`
>   并注明"装配期本应发现"；其余 → `Error`；
>   ③ **刻意不覆写 `long_running()`**：插件工具默认受引擎策略超时约束，第三方脚本不该有
>   "我是长任务"的免死金牌。
> - 遥测出口的形状又收敛了一次：从 `ScriptCtx`（每次调用）挪到**宿主构造时**
>   （`TsScriptHost::with_diag`）。理由是它是**宿主级**的事实（谁在跑脚本），放在每次调用的
>   上下文里只会逼每个调用方去凑一个出口——而工具适配器凑不出来（它没有 seq 分配器）。
>   没接出口时退回 `tracing`：宁可只进日志，也不伪造事件信封。
> - 用例 7 条，其中一条就是 P1 DoD 那句：**死循环得到 `ToolStatus::Timeout` 回执**（秒级返回）。
>
> 就 DoD 而言：**两条硬判据都已达成**——
> ① "`while(true){}` 得到 `Timeout` 回执而非挂住"：SP-15 的用例在**工具回执**层钉住
> （真插件 + 真沙箱，秒级返回 `ToolStatus::Timeout`）；
> ② "插件工具进 catalog 被模型调用、回执 `success`"：SP-20 的集成用例跑完整链路——
> 磁盘插件 → 装配收集 → 进 catalog → 模型调用 → 沙箱执行 → 界面卡片 `status = done`
> 且输出确实来自插件脚本。
>
> 仍未做的 P1 尾巴：**SP-14**（stdout 捕获）与 **SP-18**（`plugin.validate` RPC）。
>
> **第六批（SP-14 + SP-18，P1 收尾）**：
> - **SP-14 控制台结构化捕获**：`ConsoleMode { Echo, Capture }` + 每运行时一份的
>   `ConsoleCapture`（上限 8 KiB，**截断时原始字节数继续如实累计**）。两处细节值得记：
>   ① 捕获出口用**线程局部**而不是往闭包里捕获 `Arc`——console 原生是
>   `from_copy_closure`（要求闭包 `Copy`），而**每个运行时独占一个事件循环线程**，
>   线程局部正好等于"每运行时一份"，省掉了为捕获去改 Boa 闭包签名的麻烦；
>   ② 插件路径**不再把 `console.log` 打到宿主 stdout**（那是往 `host.log` 灌水、还看不出是谁打的），
>   控制台输出走**结构化**通道：进回执的 `details.console`（给排查看）+ `script.finished`
>   诊断的字节数，**不进 `output`**（那是给模型看的，混进去等于污染上下文）。
> - **SP-18 `plugin.validate`**（只读自检，不执行工具）：转译过不过 / 装载起不起得来 /
>   每个工具名在不在注册表 / 当前权限与预算是什么，外加一份**人话问题清单**
>   （空 = 可以放心启用）。判据与装配期**共用同一批步骤**，不另写一套"看起来像"的检查；
>   方法已登记进 `spec/proto/ada-coding.ext.json` 与 `client-ts/methods.ts`。
>
> **第五批（SP-16 + SP-20）**：
> - **SP-16 装配期收集**（`crates/agent-node/src/scripts/assembly.rs` + `HostOptions.extra_tools`）：
>   扫磁盘插件 → 逐个装载（装载即"能跑起来吗"的判据）→ 把**名字在注册表里**的导出包成
>   `PluginTool`。三条口径：只认注册表（未注册的产 `tool_not_registered` 诊断）、
>   **失败安全**（一个插件装不起来只跳过它自己，不影响别的插件与内置工具——
>   与 `tools_for_toolkits` 对未知工具包炸装配**刻意不同**：工具包是自己的声明、插件是第三方输入）、
>   内置插件**不走脚本沙箱**（它们的工具是编译进来的 Rust 实现）。
> - 🔴 装配时撞上一个必须先回答的结构问题：**只认注册表 ⇒ 所有注册名都已被内置工具占用**，
>   于是"插件工具进 catalog"必须明确**同名谁胜出**，否则整条执行面是空转。
>   按决策 1 的读法（插件只能**重实现已注册的工具名**）定为**插件胜出**，
>   并逐条产 `plugin.replaced_builtin` 诊断——静默替换等于把用户信任的工具悄悄换掉。
> - **异步装载的位置**：`run_from_spec` 是同步的（连带被同步测试用着），收集要 `await`，
>   所以收集放在**组合根**（产品 `main` 本来就是 async），装进 catalog 与替换审计留在
>   `agent-host`。**动态装配路径（`EngineFactory`）暂不带插件工具**：那条闭包是同步签名，
>   要给它加插件就得先把 `EngineFactory` 改成异步（记为后续小任务）。
> - **SP-20 冒烟**（`crates/agent-rpc/tests/dispatch_integration.rs`）：判据**不看日志、只看 store 里的卡片**
>   ——卡片是界面渲染的依据，也是"模型真的拿到了什么"的投影。

**风险与回退**：每次新建沙箱的启动开销 → transpile 缓存 + 预热（SP-17），必要时先只在单轮内复用；
Boa 预算过于严格导致正常插件误伤 → 预算做成产品声明可调（`policies`），默认给宽值 + 埋点记
`ScriptBudgetExceeded` 以便调参；回退点=保持 `capabilities.plugins` 语义只覆盖管理面并在文档写明。

---

### P1b 完全访问档位（约 5–8 人日）

**目标**：审批权限加第四档「完全访问」，把工作区边界与脚本/网络权限按档放行（设计文档 §6）。

| ID | 任务 | 目标文件 | 依赖 | 大小 |
|---|---|---|---|---|
| SP-30 | `ApprovalMode::Full`（serde `"full"`）+ TS 联合类型 + ext spec 描述 | `crates/agent-proto/src/dto.rs:152`、`client-ts/dto.ts:6`、`spec/proto/ada-coding.ext.json:17` | — | S |
| SP-31 | 档位解析加 `"full"`；**兜底从 `Auto` 改为更严的一档** + 用例 | `crates/agent-rpc/src/state/store.rs:157-162` | SP-30 | S |
| SP-32 | 权限档位两层（审批档位 + `needs` 清单）；规则 0 覆盖 `Full` | `crates/agent-adapter/src/approval/policy.rs:151` | SP-30/13 | M |
| SP-33 | `Scope` 按档装配：允许越界 + `~/.a-da` 硬红线；**`id()` 必须与工作区档一致** | `crates/agent-adapter/src/scope/workspace.rs`、组合根 `crates/agent-host/src/lib.rs:274` | SP-32 | M |
| SP-34 | 插件钩子 `Deny` 的显式开关（默认仍拦） | 与 P3a 的 HookTable 合流 | SP-32 | S |
| SP-35 | 界面：第四个选项 + 开启确认弹窗 + 常驻指示 + 一键撤回 | `desktop-ui/src/components/Composer.tsx:105` | SP-30 | M |
| SP-36 | 埋点：`AutoApproved{mode, tool, path?}` + 越界路径逐次记录 | 接 P0 诊断族 | SP-32 | S |

**验收（P1b DoD）**——设计文档 §6.6 六条，其中三条是硬证据：

- `full` 档：工作区**外**写文件成功且产出带路径的 `AutoApproved`；
- **切回 `auto` 后同一路径立刻被拒**（证明增量真的挂在档位上）；
- **任何档位**下 `~/.a-da` 越界一律被拒（红线）；
- `readonly` 档写工具仍被拦（既有测试不放宽）；未知档位值落到更严的一档。

**风险与回退**：越界写是"能删用户数据"的能力 → 常驻可见 + 一键撤回 + 逐次留痕；
P1（脚本）未完成前，界面对"脚本执行"一项必须标注**尚未生效**，回退点=该档仅放开路径与网络、
脚本部分保持未生效。

> **P1b 进度（SP-30 / 31 / 32 / 33 / 35 完成，36 部分）**
>
> - **SP-30/31**：`ApprovalMode::Full`（`full`）+ TS 联合类型 + spec 描述；
>   入盘解析**认不出的档位倒向更严的 `Ask`**（老实现是 `_ => Auto`，那会让"老版本读到 `full`"
>   静默变成自动放行）并留一条警告日志。
> - **SP-32**：规则 0 覆盖 `Full`（"问不问"这一层与 `auto` 完全一致），
>   另加 `permission_profile_of()` 作为**唯一映射**到第二层（`Workspace` / `Full`）。
> - **SP-33**：`agent-adapter::scope::ApprovalAwareScope`——默认档位**逐字节沿用**工作区沙箱；
>   `Full` 放开 `..` 与绝对路径；`~/.a-da`（应用自身配置与密钥）**任何档位都不可越**，
>   红线判定在**规范化之后**（`..` 拼出来的同一条路径也拦得住）；`Scope::id()` 不随档位变
>   （落盘分组与事件归属按它）。档位真源与审批闸门**共用一处**（`StoreBackedNodeConfig`
>   同时实现 `NodeConfigSource` 与 `ApprovalModeSource`）。
> - **SP-35**：第四个选项 + **开启时一次确认**（弹窗逐条列出放开了什么、仍然不放什么）
>   + **两处常驻指示**（输入区上方与标题栏）+ 一键撤回。
> - **SP-36 部分**：新增 `approval.skipped` 埋点——"声明侧要求审批（非 `Never`）但最终没问"
>   才记，避免 `read_file` 这类免问工具刷屏；**刻意不记档位**（AGENTS §14：引擎不解释档位，
>   档位界面本来就看得到）。越界留痕目前只有 `tracing::warn`（进 host.log）：
>   界面侧那条需要**工具层**的信号（`Scope::resolve_path` 没有事件出口、也没有线程上下文），
>   记在下面。
>
> **与设计的偏离（写明，避免下次当成"设计没做"）**：
> 1. 设计 §6.2 把两层做成"审批档位 × 权限档位"可自由组合；当前是**一个档位同时驱动两层**
>    （`Full` = 不问 + 放开边界）。拆成两个独立档位是后续增强。
> 2. 设计 §6.4 建议完全访问**只作用于当前会话**；当前与其余三档一样是**全局配置**
>    （`config.setApproval`）。会话级需要按会话存档位，属后续。
> 3. **SP-34 未做**：它要挂在 P3a 的 HookTable 上（没有钩子就没有"插件 Deny"可开关）。
> 4. 界面侧越界留痕待补：需要给 `Scope` 加"是否在工作区内"的判断或让工具层拿到根路径。

---

### P2 统一贡献装配（约 8–12 人日）

**目标**：系统提示词 / 技能 / 工具 / 钩子四条来源合成一条装配管线，插件能贡献提示词片段与技能。

| ID | 任务 | 目标文件 | 依赖 | 大小 |
|---|---|---|---|---|
| SP-40 | `Contribution` 抽象 + 装配管线（校验 → 排序 → 产出四项产物） | `crates/agent-runtime/src/catalog.rs`、新增装配模块 | P1 | L |
| SP-41 | `PromptPlan`：把 `compose_system_prompt_full` 的段落拆成有序片段 | `crates/agent-adapter/src/prompt/coding.rs:137` | SP-40 | L |
| SP-42 | **提示词逐字节回归测试**（以现有输出为黄金样本） | `crates/agent-adapter/src/prompt/*` 测试 | SP-41 | M |
| SP-43 | `PromptComposed` 埋点：每片段的来源与字节数 | 接 SP-03 | SP-41 | S |
| SP-44 | 插件提示词片段（`prompt_extensions` 那个空缝被填上） | `crates/agent-host/src/lib.rs:76/137` | SP-41 | S |
| SP-45 | `SkillIndex`：插件技能接通（`plugin_id` 非空）+ frontmatter 未知键产诊断；**是否索引化按决策 4** | `crates/agent-node/src/skills/*` | SP-40 | M |
| SP-46 | 受控的重装配入口（启停真的改变贡献集合），为 P3a 的 `afterPluginToggle` 回执准备 | `crates/agent-runtime/src/catalog.rs:44`（既有 `register/unregister/reload`） | SP-40 | M |

**验收（P2 DoD）** —— 四条**全部达成**（证据见下方各批次说明）：

- ✅ 提示词逐字节回归全绿（**这是不可放宽的一条**：现有断言就是这么钉的）；
- ✅ `PromptComposed` 能回答"这段提示词是谁加的、占多少字节"；
- ✅ 插件技能出现在扫描结果里且 `plugin_id` 非空（`skill.list` 与提示词两条路都验了）；
- ✅ 重装配入口有生产调用者（组合根的 `PluginReassembler` ← `plugin.setEnabled`）。

> **P2 进度（SP-40 已完成）**
>
> `crates/agent-runtime/src/contributions.rs`：四类贡献（工具 / 提示词片段 / 技能 / 钩子）
> + `AssemblyBuilder`（收集 → 校验 → 排序 → 产出 `{tools, catalog, prompt, skills, hooks}`）。
> 三条规则都落成可断言的用例：
> ① **谁的声明谁负责**——内置重名 = 项目自己的声明打架 → **炸**（fail-fast）；
> 插件重名/装不起来 = 第三方输入 → **跳过并记录理由**（fail-safe）；
> ② **同名工具插件胜出**，但**必须记入 `replaced_builtins`**（静默替换等于把用户信任的工具悄悄换掉）；
> ③ **排序稳定**：`order` → 内置优先 → id 兜底（片段顺序变了就是行为变更，不是重构）。
>
> **顺带把 P1 留下的重复实现收掉了**：SP-16 当时把"同名谁胜出"的规则手写在 `agent-host` 里，
> 现在那段装配改走同一条管线（`AssemblyBuilder`），规则只剩一处。
> `HostOptions.extra_tools` 与 `collect_plugin_tools` 的形状从 `Vec<Arc<dyn Tool>>` 改成
> `Vec<(String, Arc<dyn Tool>)>`——**插件 id 是来源标注**，装配归属与埋点都靠它，
> 丢掉 id 就只能报"某个插件"，那等于没归因。
>
> **第二批（SP-41 + SP-42，同批交付）**
>
> - **SP-41 片段化**（`agent-adapter/src/prompt/plan.rs` + `build_prompt_plan`）：
>   提示词从"一串 `push_str`"变成有序片段——`persona` / `workspace` / `guidelines` /
>   `language` / `extensions` / `skills_header` / **`skill:<名字>`（技能逐条成片段）**。
>   两条关键口径：
>   ① **分隔符归片段自己**，`render()` 是**纯拼接**——这样"提示词一个字节不变"是**可验证**的，
>   而不是靠"新拼接规则恰好等于老排版"去口算；
>   ② `PromptPlan` 只有**一份类型**（住在适配器，因为片段是它产出的），
>   `agent-runtime` 的装配管线**复用它**（`pub use`）而不是另立一套等价类型——
>   两份等价类型就是"第二份名单"，迟早会出现"适配器改了字段、管线没跟着改"。
> - **SP-42 逐字节回归**（`crates/agent-adapter/tests/prompt_golden.rs` +
>   `tests/fixtures/prompt/*.txt`）：夹具是**重构前**用当时的代码跑出来的（不是照着新实现写的
>   ——那等于自己给自己判卷），覆盖三条路径（最朴素 / 身份+扩展 / 英文环境+两条技能，
>   后两条分别走 `when_to_use` 与 `description` 分支）。
>   **已验证判据能红**：故意在指引里多打一个空格，测试立刻失败；恢复后转绿。
> - 另有一条结构断言：`plan.total_bytes() == render().len()`——"各片段字节数之和 == 渲染长度"
>   能抓住"拼接时凭空多出字节"这类错（埋点 SP-43 报的字节数就靠它才可信）。
>
> **第三批（SP-43）**：`PromptComposed` 从"只报总字节"升级为**按片段报来源与字节**。
> - 端口 `PromptSource` 加了 `prompt_segments()`（**无默认实现**，所以 5 处实现都补齐了：
>   `CodingPromptSource` 走同一份计划、`FixedPrompt`/子智能体 profile 如实返回"不细分"或一段）。
>   领域类型 `PromptSegment` 放在 `agent-base`——引擎要发这条遥测，而引擎只认识基座。
> - 🔴 **契约里加了一条自洽断言**：非空的逐段报告，各段字节之和必须等于
>   `system_prompt().len()`。**不自洽的拆分比不拆更坏**——埋点会把"这段 200 字节"报出去，
>   排查者照着去追一个不存在的差异。这条断言由 `agent-conformance` 守着，
>   且**真实 `CodingPromptSource` 也要过**（等于顺带验了 SP-41 的拆分自洽）。
> - 引擎侧用例用一个**会逐段报告的**提示词源（测试替身 `FixedPrompt` 不细分），
>   断言 `data.segments` 里真能看到 `plugin:p1` 这种来源——只断言字段存在的话，
>   证明不了"片段真的被报出来"。
>
> **第四批（SP-44）**：插件终于能给模型**加话**，而且加的内容**可归因**。
> - **插件侧的声明面**：`api.registerPrompt`（此前是**空壳函数**）现在真的收集，
>   对象形态也可写 `prompts: [{ id, content }]`；沙箱的 inspect 一并把它们带回来
>   （`PluginSandbox::inspect_plugin_full` → `PluginInspection { tools, prompts }`），
>   端口加了 `ScriptHost::prompt_fragments(handle)`。
> - **进计划**：装配期收集成 `(plugin_id, 片段)`，经 `HostOptions.plugin_prompt_fragments`
>   → `CodingPromptSource::with_plugin_fragments`，追加在**固有段落与技能之后**（order 300+），
>   `source` 标 `plugin:<id>`、`id` 是 `plugin:<pid>:<fid>`——于是 SP-43 的埋点能直接回答
>   "这 200 字节是哪个插件加的"。**空片段不追加**（否则只会多出空行）。
> - 用例钉住"只能在后面追加"这条硬要求：`prompt.starts_with(base)`——插件不许改动固有段落
>   （这是"只收窄不放宽"在提示词上的形态）。
> - **顺带填上了一条真空缝**：`HostOptions.prompt_extensions`（"当前已启用的扩展工具与
>   额外能力"段落）**此前没有任何生产者**（§1.1 记过），现在由插件工具清单喂上——
>   模型因此知道"这些能力存在"。没有插件时该段落不出现，提示词与黄金夹具**逐字节一致**。
>
> **未做（P2 尾声）**：SP-45（`SkillIndex` 接通插件技能）、SP-46（受控重装配入口）。
> —— `PromptPlan` / `SkillIndex` / `HookTable` 三个类型**已就位并有用例**，
> 提示词那条来源也已真的走管线；剩下的是"插件技能"与"重装配"。
>
> **第五批（SP-45 插件技能）**：技能从两来源（目录 / 内置）扩到**三来源**，且**带得住归因**。
> - **声明面真接线**：`api.registerSkill`（此前是空壳）与对象形态 `skills: [...]` 都收下
>   （`PluginInspection { tools, prompts, skills }` → 端口 `ScriptHost::skills(handle)`）。
>   **空名字或空正文的声明被丢掉**——留着只会让界面出现一条占着位置却什么也不说的技能。
> - **三来源合流**：`SkillManager::scan_skills_with(ws, plugin_skills)`，
>   顺序 **目录 → 插件 → 内置**（用户自己写的胜过第三方插件；插件胜过通用兜底）。
>   与工具口径**刻意不同**（工具是"注册表先占名、插件只能重实现"）：技能同名的语义是
>   "这份规范谁更权威"，用户显式写下的那份最权威。
> - **归因**：`SkillSummary.plugin_id` / `plugin_name` 这两个字段**第一次真的被填上**；
>   提示词里那条技能片段在 `prompt.composed` 的字节报表中标 `plugin:<id>`
>   （`SkillPrompt::with_source`），界面技能库多一个插件名徽章与 `plugin` scope。
> - **frontmatter 未知键不再静默**：`SkillSummary.unknownFrontmatterKeys` + 新点位
>   `skill.invalid`（逐条报，带键名清单）。以前拼错一个字母就只有"技能看起来装上了却不生效"。
> - **决策 4 的处置：暂不索引化**（保持全量内联）。理由是它**改变模型可见信息**，
>   属行为变更；而量化它所需的埋点（SP-43 的逐片段字节报表）**这一批才刚刚可用**——
>   等真实使用数据出来再决定，回退点就是现在这个样子。
>
> **第六批（SP-46 受控重装配）**：插件启停**真的改变贡献集合**，并产出可回报的差异收据。
> - 此前 `plugin.setEnabled` 只往配置里写一个布尔值：界面上关掉了，引擎手里那份工具集合、
>   提示词片段与技能**一字未动**——表现正是"关掉了但它还在跑"。
> - 新增 `agent_runtime::reassembly`：`ContributionDigest`（摘要）/ `ContributionDelta`（差异）
>   / `ReassemblyReport`（收据，即设计 §5.9 的 `afterPluginToggle` 载荷）+ `LiveContributions`
>   （**受控**地往活体注册表里 register/unregister）。
> - 装配期就布好三条活体：**工具注册表**（`ProductBuilder::with_catalog`，引擎与重装配用同一本账）、
>   **提示词片段活体**、**技能活体**（`agent-host::LivePluginContributions`）。
>   三者共用同一份句柄，所以"换"是真的换——端到端用例断言**停用之后下一轮提示词里
>   既没有那条技能、也没有那段话**，而内置技能不受影响。
> - 生产调用者：组合根注入三样只有它拿得到的东西（产品声明、`ScriptHost`、活体贡献集合），
>   实现体在 `agent-host::LivePluginReassembler`（放那儿才**测得到**——产品二进制没有集成测试入口），
>   经 `EngineInjection` 送进 `Dispatcher`；`plugin.setEnabled` 的回执从此带上差异；
>   **没注入入口时如实回 `applied: false`**（界面别把"只写了个布尔值"当成已经生效）。
> - 端到端用例走的是**真插件 + 真脚本宿主 + 真重装配器**：磁盘上写一个声明了
>   `registerTool`/`registerSkill`/`registerPrompt` 的插件 → 装配 → 断言三样都在 →
>   停用 → 断言差异里三条齐全、且活体注册表里工具真的没了、下一轮提示词里技能与片段也没了。
>   （踩过的坑记在用例注释里：`scan_plugins` 会先列 9 个内置插件，拿 `first()` 会去停用
>   一个内置插件；`tool_names()` 是装配期快照，断言"停用后工具消失"必须看活体注册表。）
> - 未做（属 SP-55）：把这次收据接到 `afterPluginToggle` 点位上——P3a 的点位与订阅分级同批。
>   UI 目前只消费 `ok`，`report` 字段已就位待人接。

**风险与回退**：提示词改动会改变模型行为 → 逐字节回归 + 变更前后 diff 审查；
技能索引化是行为变更 → 按决策 4 用 P0 埋点先量化占比；回退点=保持全量内联，只做来源标注。

---

### P3a 钩子（先能拦）+ 订阅分级（约 6–10 人日）

**目标**：首批 3 个点位可**拦下**，管理面点位（会话创建、插件启停）落地，
**订阅分级同批交付**（设计文档 §5.9：分级不落地 = 把锁死系统的开关交给第三方）。

| ID | 任务 | 目标文件 | 依赖 | 大小 |
|---|---|---|---|---|
| SP-50 | `HookTable` + 点位注册与调用顺序（内置优先 / `order` 稳定排序 / 链式 / 拦下即定稿） | 新增 hook 运行时模块（`agent-node`） | P2 | L |
| SP-51 | 前置点位接入引擎：`beforeToolCall` / `beforeApproval` + 事后 `afterToolCall` | `crates/agent-base/src/engine/runtime.rs:410,437,547` | SP-50 | L |
| SP-52 | 成对埋点：`HookEntered` + 恰好一个终结事件；无插件产 `HookSkipped` | 接 SP-03 | SP-50 | M |
| SP-53 | 超时即 `Pass`（`hookTimeoutMs` 从"只解析"变成真消费者）+ `HookTimedOut` | `crates/agent-node/src/plugins/manager.rs:138-148` | SP-50 | S |
| SP-54 | **订阅分级**（`data-plane` / `control-plane` / `observe-only` + 动作级例外）+ 装配期拒绝越级 | 与 SP-50 同一模块 + manifest 校验 | SP-50 | M |
| SP-55 | 管理面点位：`beforePluginToggle`/`afterPluginToggle`（只开 `Deny`/`Note`）、`beforeThreadCreate`/`afterThreadCreate`（原子性） | `dispatch.rs:1543,3111` + `manager.rs:81` | SP-53/54/46 | L |
| SP-56 | `capabilities.hooks` 翻 true（**三条翻位条件**：有生产调用 + 成对埋点 + 缺席可见测试） | `crates/agent-rpc/src/server/dispatch.rs:1362` | SP-52 | S |

**验收（P3a DoD）** —— 四条**全部达成**（2026-10-16）（证据见下方"P3a 进度"）：

- ✅ 成对埋点契约测试：每个 `HookEntered` 有且只有一个终结事件；无插件时 `Skipped` 非空；
- ✅ 真实插件在 `beforeToolCall` **拦下**一次调用，理由作为**普通工具结果**回模型（不是"执行失败"）；
- ✅ 第三方插件订阅 `beforePluginToggle` 的 `Deny` 被**装配期拒绝并产诊断**（分级生效的证据）；
- ✅ `capabilities.hooks` 从 `false` 变 `true` 且有生产消费者（check L 看着）。

**风险与回退**：钩子让每个工具调用多一次跨沙箱往返 → 单点位预算默认 500ms + 只在有插件时才走；
拦下语义被误用成"拒绝服务" → 控制面分级（SP-54）已挡；回退点=能力位退回 `false`，点位保留但不调用。

> **P3a 进度（SP-50…SP-56 全部落地）**
>
> **① 运行时（SP-50/53）**：`agent-node::hooks::HookRuntime`。顺序是**内置优先 → `order` → 插件 id**
> （内置代表"本项目自己的规矩"，第三方不该插到它前面）；任一钩子拦下/结束整轮即**定稿**，
> 之后不再问别人；单点位预算来自插件能力的 `hookTimeoutMs`（此前**只解析不消费**），
> **超时与"钩子自己抛错"一律视为无意见**——插件是外部代码，它卡住/坏掉不该拦住工具，
> 但两种情况都**留痕说明**（`timed_out` / `hook_failed:...`），不是静默放行。
> 注册表用内部可变（`RwLock`）：插件启停要**原地重装**它，否则会出现"关掉了插件、
> 它的 `beforeToolCall` 还在拦"——比工具残留更糟，因为那是**行为面**的残留。
>
> **② 引擎接入（SP-51）**：新增端口 `HookRunner`（`agent-base`，**无默认方法体**），
> 注入走 `AgentRuntime::with_hooks`（builder，不动 `new` 的十几个调用点）。
> `beforeToolCall` 插在**审批判定之前**（安全序，设计 §5.6）：P3a 只有"拦下"，
> 所以没有"审批通过后被改写参数"那条漏洞（`Replace` 属 P3b，会带上重校验）。
> `afterToolCall` 的旁注**追进工具结果正文**（模型正在读的那段），前缀写明来自插件，
> 且**不改 `status`**——"工具到底成没成"是已经发生的事实。
>
> **③ 成对埋点（SP-52）**：`hook.entered` / `hook.finished`（带 outcome/耗时/理由）/
> `hook.skipped`（没人订阅，缺席可见）。转换点**只有一处**（`ports::hooks::observation_to_diagnostic`）：
> 引擎（轮内）与宿主（轮外管理面）各写一份的话，迟早出现"引擎报 denied、宿主报 deny"
> 这种对不上的账。观测由端口**实时**回调上报，不是"跑完再补发"——后者会让 `HookEntered`
> 晚于钩子实际结束，时间是编的。
>
> **④ 分级（SP-54）**：判据唯一在 `allows(level, trust, action)`。三级三档：
> `observe`/`note` 任何来源任何点位都行；`deny` 在行为面（`beforeToolCall`）第三方可用，
> 在控制面（插件启停 / 会话创建 / 审批）**只有内置**可用；`abort`（结束整轮 = 拒绝服务）
> 只有内置；`replace` 本批**一律拒绝**（属 P3b，如实说"还没实现"而不是收下不用）。
> 越级 / 未实现 / 重复注册都在**装配期逐条拒并产诊断**——静默忽略会让插件作者
> 以为自己的拦截生效了。本仓**没有"受信"这一档**，所以控制面的 `Deny` 对第三方一律关闭；
> 将来要做，"受信"应当由 manifest 的显式字段驱动，不靠目录猜。
>
> **⑤ 管理面点位（SP-55）**：`beforePluginToggle`（拦下 = **不写盘、不重装配**）、
> `afterPluginToggle`（旁注里带上"贡献 N → M + 差异"）、`beforeThreadCreate`
> （**在任何状态变更之前**判定）、`afterThreadCreate`（**只在落盘成功之后**触发）。
> 它们跑在 Dispatcher 里（轮外，没有 turn 上下文），用的是**同一个** `HookRuntime` 实例
> （注册表共享）。观测按"先攒后发"投影成诊断：诊断要写 store（async），而端口回调是同步的——
> 代价是发出时刻晚一点，换来的是不长时间占着 store 写锁；轮内用同步 sink，所以那边是真·实时。
>
> **⑥ 能力位（SP-56）**：`capabilities.hooks` 从"硬编码 false"改成**从声明派生**，
> `ada-coding` 与 PM 预设声明 `true`（骨架产品仍是 `false`，正好当反例）。
> `CAPABILITY_FIELDS` 登记了两个消费者：握手如实回报 + **装不装钩子运行时**
> （没开的产品连 runner 都不装，一条 `hook.*` 诊断都不发；但会额外报一条
> `hooks_disabled_by_product`，免得插件作者对着自己的代码找问题）。
>
> **端到端证据**：`test_real_plugin_hook_denies_a_tool_call_through_the_engine`
> （真插件 JS → 沙箱 → HookRuntime → 引擎 → 工具结果）；
> `test_control_plane_deny_from_third_party_is_rejected_with_a_diagnostic`（分级 + 诊断）；
> `test_management_plane_hooks_gate_toggle_and_thread_create`（拦下即不写盘 + 原子性 + 旁注）。
>
> **本批刻意没做（属 P3b / 后续）**：
> ① `beforeApproval` / `beforeLlmRequest` / `beforeSystemPrompt` / `beforePersist` 等点位尚未接
>   （`points::IMPLEMENTED` 里没有 = 插件声明它们会被**如实拒**，不是静默不生效）；
> ② `Replace`（改写）——必须与 §5.6 的重校验、风险单调性同批；
> ③ **按工作区动态装配的引擎**（`EngineFactory`）仍不带插件贡献，因而也没有钩子
>   （与 P1 那条限制同源：该路径的闭包是同步的，收不了异步装载结果）；
> ④ 界面尚未显示"这次启停的贡献差异"与"钩子拦下了什么"（数据都在回执与诊断里）。

---

### P3b 钩子（再能改）（约 5–8 人日）

**目标**：开 `Replace`（改写），**并与 §5.6 的重校验、风险单调性同时落地**。

| ID | 任务 | 目标文件 | 依赖 | 大小 |
|---|---|---|---|---|
| SP-60 | `Replace` 语义 + 应用点唯一化（核心侧一处）+ `sha8`/字节差埋点 | 与 SP-50 同模块 | P3a | M |
| SP-61 | **改写后重校验**：schema / `Scope::resolve_path` / 风险重判 → 审批读改写后的 args | `crates/agent-base/src/engine/runtime.rs:434` | SP-60 | L |
| SP-62 | 风险单调性用例：改写不得降低风险等级、不得绕过只读档、不得改描述符字段 | 测试 | SP-61 | M |
| SP-63 | `beforeTodoUpdate` / `afterTodoUpdate`（`diffTodos` 纯函数可独立测） | todo 工具 | SP-60 | M |

**验收（P3b DoD）** —— 三条**全部达成**（2026-10-16）：

- ✅ 改写 args 后审批**必须重新触发**（`test_approval_reads_the_rewritten_args`：
  用例设计成"改之前无害、改之后危险"，闸门必须看到**改写后**的参数）；
- ✅ 改写出的越界路径在 `auto` 档**必须被拦**、在 `full` 档允许但逐次留痕
  （引擎侧 `test_out_of_scope_path_in_replacement_is_rejected` + 档位对照
  `agent-adapter::scope::tests::test_replacement_path_check_follows_the_approval_mode`）；
- ✅ 风险等级不得被降低（`test_risk_downgrade_is_rejected_and_original_args_are_used`：
  企图把 `rm -rf` 改成无害形式骗审批 → 被拒 + 用回**原参数** + 留痕 `risk_downgrade`）。

**风险与回退**：改写是最容易被绕过的一环 → 只开了 `beforeToolCall` 一个点位
（`points::REPLACE_SUPPORTED`），`beforeTodoUpdate` 待接；描述符字段（工具名/读写性）
**不在可改写载荷里**，结构上就改不动；回退点=把 `Replace` 从 `REPLACE_SUPPORTED` 拿掉
（声明它的插件会在装配期被如实拒，机制整体退回 P3a 的"只能拦、只能看"）。

> **P3b 进度（SP-60/61/62 落地，SP-63 未做）**
>
> **① 载荷契约（SP-60）**：插件返回 `{ action: 'replace', args: {...} }`。只接受**对象**——
> 数组/标量没法再走 schema 校验，与其猜不如如实拒并说清契约（`replace_payload_invalid`）。
> 载荷里**只有 args**：工具名、读写性、描述符字段都由核心持有，插件碰不到，
> 所以"把写工具伪装成只读"在结构上做不到（用例 `test_replacement_cannot_change_the_tool_identity`）。
> 多个插件**链式**：前一个的改写是后一个的输入（§5.5）；最后一个改写者进 `by` 字段，
> 于是诊断能回答"这次到底是谁改的"。改前/改后的**内容指纹与字节差**进 `hook.finished`
> 与 `hook.replace`（指纹是 FNV-1a 64 位的短十六进制，名字如实叫 `hash8`——本仓不引 sha 依赖，
> 而这里要回答的只有"改没改、是不是同一份"）。
>
> **② 重校验（SP-61）**：`crates/agent-base/src/engine/hook_guard.rs`，三条：
> **形状**（必须对象）→ **schema**（必需字段 + 顶层类型；**刻意做窄**：内置工具的 schema
> 常常是宽松的，严格校验会把本来能跑的调用拒掉，而真正要挡的东西由下面两条负责）→
> **路径沙箱**（用工具声明的 `Access::Mutates { paths }` 字段，走**外层同一个** `Scope`，
> 于是档位语义自动一致）。任何一项不过就**退回原参数**并留痕 `hook.replace_rejected`。
> 之后才轮到审批，且审批读的是**改写后的** args。
>
> **③ 风险单调性（SP-62）**：判据就是审批端口对同一 (工具, 声明, 参数) 的判定——
> 改之前要问、改之后不用问 = 降级 → 拒。这条补上了最容易被绕过的那一环：
> 因为**执行的是改写后的参数**，允许"把危险形式改写成无害形式"就等于允许骗过审批。
>
> **④ 踩到并修掉的一个真 bug**：`ToolCall` 原先在工具解析时就用**旧** `parsed_args` 组装好了，
> 改写之后只更新了 `parsed_args`——于是变成"事件里报的是新参数、真正执行的是旧参数"。
> 现在执行用的 `ToolCall` **在改写之后**才组装（注释里写明了为什么）。
> 这类"两本账"正是本仓反复强调要消灭的形态，能被用例抓住是好事。
>
> **⑤ SP-63 ✅ 已落地**：`beforeTodoUpdate` / `afterTodoUpdate` 由**工具自己跑**——
> 引擎通过 `ToolContext.hooks`（`ToolHooks { runner, turn_id, call_id, on_event }`）把运行时、
> 归属与遥测出口交给工具。为什么不让引擎代跑：载荷是"清单的**语义**"，只有工具知道
> 哪个字段是标题、什么算一次更新；引擎代跑就只能靠"工具名叫 `todo`"这类硬编码。
> 出口给的是 `HookObservation`（不是事件信封），`seq` 与轮次上下文仍由引擎套上——
> **事件流单一真源没有因此被破坏**。
>
> 三处值得记的取舍：
> ① **权威清单不在 Rust 侧**（模型每次带全量、界面据参数渲染），所以工具只记
>    "每线程上一次见到的清单"用于算 diff，**不落盘、不冒充权威**；重启后第一份快照
>    如实说"（首个快照：没有上一版清单可比）"，而不是把整张清单报成"全是新增"
>    ——那是最像数据、其实最误导的一种输出。
> ② **改写回到工具内重新校验**：必须还是"对象数组 + 非空标题"，否则**不改**并如实报错。
>    这守的是"改写不是授权"（§5.6）在**第二个点位**上同样成立。
> ③ `diffTodos` 是**纯函数**（`core/todo_diff.rs`），把"改标题"与"删一条加一条"的区别、
>    "顺序不算变化"、以及设计点名要单列的**"完成又被改回未完成"**都钉在用例里。
>    口径如实写在注释里：**无 id 时按标题配对，所以改标题必然表现为删+加**——
>    那不是缺陷，是"没有稳定标识就认不出同一条"。
>
> 顺带一提：`AbortTurn` 在工具内部点位**不成立**（结束整轮要引擎才能做），
> 所以工具遇到它时按拦下处理并在理由里说明——不假装自己有那个能力。

---

### P4 隔离强化与诊断完善（约 8–12 人日）

**目标**：让"不可信第三方插件"敢进生产。

| ID | 任务 | 目标文件 | 依赖 | 大小 |
|---|---|---|---|---|
| SP-70 | L3 子进程档（超时 `kill`；子进程 stdin 一律 `Stdio::null()`） | `crates/agent-node/src/scripts/*` | P1 | L |
| SP-71 | 权限清单完整版：manifest `needs` 声明 → 按声明裁剪注入；未声明即不存在 | `crates/ts-engine/src/env/mod.rs` + 装配 | SP-13 | L |
| SP-72 | 网络参数可配（超时 / 域名白名单），默认 `net: none` | `crates/ts-engine/src/runtime/mod.rs:454` | SP-71 | M |
| SP-73 | source map / 行号映射 + 结构化转译错误 | `crates/ts-engine/src/compiler/mod.rs` | — | M |
| SP-74 | 诊断面板完善（插件维度聚合：加载失败、被忽略的重名、越级动作、预算超限） | `desktop-ui` + SP-05 | P0 | M |

**验收（P4 DoD）** —— **2/3 达成、1 条部分**（2026-10-16）：

- ✅ 不声明 `net` 的插件**拿不到 `fetch`**（不是"注入了再拦"）
  （`test_declared_needs_gate_the_network_per_plugin`：两个方向都断言——没声明时 `typeof fetch`
  是 `undefined`，声明 `net` 之后是 `function`）；
- ✅ 子进程档下内存暴涨的插件被 `kill`，并产出诊断事件
  （`crates/agent-node/tests/subprocess_sandbox.rs`：吃内存的插件 2 秒预算内被**杀掉**并收到
  `TimedOut`（可纠正），而**宿主活着**、下一次调用照常起新子进程。⚠️ 它只保证
  "进程死掉这件事被隔离住"，**不保证"它死之前吃多少内存"被限制**——见下方第 ③ 条）；
- ◐ 插件报错能指回**位置**：转译错误已带行列与出错行原文；运行时错误已带文件名、位置与栈。
  **source map 未做**，所以运行时行号是"转译后 JS"的座标（见下）。

**风险与回退**：子进程档增加延迟与复杂度 → 作为**档位**而非默认；回退点=只保留进程内档，
并在文档明确"无内存护栏"这一事实。

> **P4 进度（SP-71 落地；SP-73 部分；SP-70/72/74 未做）**
>
> **① 权限清单（SP-71）**：声明面是插件源码首部的一行指令 `// @ada-needs fs,net`。
> 为什么用**注释**而不是运行时对象：能力必须在**执行之前**定下来——插件得跑起来才知道
> 它声明了什么，那就意味着"先给能力、再按声明收回"，正是本仓否掉的"注入后再拦"。
> 三条口径写死在 `scripts::needs`：**没有指令 → 宿主地板**（`fs` 开、`net` 关，
> 向后兼容既有插件）；**有指令 → 就是列出的那些**（不叠加地板，读者不必做心算）；
> **认不出的 token 一律不授予**并如实报进诊断（`unknown_need:nett`——拼错不能静默通过）。
> 装载期与调用期取**同一份**解析结果（`LoadedScript.needs`），所以不会出现
> "装载期跑得过、调用期跑不过"。`plugin.validate` 的自检报告也改成报**这个插件实际拿到的**
> 档位（附"未声明则按地板"与认不出的 token）。
>
> **② 错误定位（SP-73）**：转译错误从 `Unexpected token` 升级为
> "文件:行:列 + 出错那一行的原文 + 插入符"（`render_oxc_diagnostics`；行列按**字符**算，
> 中文注释不会把列号推到几十）。运行时错误：Boa 的 `Source` 现在带上**源码名**
> （此前是 `unknown at :4:2`），并且**JS 侧 catch 住的那条路**也把 `stack` 带出来
> （工具路径与钩子路径各一处）——在那之前，任何被 JS catch 的错误都只剩一句没有出处的消息。
> 包装脚本的前导部分也压成了一行：插件代码从第 1 行开始，报错行号不再被前导脚本整体顶偏。
>
> **③ 未做（按优先级）**：
> - **SP-70（L3 子进程档）✅ 已落地**：`crates/agent-node/src/bin/runner.rs`（读一条
>   JSON 请求、干一件事、写一条结果；**一次请求一个进程**，不带常驻状态）+ `SubprocessScriptHost`
>   （同一端口、同一份句柄注册表、同一套截断口径与遥测）。**超时即杀**靠 `kill_on_drop`：
>   宿主不再等它的那一刻，子进程就被回收——否则"超时"只意味着宿主不等了，插件还在后台吃内存。
>   档位用 `--script-sandbox inprocess|subprocess` 选（默认进程内）：
>   要求子进程档却找不到入口时**拒绝启动**，绝不静默退回进程内档
>   （"以为在隔离里跑、其实跑在宿主进程里"是最坏的误解）。
>   `electron-builder.json` 的 `extraResources` 也加上了 `runner.exe`，否则打包后
>   这一档会因为找不到入口而拒绝启动（那是对的：宁可拒绝，也不要假装隔离）。
>   ⚠️ **它不做内存限额**：给子进程设 RSS 上限要看平台（Windows Job Object / Linux rlimit），
>   本档只做到"进程死掉这件事被隔离住"。这条限制写在 `docs/unfinished-features.md`。
>   顺带抽出两处共用件：`scripts::registry`（句柄表）与 `scripts::telemetry`（一进一出埋点）
>   —— 两档各写一份的话，迟早出现"换个档位，诊断就少一个字段"这种最难察觉的漂移。
> - **SP-72 ✅ 已落地**：`ada_ts_engine::NetPolicy`（超时 + 域名白名单）进了运行时的构造路径，
>   并在 fetch 原生里**先于请求**生效（白名单之外的域名连连接都不建）。匹配口径是
>   "精确域名或**带点的子域后缀**"——不带点的后缀匹配会把 `evilexample.com` 放进白名单，
>   那是这类实现最常见的漏洞，用例专门钉着它。配置来源是运维环境变量
>   （`A_DA_PLUGIN_NET_TIMEOUT_SECS` / `A_DA_PLUGIN_NET_ALLOW`，逗号分隔、空=不限域名），
>   因为它是个**部署决定**；默认值（30s / 不限域名）本身就是安全的那一端，
>   所以"没配"不等于"没护栏"。界面/配置文件的可视化留给后续。
> - **SP-74 ◐ 大部分完成**：两处界面改动，都是"让已经存在的数据看得见"——
>   ① **插件卡渲染 `PluginItem.diagnostics`**（`PluginDiagnostics.tsx`）：数据从 P0 起就被扫描端
>   如实填好了（"已停用 / 本页工具清单是文本推断的"），但界面一处都没渲染，
>   于是插件卡永远显示不出问题。`info` 也显示：只显示 warn/error 会让"正常"与"没被检查过"
>   长得一样（缺席可见）。P0 那条"脚本执行面尚未接线"的 info 已在 P1 接线后删除——
>   它当时是真的，接线之后就是对已经生效的插件喊"不生效"（日志侧的
>   `plugin.skipped{execution_not_wired}` 同批删除，只留 `disabled`）。
>   ② **诊断日志按来源聚合**（`DebugPanel`）：每个来源一条 chip，
>   显示**条数 + 最近的点位**，点一下即按它过滤（复用既有的来源过滤状态，不引入第二套交互）。
>   点位名本身就说明问题类型：`hook.replace_rejected` 是被重校验拒了、
>   `hook.finished{timed_out}` 是撞预算、`plugin.skipped` 是压根没装上。
>   未做：跨会话的持久化汇总、按点位的二次分组（现在的聚合只到"来源"这一层）。
> - **source map**：运行时行号仍是转译后 JS 的座标。要真对齐，需要 codegen 产 source map
>   并在错误文案上做一次位置映射（错误此刻是文本，映射要结构化——这也是它单独立一项的理由）。

---

## 3. 关键路径与顺序约束

```
P0 埋点地基 ──→ P1 脚本宿主 ──┬─→ P1b 完全访问档位 ──┐
                              └─→ P2 统一贡献装配 ──┴─→ P3a 钩子(能拦)+分级 ──→ P3b 钩子(能改) ──→ P4 隔离强化
```

**不能换的顺序（每条都有理由，不是偏好）**

1. **P0 先于一切**：没有埋点，后面任何"接通了"都无法证明，只会再造一批"看起来装上了"。
2. **P1 内部：SP-12（Boa 预算）先于"取消贯通"**：`terminate` 打不断同步 JS，
   不先把同步段变成"必然返回"，取消与超时都只能靠外部杀进程。
3. **P1 先于 P3**：没有执行面，钩子没有调用方（插件跑不起来，谁来回调点位）。
4. **P2 的装配管线先于 SP-55 的 `afterPluginToggle` 回执**：否则那个回执没有内容可报（空转）。
5. **SP-54 订阅分级与 SP-50 同批**：分级晚落地一天，第三方就多一天"能拒绝自己被停用"的窗口。
6. **SP-61 必须与 SP-60 同批**：改写不带重校验 = 直接在审批闸门上开洞。

**可并行**：P1b 与 P2 相互独立；SP-73 不与任何任务耦合；SP-74 依赖 P0，可随时插入。

---

## 4. 验收门（每个阶段都要过）

**仓库既有门（一条都不能跳）**

```bash
bun run typecheck                        # 门一：desktop-ui 的 tsc --noEmit
cargo test --workspace -- --test-threads=1   # 门二：必须串行（有共享全局态的用例）
cargo xtask verify-spec                  # 声明门
cargo xtask verify-wiring                # 接线门
cargo xtask verify-docs                  # 文档时效门
bun run verify:archive                   # 归档门
```

**跑门的环境纪律（本机踩过的坑）**

- `CARGO_BUILD_JOBS=2`（或在 `--` 之前加 `-j 2`）：本机页面文件上限 2GB，默认并行度会
  `os error 1455` / `link.exe 1102`；这是环境限制，不是代码缺陷。
- 集成测试必须显式注入临时应用目录（`AgentStore::with_home` / `PluginManager::with_home` /
  `SkillManager::with_home`），**不许 `std::env::set_var`**；跑完门二比对 `~/.a-da/config.json`
  的 mtime 没变（历史真事故：一次测试把用户配置里的激活供应商改了）。
- Electron 相关构建需要 Node 在 PATH 上（本机在 `D:\tools\nodes\24.10.0`），
  否则 `electron:pack` 会在 `npm --prefix desktop-ui run build` 那步报 `command not found`。

**本计划新增的门（随阶段引入）**

| 门 | 内容 | 引入阶段 |
|---|---|---|
| check K | 每个诊断事件变体必须有生产发射者；**新族不许进 `UNEMITTED_EVENT_ALLOW`** | P0 |
| check L | 每个能力位必须有生产消费者（顺带修 `capabilities.streaming`） | P0 |
| 成对埋点契约测试 | `HookEntered` ↔ 恰好一个终结事件；缺席可见 | P0/P3a |
| 执行面冒烟 | 插件工具被调用并回执 `success`；`while(true){}` → `Timeout` | P1 |
| 档位用例 | 切回 `auto` 后越界被拒；`~/.a-da` 任何档位都拒；未知档位值落到更严档 | P1b |
| 提示词逐字节回归 | 与既有输出完全一致 | P2 |
| 改写安全用例 | 改写后审批重触发；越界被拦；风险不降级 | P3b |
| 权限裁剪用例 | 未声明 `net` 拿不到 `fetch` | P4 |

---

## 5. 任务清单总表

| 阶段 | 任务 | 依赖 | 大小 | 状态 |
|---|---|---|---|---|
| P0 | SP-01 `TraceCtx` / `Origin` + 事件信封 | — | M | ✅ 已完成 |
| P0 | SP-02 引擎填 `turn_id` / `step_index` / `origin` | SP-01 | M | ✅ 已完成 |
| P0 | SP-03 诊断事件族 | SP-01 | L | ✅ 核心已落地（单变体 + 点位词汇表，3 个引擎点位） |
| P0 | SP-04 节点侧埋点发射 | SP-03 | M | ✅ 已完成（**扫描类在 RPC 用户动作点发**，见下方口径说明；脚本调用随 P1） |
| P0 | SP-05 诊断投影 + 日志桥 + 截断脱敏 | SP-03 | M | ✅ 已完成（唯一投影点 `server/diag.rs`；内容 200 字符、`data` 4KB 双重上限） |
| P0 | SP-06 诊断面板结构化 + `origin` 过滤 | SP-05 | M | ✅ 已完成（含一条此前**缺失的读出路径**：服务端诊断缓冲原先根本到不了面板） |
| P0 | SP-07 门禁 check K / check L | SP-03 | M | ✅ 已完成（含红能力验证） |
| P0 | SP-08 埋点契约测试（含缺席可见） | SP-03/04 | M | ✅ 已完成（引擎侧 2 条 + 扫描侧 2 条，缺席可见两条都钉住了；钩子/脚本那部分随 P3/P1） |
| P1 | SP-10 `ScriptHost` 端口 | — | M | ✅ 已完成（含生产实现 `TsScriptHost`：端口零实现会被 check D 判红） |
| P1 | SP-11 `HostCtx`（预算 / 取消 / 埋点）+ 超时收敛 | SP-10 | M | ◐ 预算收敛与诊断出口已落地（`ScriptCtx` 三件套 + `script.invoked/finished`）；"三套超时收敛"待 SP-15 把沙箱与引擎的超时并到一处 |
| P1 | SP-12 Boa `RuntimeLimits` 预算化 | — | M | ✅ 已完成（含两个真 bug 修复：行级转译器补分号、`CallAsyncFunction` 吞错误） |
| P1 | SP-13 环境注入可裁剪（最小版） | — | M | ✅ 已完成（`needs` 门禁：网络默认关 + 原生桥同裁；fs 读写分离留 P4） |
| P1 | SP-14 stdout/stderr 结构化捕获 | SP-11 | S | ✅ 已完成（`ConsoleMode` + 每运行时捕获 + 截断标注 + 按插件归属；进回执 `details.console` 与 `script.finished` 字节数） |
| P1 | SP-15 `PluginTool` 适配器 + 回执唯一转换 | SP-10 | M | ✅ 已完成（`scripts/plugin_tool.rs`：描述符取自注册表、回执唯一转换点、默认受超时约束） |
| P1 | SP-16 装配期收集插件工具进 catalog | SP-15 | M | ✅ 已完成（`scripts/assembly.rs` + `HostOptions.extra_tools`；同名插件胜出并产替换审计） |
| P1 | SP-17 transpile 缓存 + 预热 | SP-10 | S | ✅ 已完成（内容寻址 + 有界 + 失败不缓存；进程内，见下方说明） |
| P1 | SP-18 `plugin.validate` RPC + spec 登记 | SP-15 | M | ✅ 已完成（只读自检：转译 / 装载 / 工具名在不在注册表 / 权限与预算 + 人话问题清单） |
| P1 | SP-19 `ScriptHost` 契约测试 | SP-10 | M | ✅ 已完成（`agent-conformance` 通用契约 + 实现侧接入） |
| P1 | SP-20 执行面端到端冒烟 | SP-16/12 | M | ✅ 已完成（集成用例：磁盘插件 → 装配 → 模型调用 → 沙箱执行 → 卡片 `done`） |
| P1b | SP-30 `ApprovalMode::Full` + 协议面 | — | S | ✅ 已完成（枚举 + `client-ts` 联合类型 + spec 描述） |
| P1b | SP-31 解析 + 兜底收紧 + 用例 | SP-30 | S | ✅ 已完成（认不出的档位倒向更严的 `Ask` + 警告日志） |
| P1b | SP-32 权限档位两层 | SP-30/13 | M | ✅ 已完成（规则 0 覆盖 `Full` + `permission_profile_of` 唯一映射） |
| P1b | SP-33 `Scope` 按档装配 + 红线 | SP-32 | M | ✅ 已完成（`ApprovalAwareScope`：越界放行 + `~/.a-da` 红线 + `id()` 不变） |
| P1b | SP-34 插件 `Deny` 开关 | SP-32 | S | ⏳ 等 P3a（没有 HookTable 就没有可开关的东西） |
| P1b | SP-35 界面第四档 + 确认 + 常驻指示 | SP-30 | M | ✅ 已完成（选项 + 开启确认弹窗 + 输入区与标题栏两处常驻指示 + 一键撤回） |
| P1b | SP-36 自动放行埋点 + 越界留痕 | SP-32 | S | ◐ 部分：`approval.skipped` 已发（没问就放行的审计）；越界留痕目前是 `tracing::warn`，界面侧那条需要工具层信号，见下方说明 |
| P2 | SP-40 `Contribution` + 装配管线 | P1 | L | ✅ 已完成（`agent-runtime/src/contributions.rs`：四类贡献 + 校验/排序/产出四项产物；**工具那一段已接进 `agent-host`**） |
| P2 | SP-41 `PromptPlan` 片段化 | SP-40 | L | ✅ 已完成（`build_prompt_plan` + 片段自带分隔符 + 纯拼接；技能逐条成片段） |
| P2 | SP-42 提示词逐字节回归 | SP-41 | M | ✅ 已完成（黄金夹具取自**重构前**输出；已验证"改一字节即红"） |
| P2 | SP-43 `PromptComposed` 埋点 | SP-41 | S | ✅ 已完成（端口加 `prompt_segments()` + 引擎按片段报 id/来源/字节；契约断言"分段之和 == 总长"） |
| P2 | SP-44 插件提示词片段 | SP-41 | S | ✅ 已完成（插件声明 `prompts` → `PromptPlan` 带 `plugin:<id>` 来源；顺带填上 `prompt_extensions` 的**空缝**） |
| P2 | SP-45 `SkillIndex` + 插件技能 | SP-40 | M | ✅ 已完成（三来源合流 + `plugin_id` 归因 + `skill.invalid` 诊断；**决策 4：暂不索引化**，理由见批次说明） |
| P2 | SP-46 受控重装配入口 | SP-40 | M | ✅ 已完成（`reassembly` + 三条活体 + 组合根 `PluginReassembler`；`plugin.setEnabled` 回执带差异，端到端断言"停用后提示词里就没有了"） |
| P3a | SP-50 `HookTable` + 调用顺序 | P2 | L | ✅ 已完成（`agent-node::hooks::HookRuntime`：内置优先→order→id；拦下即定稿；超时即 `Pass`；注册表内部可变以支持启停重装配） |
| P3a | SP-51 前置点位接入引擎 | SP-50 | L | ✅ 已完成（`beforeToolCall` 拦下/结束整轮 + `afterToolCall` 旁注；位置在审批**之前**，安全序见设计 §5.6） |
| P3a | SP-52 成对埋点 | SP-50 | M | ✅ 已完成（`hook.entered` / `hook.finished` / `hook.skipped`；转换点唯一在 `ports::hooks::observation_to_diagnostic`） |
| P3a | SP-53 点位超时 = `Pass` | SP-50 | S | ✅ 已完成（`hookTimeoutMs` 从"只解析"变成真消费者；超时/脚本抛错都**不拦**，留痕说明） |
| P3a | SP-54 订阅分级 + 装配期拒绝越级 | SP-50 | M | ✅ 已完成（`allows(level, trust, action)` 一处判；越级/未实现/重复**逐条拒并产诊断**） |
| P3a | SP-55 管理面点位（插件启停 / 会话创建） | SP-53/54/46 | L | ✅ 已完成（`beforePluginToggle`/`afterPluginToggle`、`beforeThreadCreate`/`afterThreadCreate`；拦下即不写盘、原子性有用例） |
| P3a | SP-56 `capabilities.hooks` 翻 true | SP-52 | S | ✅ 已完成（能力位从声明派生；`CAPABILITY_FIELDS` 登记两个消费者） |
| P3b | SP-60 `Replace` 语义 + 应用点唯一化 | P3a | M | ✅ 已完成（载荷契约 `{action:'replace', args:{...}}`；链式；应用点在引擎一处；`hook.replace` 带改前改后指纹与字节差） |
| P3b | SP-61 改写后重校验（审批读改写后的 args） | SP-60 | L | ✅ 已完成（`engine::hook_guard`：形状/schema/路径沙箱 + 风险单调性；执行用的参数在**改写之后**才组装） |
| P3b | SP-62 风险单调性用例 | SP-61 | M | ✅ 已完成（5 条：审批读新参数 / 降级被拒用回原参数 / 越界被拦 / schema 不符被拒 / 工具名改不动；另加档位对照落在 `agent-adapter`） |
| P3b | SP-63 todo 点位对 | SP-60 | M | ✅ 已完成（`ToolContext.hooks` 把运行时交给工具，点位由**工具自己跑**；`diffTodos` 纯函数含"完成又被改回"单列；改写回到工具内**重新校验**；5 条端到端用例） |
| P4 | SP-70 L3 子进程档 | P1 | L | ✅ 已完成（`runner` 子进程入口 + `SubprocessScriptHost` 实现同一端口；超时即 `kill_on_drop`；档位由 `--script-sandbox` 选，默认仍是进程内。**不做内存限额**——见下方说明） |
| P4 | SP-71 `needs` 完整裁剪 | SP-13 | L | ✅ 已完成（声明面 `// @ada-needs fs,net`；**未声明即不注入**；调用期按插件自己的声明建沙箱；认不出的 token 不授予并产诊断） |
| P4 | SP-72 网络参数可配 | SP-71 | M | ✅ 已完成（`NetPolicy`：超时 + 域名白名单，**策略先于请求**；来源是运维环境变量 `A_DA_PLUGIN_NET_TIMEOUT_SECS` / `A_DA_PLUGIN_NET_ALLOW`，默认值本身即安全档） |
| P4 | SP-73 source map + 结构化转译错误 | — | M | ◐ 部分：**转译错误**已带行列 + 出错行原文 + 插入符；**运行时错误**已带文件名/位置/栈（含 JS 侧 catch 的那条路）。**source map 未做**：转译后 JS 的行号与原 TS 不一一对应，运行时行号仍是"转译后"的座标 |
| P4 | SP-74 诊断面板完善 | P0 | M | ◐ 大部分完成（**插件卡渲染 `PluginItem.diagnostics`**（含 level/hint）+ **诊断日志按来源聚合**（条数 + 最近点位，点一下即按它过滤）；未做：跨会话/跨轮的持久化汇总、按点位的二次分组） |

---

## 6. 风险与回退

**回退原则（贯穿全程）**：每一期都能单独回退——**能力位保持 `false` 就等于对外没承诺**。
不许出现"半接通的中间态对外声明为已支持"（这正是本计划要根治的病）。

| 风险 | 触发形态 | 缓解 |
|---|---|---|
| 埋点本身成为负担/噪音 | 事件量暴涨、日志淹没 | 默认只记元数据 + 采样开关；诊断与 UI 卡片分流（§2.4） |
| 沙箱启动开销拖慢首轮 | 每个插件工具调用都新建实例 | transpile 缓存 + 预热（SP-17）；必要时单轮内复用 |
| Boa 预算误伤正常插件 | 合法插件被 `ScriptBudgetExceeded` | 预算走产品声明可调，默认给宽值 + 埋点调参 |
| 钩子带来每调用一次跨沙箱往返 | 工具卡顿 | 单点位预算 500ms + 无插件时整体跳过（§5 原则 1） |
| `Replace` 被用来绕过审批 | 改写 args 后风险下降 | SP-61 重校验 + SP-62 单调性用例；先只开两个点位 |
| 完全访问被误开或忘记关 | 越界写、横向扩散 | 会话级 + 开启确认 + 常驻指示 + 一键撤回 + 逐次留痕（SP-35/36） |
| 第三方插件锁死系统 | 拒绝停用 / 拒绝建会话 | 订阅分级（SP-54），越级在装配期拒绝 |
| 提示词行为变更 | 模型输出风格漂移 | 逐字节回归（SP-42）+ 变更审查 |
| 决策迟到导致返工 | SP-15/16 按 A 做完又改 B | §1 的阻塞项开工前定；未定的按"建议"值走并记录 |

**关键人员依赖**：SP-12（Boa 预算）、SP-33（`Scope` 按档装配）、SP-61（改写后重校验）
三处是全计划的技术风险最集中处，建议在各自阶段**先写用例再写实现**（红了再做）。

---

## 7. 不做（out of scope）

- **不做 26 个点位全量**：只做设计文档确认的首批（3 个）+ 两个管理面点位对；
  其余点位按"真被调用才加"的原则逐个补，不做一次性铺开。
- **不做通用"执行任意函数"的远程入口**：`plugin.*` 只加自检类方法（§3.8）。
- **不做第二个脚本引擎 / WASM 插件**：`ada-ts-engine` 仍是唯一内核；
  多语言作为未来的 `PluginHost` 实现（契约不变）。
- **不做插件市场、签名与分发**：只保留"受信门槛"的接口位置（决策 12）。
- **不做技能的全文检索/向量化**：保持目录扫描 + frontmatter。
- **不重写审批闸门**：只加一档，既有 `Ask` / `Readonly` 语义与测试不放宽。
- **不动网关与协议分层**：新增方法一律进 ext（L2），不碰 base（L1）。

---

## 8. 与既有文档的关系

| 文档 | 关系 |
|---|---|
| [docs/script-plugin-system-design.md](script-plugin-system-design.md) | 本计划的**依据**（现状分析 / 埋点方案 / 脚本系统 / 贡献模型 / 点位契约 / 完全访问） |
| [docs/agent-base-plan.md](agent-base-plan.md) | **计划口径的权威**；本计划落地时并入其里程碑与逐条处置表 |
| [docs/agent-base-design.md](agent-base-design.md) | **设计口径的权威**；本计划不得与其 INV 冲突 |
| [docs/unfinished-features.md](unfinished-features.md) | 每完成一项（如"钩子点位 = 0"、"插件执行面"）顺手划掉对应条目 |
| [docs/plugin-sdk/v1.md](plugin-sdk/v1.md) | 插件契约；P1 完成后其 §5/§7 的描述才第一次真正成立 |
