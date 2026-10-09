# AGENT BASE 接线收口开发计划（W0–W6）

> **依据**：[docs/agent-base-design.md](agent-base-design.md) v0.2 的不变量表（INV-1…INV-12）、
> [docs/agent-base-plan.md](agent-base-plan.md) 的里程碑口径、[AGENTS.md](../AGENTS.md) 的三条硬规矩。
> **触发**：2026-10-09 的代码级缺口审计（全仓 grep + `file:line` 取证 + 实跑门禁）。
> **与既有文档的关系**：本文**不改写** `agent-base-plan.md` 的历史记录；它只针对"计划声称已完成、
> 但**运行路径上未接线**"的那批缺口，给出可执行的收口计划。冲突时以本文为准，并回写 `agent-base-plan.md`。
>
> **状态列取值**：`未开始` / `进行中` / `已完成` / `已放弃（须写理由）`。
> 每完成一个任务，**必须**同步改本文件的状态列与 [docs/unfinished-features.md](unfinished-features.md)。

---

## 0. 本文口径：为什么需要这份计划

2026-10-09 实测：`cargo build` / `cargo test --workspace`（149 passed）/ `bun run typecheck` /
`bun run verify:archive` **四道门全绿**。但门禁绿的是**测试域**；真实产品运行路径是另一回事。

一句话结论：

> **基座（`agent-base` 引擎 + `agent-conformance`）在测试域完整自洽，但真实产品
> （`products/ada-coding` → `WsHostServer` → legacy `run_agent_loop`）没有跑基座。**
> 于是基座的能力（审批闸门、取消贯穿、`Termination`、`ToolCatalog` 装配一次）**只活在测试里**，
> 而 legacy 侧对应的能力又是**死代码**。这正是 AGENTS.md 反复警告的
> "看起来装上了、其实没接线"——只不过这次壳子换成了新基座。

三条**已核实**的核心事实（本文所有 P0 都从这三条派生）：

| # | 事实 | 证据 |
|---|---|---|
| A | **产品入口不起基座引擎** | [products/ada-coding/src/main.rs:75](../products/ada-coding/src/main.rs#L75) 只 `WsHostServer::bind`；`AgentSpec` 仅在 `#[cfg(test)]`（[:121-137](../products/ada-coding/src/main.rs#L121-L137)） |
| B | **真实路径跑 legacy 循环** | [dispatch.rs:534](../crates/agent-core/src/server/dispatch.rs#L534)、[:1476](../crates/agent-core/src/server/dispatch.rs#L1476)、[:1562](../crates/agent-core/src/server/dispatch.rs#L1562) 调 [agent_loop.rs:39](../crates/agent-core/src/runner/agent_loop.rs#L39) 的 `run_agent_loop` |
| C | **`AgentRuntime` 只在测试与演示品里跑** | 调用者 = 自身单测 + [inv1](../crates/agent-conformance/src/invariants/inv1_single_engine.rs#L47)/[inv2](../crates/agent-conformance/src/invariants/inv2_no_global_state.rs#L80)/[inv8](../crates/agent-conformance/src/invariants/inv8_cancellation_penetration.rs#L57) + [ada-skeleton](../products/ada-skeleton/src/main.rs#L66)（`ScriptedModelClient`） |

由此，**INV-1「单一引擎」在运行时是两份实现并存**。`verify-archive` 的"引擎入口恰好一处"
断言查的是 `fn run_agent_loop` 这个 legacy 符号，所以它绿了也不代表新引擎在跑。

---

## 1. 目标与完成判据（DoD）

**目标**：让产品运行时**真的**跑基座引擎；清掉"声明 ↔ 实现"的漂移；让文档、门禁与代码三者对齐。

**完成判据（必须全部可机械执行，不许"人工确认"）**：

```bash
cargo xtask verify            # 全绿（= cargo test + verify-archive）
cargo xtask verify-spec       # 【新增】产品声明 ↔ 实现一致（W0-T1）
cargo xtask verify-wiring     # 【新增】接线结构审计 A/B/C/D（W0-T3…T6）
cargo xtask verify-docs       # 【新增】文档时效性（W0-T7）
cargo xtask compat            # 【新增】跨产品 base 协议一致（W0-T2）
cargo xtask verify-archive    # 全绿
cargo test --workspace -- --test-threads=1
bun run typecheck
```

> 命令名以**实际实现**为准（W0 落地后修正）：计划早期写的 `verify-spec --all` /
> `compat --all` 里的 `--all` 参数没有实现——当前每个产品/每个 spec 都无条件全扫，
> 不需要 `--all`。**改命令名必须同步改本节与 §6**，否则就是文档漂移。

外加一条**运行时**判据（它才是本次的重点）：

> `products/ada-coding` 的 `cargo run` 冒烟脚本里，**审批、取消、工具执行三条链路全部走
> `agent-base` 的端口实现**，且 legacy `run_agent_loop` / `executor::execute_tool_call_extended`
> **已从仓库删除**（`verify-archive` 增断言 9：主干不存在 `fn run_agent_loop`）。
>
> ⚠️ 断言 9 **现在不能加**：当前 `verify-archive` 断言 3 要求"`fn run_agent_loop` 恰好一处"，
> 与断言 9 直接冲突。W3-T4 删 legacy 时**同时**把断言 3 换成断言 9。

---

## 2. 缺口总账（P0/P1/P2 → 任务映射）

> 每一行都必须有归属任务；**没有归属的缺口不许出现在本表**（否则就是计划漂移）。

| 级别 | 缺口 | 证据 | 归属 |
|---|---|---|---|
| P0-1 | 产品运行路径仍是 legacy 引擎；`AgentRuntime` 只被测试用 | 事实 A/B/C | **✅ W3 完成**：legacy 主循环已删除，产品运行时**只跑** `AgentRuntime::run_turn`；`verify-archive` 断言主干无 `fn run_agent_loop` |
| P0-2 | 6 个端口**无生产实现**（Scope/SessionStore/ApprovalGate/EventSink/PromptSource/CancelToken） | `impl … for` 只有 Clock/AppHome/ModelClient/ToolCatalog | **W1 ✅ 已清零（10/10 端口有生产实现）** |
| P0-3 | 审批闸门在真实路径**不存在**；三档策略无消费者 | [guard.rs:43](../crates/agent-core/src/approval/guard.rs#L43) 唯一调用者在 [mod.rs:23-92](../crates/agent-core/src/approval/mod.rs#L23-L92) `#[cfg(test)]`；[manager.rs:18](../crates/agent-core/src/approval/manager.rs#L18) `register_waiter` 无生产调用点；`ApprovalMode` 只写不读 | W1-T6 ✅（实现+契约）/ **W3-T2 ✅ 执行侧已接入真引擎**（`EngineInjection` 强制共用 waiter 表，UI 的批准能落到闸门）/ **⚠️ 界面提示通道属 W3-T3**（见 P1-14） |
| P0-4 | 取消链到子进程/子智能体全断（含**假接线**） | [command/run.rs:83-86](../crates/agent-toolkit/src/command/run.rs#L83-L86) `abort_tx` 只在执行前发一次；[executor.rs:149](../crates/agent-core/src/runner/executor.rs#L149) 传 `None`；[:189](../crates/agent-core/src/runner/executor.rs#L189) `abort_rx: None` | **✅ W4 完成**：主循环侧 W4-T1/T2/T4（执行中取消真杀进程树 + 状态结构化 + INV-8 强化）；子智能体侧 W4-T3/T6（取消连穿三层：父会话 → 子智能体引擎 → 命令进程树） |
| P0-5 | 三个工具包工厂零调用者 → 13 个 `Tool` 实现是死代码 | `fs/mod.rs:20` / `core/mod.rs:13` / `command/mod.rs:10` | **W2-T1 ✅ 已接线**（`toolkits.rs` 真源 + `ProductBuilder::with_declared_toolkits`） |
| P1-1 | **第 6 张硬编码名单**：`PLUGIN_BUILTIN_CATALOG` 与真源不符 | [dispatch.rs:898-922](../crates/agent-core/src/server/dispatch.rs#L898-L922) 写死 21 个（广告 12 个不存在、漏 13 个真实） | **W2-T5 ✅ 已消除**（改为注册表投影；结构断言 + 行为断言双守门） |
| P1-2 | 2 个孤儿协议方法 + 无覆盖性守门测试 | `WORKSPACE_SET` / `CONFIG_UPDATE` 无 dispatch 臂（grep 已证） | **W5-T2 ✅ 已处置**（两者**无实现、前端也从未调用** → 按 R3 删声明：`spec/proto` + `methods.rs` + `client-ts` 三处同步；覆盖性守门由 W0-T3 提供） |
| P1-3 | 插件钩子**整套 25 点位 0 接线**；8 个能力开关无消费者 | `hooks: false`（[dto.rs:79](../crates/agent-proto/src/dto.rs#L79)）；`approval-guard` 插件 [builtins.rs:66-71](../crates/agent-core/src/plugins/builtins.rs#L66-L71) `tools: &[]` | W5-T5（先如实降级）+ 附录 B |
| P1-4 | 子智能体白名单硬编码，含 8 个不存在的工具名；`NEVER_FOR_SUBAGENT` 5 个里 4 个是幽灵 | `subagents/builtins.rs`、`subagents/runner.rs:28-34` | **W2-T6 ✅ 已修复**（删幽灵名 + 新增"名字必须真实存在"守门断言） |
| P1-5 | 插件 `is_write` 三处残留；第三方写工具被当只读 | `plugins/types.rs:44`；`plugins/builtins.rs:10`；`plugins/manager.rs:363` | **W2-T7 ✅ 已修复**（死字段删除；第三方改为按描述符派生、未知失败安全当写） |
| P1-6 | `run_background` / `read_url_content` 有描述符无执行器（`todo`/`finish` 已由 W2-T1 转绿） | `registry.rs:95/205` | **W2-T3 ✅ 已处置**（后台任务族补实现；`read_url_content` 删声明并登记 M6） |
| P1-7 | `AgentSpec` 的 `toolkits`/`capabilities`/`identity` 解析后无人读 | [builder.rs:113-117](../crates/agent-runtime/src/builder.rs#L113-L117) 只用 `policies` | **W5-T4 ✅ 已修复**（`toolkits`→工具装配 + 子智能体装配；`identity`→系统提示词；`capabilities.subagents`→是否装配委派工具；`capabilities.images`→是否接受图片输入。`verify-spec` 已全绿） |
| P1-8 | `xtask verify-spec` / `compat` / `gen` 三个子命令**不存在**；golden 夹具 / `spec/proto/README.md` / `docs/protocol/*.md` 缺失 | [xtask/main.rs:15-35](../tools/xtask/src/main.rs#L15-L35)；`crates/agent-conformance/golden/` 不存在 | W0-T1/T2 + W3-T5 + W5-T1/T3 |
| P1-9 | **toolkit 声明与实现不一致**：`ada-coding` 声明 `patch` 但 `crates/agent-toolkit/src` 下无该模块；`decision` 有模块却未在任何 spec 里声明 | W0-T1 实测（`cargo xtask verify-spec` 第 1 条违约） | **W2-T1 ✅ 已修复**（`patch` 删声明、`decision` 补声明、判定改查 `TOOLKIT_NAMES` 真源） |
| P1-10 | **插件工具不是一等 `Tool`**：`git_*` / `project_inspect` / `code_outline` / `run_tests` 的实现只存在于 `execute_builtin_plugin_tool` 的字符串分派里，虽有 `ToolDescriptor` 却进不了 catalog | W2-T2 实测（7 个描述符只靠 legacy 分派才算"可达"） | **W2-T2 ✅ 已修复**（逻辑搬进 `agent-toolkit::plugin_tools` + `git`/`project` 工具包；legacy 侧改为委派） |
| P1-11 | 🔴 **`code_outline` 静默失效**：它用 `starts_with("pub fn ")` 匹配 `read_file` 的输出，而后者每行都带「行号 + 竖线 + 空格」前缀 → **永远提取不到任何签名**，每次都返回"未提取到显著大纲" | W2-T2 搬运时补单测才暴露 | **W2-T2 ✅ 已修复**（剥掉行号前缀；`test_code_outline_extracts_rust_signatures` 钉住） |
| P1-12 | 🔴 **引擎步数预算 off-by-one**：`max_steps=1` 报 `steps_taken=2`——被预算拒绝的那次尝试也算进了"已执行步数" | [runtime.rs:137-144](../crates/agent-base/src/engine/runtime.rs#L137-L144) 先自增后判断 | **W3-T5 ✅ 已修复**（先判预算后自增；补 `steps_taken == 1` 断言） |
| P1-13 | 🔴 **`provider.delete` 用例偶发红**：删共享配置里的 `providers[0]`，当列表只剩 1 个时按规则被拒 | `agent-core/src/lib.rs` 的 `test_dispatcher_provider_management` | **W3-T5 ✅ 已修复**（改为自造专用供应商再删；连跑 3 次 + 全量跑 2 次稳定绿） |
| P1-14 | 🔴 **新引擎的审批提示没有界面通道**：引擎会发 `ApprovalRequested` 领域事件，但 legacy 事件集（`AgentLoopEvent`）**没有对应变体**，桥接时被丢弃 → 走 `A_DA_ENGINE=runtime` 且处于"询问档"时，工具调用会**静默等满 300s 超时**，界面不显示任何批准按钮 | W3-T2 实测（[engine_bridge.rs](../crates/agent-core/src/runner/engine_bridge.rs) 的差集分支） | **W3-T3 ✅ 已修复**（新增 `AgentLoopEvent::ApprovalRequested` + `store.set_tool_waiting_approval`；端到端断言：审批请求到达界面通道且批准后闸门被唤醒） |
| P1-15 | 🔴 **子智能体委派能力随 legacy 消失**：`invoke_subagent` 只在 legacy 字符串分派里，新引擎的 catalog **没有**它 → 主循环再也无法委派子智能体 | W3-T4 删除时实测（`executor.rs` 的 `invoke_subagent` 分支被删） | **W4-T5 ✅ 已修复**（描述符进注册表 + `agent-core` 的 `impl Tool` + `agent-host` 按 `capabilities.subagents` 注入 catalog；产品冒烟：24 个工具） |
| P1-16 | 🔴 **第二份多轮引擎**：`subagents/runner.rs` 的独立多轮循环 + legacy `execute_tool_call`；INV-1 在子智能体侧仍未成立 | W3-T4 测绘时发现（删 `run_agent_loop` 后它成了唯一残留） | **W4-T6 ✅ 已修复**（并入 `AgentRuntime::run_turn`；隔离性由 5 个端口实现保证；`verify-archive` 增断言"主干不存在第二份多轮引擎"） |
| P2-1 | 3 个 dispatch 桩：`SUBAGENT_RESUME` / `STATS_PROMPT_CHARS` / `WORKSPACE_RESCAN` | dispatch.rs 三处桩 | **✅ W6-T1/T2 已处置**：`subagent.resume` 删声明（前端"恢复执行"死按钮一并删）；`workspace.rescan` 删声明；`stats.promptChars` **真实现**（实测提示词与工具规格字符数，不再返回 1200/800） |
| P2-2 | CLI `a-da run` 未实现 | [src-tauri/src/main.rs:239-248](../src-tauri/src/main.rs#L239-L248) | W6-T3 |
| P2-3 | 三处 `revert_*` 返回值被丢弃；`THREAD_EDIT_AND_RESEND` 吞错 + 丢事件流 + 不可 abort | dispatch.rs 三处 revert + resend 臂 | **✅ W6-T4 已修复**：回执带回恢复/删除清单；resend 与 `thread.start` 共用**同一条执行泵**（事件写 store + 广播、错误进 `Error` 事件、`abort_tx` 注册可停） |
| P2-4 | 合规套件两处空壳（INV-7 空函数、`ports/model.rs` 自比）；"8 端口"名不副实 | `inv7:5-8`；`ports/model.rs:43-44` | W6-T5 |
| P2-5 | 前端重连无退避/无上限/竞态；`onclose` 不清 `pendingRequests` | `tauri-ui/src/client/ws-client.ts:290-312` | W6-T6 |
| P2-6 | README 正文仍是 TS/GPUIX 时代；`unfinished-features.md` / `feature-catalog.md` 过期 | README 656 行 + 另两份文档共 60 条归档引用 | **✅ W6-T7 已修复**：三份文档按当前实现重写，`verify-docs` **60 → 0** |
| **P0-6** | 🔴 **`ask_user` 在新引擎上不可用**：工具在 catalog 里、UI 卡片组件也在，但链路永远不触发 | `AskUserTool::execute` 直接返回错误（`agent-toolkit/src/core/ask_user.rs`）；`AgentEventBody::QuestionAsked` **全仓无发射者**；唯一会注册 waiter 的 legacy `execute_ask_user` 已随 S1 删除 | **S1a**（提问端口化 + 真实现；门禁已用 `UNEMITTED_EVENT_ALLOW` 钉住） |
| **P1-17** | 🔴 **子智能体生命周期未上报**：`SubagentStarted` / `SubagentFinished` 定义了却无发射者，`engine_bridge` 把两者映射为 `None` | `crates/agent-base/src/domain/event.rs:32-33`；`engine_bridge.rs:94-95`；进度通道 `SubagentStepUpdate` 在 `InvokeSubagentTool` 里被置 `None`（`subagents/tool.rs:167`） | **S6**（交互平台一并解决：进度/生命周期上报是 agent 间交互的一部分） |
| P2-7 | `runner/` 遗留 991 行死代码，被 `pub use` 遮蔽 `dead_code` 检测 | `runner/{builtin_tools,executor,prompt}.rs` = 436+253+302 行 | **✅ S1 已删除** + 防复活断言（`test_legacy_runner_residue_must_not_come_back`） |
| **P0-7** | 🔴 **节点层依赖 UI 投影**：审批闸门与委派工具直接读 `AgentStore`，而 `AgentStore.config` 的类型是 `agent_proto::ConfigSnapshot`（**线上 DTO**）——节点行为依据"发给界面的 JSON 形状"做决定 | `approval/gate.rs`（`store.config.approval`）、`subagents/tool.rs`（`store.provider`） | **✅ S2 已修**：`NodeConfigSource` 端口 + 桥接层唯一实现；新增 `verify-wiring` check H 防复发 |
| **P1-18** | **委派语义未抽象**：`invoke_subagent` 直接调用 `run_subagent`（进程内），网关接入时会变成**第二套委派机制** | `subagents/tool.rs` 里内联的派活逻辑（profile 解析 / provider 校验 / 取消转发 / `run_subagent`） | **✅ S3 已修**：`AgentBus` 端口 + `LocalAgentBus`（行为零变化，原有测试未改）；S6 换 `GatewayAgentBus` 实现即可 |

---

## 3. 防漂移纪律（硬规则，先读再做）

> 本节是本文存在的理由。**违反其中任一条的改动，即使测试全绿也不算完成。**

### R1 红先绿后（Test-First for Gaps）
每个任务**第一步**是先写一条**会失败**的守门断言（单测 / conformance / xtask 检查），
把"缺口"变成红灯，再实现，再转绿。**不许**先改实现再补测试——那样无法证明缺口真的被堵上。

### R2 一次只动一个真源
同一份事实只允许有一个写入点：
- 工具语义 → 只改 `crates/agent-toolkit/src/registry.rs` 的 `ToolDescriptor`；
- 协议方法 → 只改 `spec/proto/*.json`，`methods.rs` / `client-ts/*.ts` / dispatch 臂都是**产物**；
- 审批策略 → 只改 `agent-adapter` 的纯策略函数；
- 能力位 → 只改 `products/<id>/agent.spec.json`。

**看到第二处需要同步改，说明该处是缺陷（第 N 张名单），必须消除而不是同步。**

### R3 补实现或删声明（二选一，不许留半吊子）
每个缺口只能有两种终局：**真实现**，或**从声明里删除**（并从能力位如实降级）。
"声明留着、实现空着、测试绕开"是禁止状态。参照 [agent-base-plan.md §1.3](agent-base-plan.md) 的处置表口径。

### R4 不许放宽断言
合规套件与 conformance 的断言**只允许加强**。确需放宽（例如语义确实变了）必须：
1. 在本文 §10「断言变更记录」追加一行（日期 / 断言 / 为什么 / 谁批准）；
2. 在 PR 描述里显式引用该行。
**删掉一条 assert 就通过的情况一律视为作弊。**

### R5 写作用域单一归属
每个任务在 §5 里声明 `write scope`（工作区相对路径前缀）。**同一时刻只允许一个任务写同一文件。**
拿不准就把改动拆成两个任务，排依赖，而不是并行改。

### R6 搬运用兼容 shim，调用点不动
把实现从 `agent-core` 搬去 `agent-adapter` 时，**必须**在原位置留 `pub use` 兼容 shim
（沿用 [agent-core/src/tools/mod.rs](../crates/agent-core/src/tools/mod.rs) 的既有模式），
让调用点零改动。shim 的删除单独排一个任务，**且必须在所有调用点迁移完成后**。

### R7 前端零改动（事件与负载兼容）
W3 切引擎时**前端 `tauri-ui` 不许改**（除 W6-T6 明确列出的重连项）。事件名与负载形状
必须由投影层适配；任何"顺便改一下前端"的需求都要新开任务并说明理由。

### R8 每个里程碑独立可交付、可回退
一个里程碑 = 一组可以单独合并、单独回滚的提交。W3（切引擎）**必须**用运行时开关
（`A_DA_ENGINE=base|legacy`）保证双跑可切换，直到 W3-T4 删除 legacy。

### R9 提交粒度
一个任务一个提交，message 前缀带任务 ID，例如 `feat(W1-T2a): 搬运 session/checkpoint 到 agent-adapter 并留兼容 shim`。
一个提交里出现两个任务 ID 说明拆分失败。

### R10 完成即登记
任务完成时**同一个提交**里更新：本文件状态列、[unfinished-features.md](unfinished-features.md) 对应条目。
"改完功能不更新文档"视为未完成。

---

## 4. 里程碑总览

| # | 里程碑 | 人日 | 关键产出 | 出口判据 |
|---|---|---|---|---|
| **W0** | 闸与红例先行（不碰实现） | 3.5 | 4 条新守门测试（预期红）+ `xtask verify-spec`/`compat` 骨架 + 文档漂移断言 | ✅ **已完成**：红灯清单与本文 §2 总账逐条对应；当时基线 `verify-wiring` 37 / `verify-spec` 7 / `verify-docs` 60 —— **现已全部清零**（W2–W6） |
| **W1** | 生产端口实现（adapter 层） | 10 | Scope / SessionStore / PromptSource / CancelToken / EventSink / ApprovalGate 的真实实现 + 对应 conformance | ✅ **已完成**：6 个端口全部有生产实现（`verify-wiring` 端口违约 0） |
| **W2** | 工具端口化与名单消除 | 10 | 23 个 `ToolDescriptor` 全部可达且**全部由工具包工厂提供**；第 6 张名单消除；白名单/`is_write` 收敛 | ✅ **已完成**：`verify-wiring` 工具违约 0（23/23 来自工具包）；目录派生自注册表；幽灵工具名清零 |
| **W3** | 产品切引擎（**最高风险**） | 12.5 | `agent-host` + 3 处调用点切换 + 事件投影 + 删 legacy + golden 回放 | ✅ **已完成**：legacy 主循环已删；主干只剩一份引擎（`verify-archive` 断言）；产品运行时跑 `AgentRuntime` |
| **W4** | 取消贯穿收口 | 4 | 修假接线；子进程/子智能体真取消；INV-8 强化 | ✅ **已完成**：执行中取消真杀进程树；取消连穿三层（父会话→子智能体引擎→进程树）；INV-8 强化为"执行中穿透" |
| **W5** | 协议与能力位对齐 | 7 | `gen` 或降级为"手写+校验"；孤儿方法处置；能力位由声明驱动；`FailDirection` 消费 | ✅ **已完成 5/5**：协议生成**正式降级**为手写+三对双向校验；孤儿方法清零；能力位接上真实消费者；`FailDirection` 参与裁决 |
| **W6** | 桩补齐与文档收口 | 10.5 | 3 个桩、CLI、`revert_*` 回传、空壳断言、前端重连、四份文档 | ✅ **已完成 7/7**：桩与空壳断言全清；CLI `run` 真执行；执行泵去重；前端重连收口；`verify-docs` 60 → 0 |
| | **合计** | **57.5** | | |

依赖关系：`W0 → W1 → W2 → W3 → W4`；`W5` 依赖 `W0`（可并行于 W2/W3，但 `W5-T4` 依赖 `W1`）；
`W6` 中 T1/T2 依赖 `W3`，T7 依赖全部。

---

## 5. 逐里程碑任务清单

### W0 闸与红例先行（3.5 人日）—— **已完成（2026-10-09）**

> **本里程碑不修任何缺口**，只把缺口变成**会失败的门**。这样后续每个任务都有"转绿"的客观证据。
>
> ⚠️ **实施口径偏差（已记录，见 §13）**：T3–T6 四项结构审计**没有写成 `#[test]`**，
> 而是合并为独立的 `cargo xtask verify-wiring` 子命令；T7 从 `verify-archive.ts`
> 移到独立的 `cargo xtask verify-docs`。理由：写进主门禁会让 `cargo xtask verify`
> 长期变红、掩盖其它信号，违反 R8。**断言本身没有削弱**（R4）。

| ID | 任务 | 现状证据 | 产出（write scope） | 守门命令与红例 | 依赖 | 人日 | 状态 |
|---|---|---|---|---|---|---|---|
| W0-T1 | `xtask verify-spec`：产品声明解析 + toolkit 模块存在性 + 字段有生产消费者 | `xtask` 无此子命令 | `tools/xtask/src/gates.rs` | `cargo xtask verify-spec` → **红 7 条** | — | 0.5 | ✅ 已完成 |
| W0-T2 | `xtask compat`：base 协议稳定哈希 + base ∪ ext 无交集 | 子命令不存在 | `tools/xtask/src/gates.rs` | `cargo xtask compat` → **绿**（base 24 方法 / ext 52） | — | 0.5 | ✅ 已完成 |
| W0-T3 | 审计 A：`ALL_METHODS` 每个方法都有 dispatch 臂 | `WORKSPACE_SET`/`CONFIG_UPDATE` 是孤儿 | `tools/xtask/src/gates.rs` | `cargo xtask verify-wiring` → **红 2 条**（76 方法 / 74 臂） | — | 0.5 | ✅ 已完成 |
| W0-T4 | 审计 B：`PLUGIN_BUILTIN_CATALOG` 与注册表同集合 | 两者严重不符 | `tools/xtask/src/gates.rs` | **红 25 条**（多 12 / 漏 13） | — | 0.5 | ✅ 已完成 |
| W0-T5 | 审计 C：每个 descriptor 都有可达执行路径 | `run_background`/`read_url_content`/`todo`/`finish` 不可达 | `tools/xtask/src/gates.rs` | **红 4 条**（22 描述符 / 18 可达） | — | 0.5 | ✅ 已完成 |
| W0-T6 | 审计 D：每个端口都有**生产**实现（排除 testing double） | 6 个端口无生产实现 | `tools/xtask/src/gates.rs` | **红 6 条**（缺 Scope/SessionStore/PromptSource/CancelToken/EventSink/ApprovalGate） | — | 0.5 | ✅ 已完成 |
| W0-T7 | `xtask verify-docs`：主干 `*.md` 不得把归档布局当现行路径 | README 20-656 行整段过期 | `tools/xtask/src/gates.rs` | **红 60 条 / 3 个文件**（README 41、unfinished-features 13、feature-catalog 6） | — | 0.5 | ✅ 已完成 |

**W0 出口判据（已达成）**：`verify-wiring` 37 条 + `verify-spec` 7 条 + `verify-docs` 60 条违约，
合计 104 条，**逐条落在 §2 总账的 P0/P1/P2 上**；`compat` 绿。
原有门禁（`cargo build`/`cargo test` 149 项/`typecheck`/`verify-archive`）**保持全绿**。

---

### W1 生产端口实现（10 人日）

> 端口落点表（**先定死，避免实现位置漂移**）。依赖方向约束：
> `agent-core` 依赖 `agent-adapter`，**adapter 不许反向依赖 agent-core**。

| 端口 | 生产实现 | 落点 | 为什么在这里 |
|---|---|---|---|
| `AppHome` | ✅ 已有 | `crates/agent-adapter/src/app_home.rs` | 已是唯一读 `A_DA_HOME` 处 |
| `Clock` | ✅ 已有 | `crates/agent-adapter/src/clock.rs` | 已是唯一读系统时间处 |
| `ModelClient` | ✅ 已有 | `crates/agent-adapter/src/model/client.rs` | 网络适配器 |
| `ToolCatalog` | ✅ 已有 | `crates/agent-runtime/src/catalog.rs` | 组合根装配 |
| `Scope` | 🆕 W1-T1 | `crates/agent-adapter/src/scope/workspace.rs` | 路径判定属适配器 |
| `SessionStore` | 🆕 W1-T2a（搬运）+ W1-T2b（实现） | `crates/agent-adapter/src/store/fs.rs` | 设计 §9 明定 `store-fs` 在 adapter（[session/types.rs:6](crates/agent-core/src/session/types.rs) 已写"下一步搬进 adapter"） |
| `PromptSource` | 🆕 W1-T3 | `crates/agent-adapter/src/prompt/coding.rs` | 人格源按产品注入 |
| `CancelToken` | 🆕 W1-T4 | `crates/agent-adapter/src/cancel.rs` | 需要 watch/进程组合 |
| `EventSink` | 🆕 W1-T5 | `crates/agent-core/src/server/events.rs` | 需与 ws/emitter 交互，属**宿主** |
| `ApprovalGate` | 🆕 W1-T6 | `crates/agent-core/src/approval/gate.rs` | **策略**在 adapter（纯函数）、**执行**（问用户）需前端通道 → 宿主；见 AGENTS.md §14 |

**W1-T1 `WorkspaceScope`**
- 现状：路径沙箱逻辑在 `crates/agent-toolkit/src/sandbox.rs:103`（`check_workspace_sandbox`），
  带 realpath / symlink / junction 把关；`Scope::resolve_path` 契约要求越界返回 `DenialKind::Sandbox`。
- ⚠️ **防抓错**：`crates/ts-engine/src/sandbox/mod.rs:83` 有一个**同名**函数，那是插件运行时的沙箱，
  与工作区作用域无关。W1-T1 只复用 `agent-toolkit` 那一个。
- 产出：`crates/agent-adapter/src/scope/workspace.rs`；`id()` = 规范化后的工作区路径。
- 守门：conformance 补 `verify_scope_contract` 对**真实** `WorkspaceScope` 跑（当前 [ports/scope.rs](crates/agent-conformance/src/ports/scope.rs) 只测 double）；
  必须覆盖 `..` 穿越、工作区内 symlink 指向外部、Windows junction 三种越界。

**W1-T2a / W1-T2b `FsSessionStore`（搬运 + 实现，必须分两个提交）**
- 两条腿，**必须分两个提交**：
  - **W1-T2a 搬运**：`agent-core/src/session/{types,manager}.rs` + `checkpoint/**` → `crates/agent-adapter/src/store/**`；
    原位置留 `pub use` shim（**R6**），调用点零改动。`agent_core::session::types` 已 re-export 领域类型，
    搬运后 `AgentMessage` 仍只有一份（✅ 已确认，[types.rs:11](crates/agent-core/src/session/types.rs)）。
  - **W1-T2b 实现**：`FsSessionStore` 实现 `SessionStore`；`thread_id` ↔ `(workspace, session_id)` 的映射
    在适配器内部解决（端口签名只有 `thread_id`，[store.rs:11-21](crates/agent-base/src/ports/store.rs)）。
- 守门：conformance `verify_session_store_contract` 对**真实** `FsSessionStore` 跑 append→load 往返等价
  （当前只测 `InMemorySessionStore`），并补"旧蛇形键会话文件仍可加载"的兼容用例。

**W1-T3 `CodingPromptSource`**
- 现状：`build_system_prompt(&ws_str)` 硬编码在 [runner/prompt.rs:169](crates/agent-core/src/runner/prompt.rs#L169)，
  且 legacy 工具表也在同文件（[`get_all_tools_for_workspace` :131](crates/agent-core/src/runner/prompt.rs#L131)，后者归 W2）。
  两者都在 [agent_loop.rs:71-72](crates/agent-core/src/runner/agent_loop.rs#L71-L72) 被调用。
- 产出：把人格源搬进 adapter，`PromptSource::system_prompt()` 返回同一份文本（**逐字节等价**：
  golden 回放会比对 messages，一旦文本变了图片里的提示词就变了）。
- 守门：`PromptSource` 输出与 legacy `build_system_prompt` 的**快照比对**（W3-T5 的 golden 会覆盖）。

**W1-T4 `CancelToken`**
- **这是 P0-4 的一半，但只做端口侧**；接线在 W4。
- 产出：`CancelHandle`（`watch` 包装）+ `impl CancelToken`；提供 `child_token()` 以便子智能体独立取消。
- 守门：conformance 补 `verify_cancel_contract`（当前**完全没有** CancelToken 套件）。

**W1-T5 `WsEventSink`**
- 把 `AgentEvent`（[domain/event.rs](crates/agent-base/src/domain/event.rs)）接到现有快照/广播通道
  （`agent-core/src/server/emitter.rs`），保证 `seq` 单调（INV-6）。
- 守门：conformance 补 `verify_event_sink_contract` 对**真实** sink 跑；断言 `seq` 严格递增 + `TurnStarted/TurnFinished` 成对。

**W1-T6 `ApprovalGate` 生产实现（P0-3）**
- 结构（**策略/执行分离**）：
  - **策略**（纯函数，搬进 `crates/agent-adapter/src/approval/policy.rs`）：
    搬 [agent-core/src/approval/guard.rs:43](crates/agent-core/src/approval/guard.rs#L43) 的 `should_ask_approval`
    + `is_destructive_command` + `ApprovalGuardConfig.confirm_commands`；原位置留 shim。
  - **执行**（`crates/agent-core/src/approval/gate.rs`）：实现 `ApprovalGate`，
    读三档模式（`auto|ask|readonly`，**从 `AgentStore.config.approval` 真实读取**，不再是只写不读）、
    命中策略时 `register_waiter(call_id)` → emit `ApprovalRequested` → 等 `approval.decide` →
    `ApprovalOutcome { approved, by: AnsweredBy::{User,Policy,Timeout,Aborted} }`。
- 必须同时修的三件事（否则仍是半吊子）：
  1. `ApprovalRequest.mode` 现在被 [runtime.rs:353](crates/agent-base/src/engine/runtime.rs#L353) **硬编码成 `"auto"`** → 改为从 gate 取真实档位；
  2. [runtime.rs:335](crates/agent-base/src/engine/runtime.rs#L335) 的 `ApprovalPolicy::Named(_) => true` 是"名字当永远要问" → 改为**解析命名策略**（`Named("approval-guard")` → 调策略函数）；
  3. `AnsweredBy::Timeout` / `Aborted` 必须有真实来源（超时与取消分支），不能只写常量。
- 守门：conformance `ports/approval.rs` 的契约对**真实 gate** 跑（当前只对 `RecordingApprovalGate` double 跑，[approval.rs:46-52](crates/agent-conformance/src/ports/approval.rs)）；
  新增"三档模式 × 读写工具"矩阵用例；新增"前端答复如实标 `AnsweredBy::User`"用例。

| ID | 任务 | write scope | 守门测试 | 依赖 | 人日 | 状态 |
|---|---|---|---|---|---|---|
| W1-T1 | `WorkspaceScope` | `crates/agent-adapter/src/scope/**` | conformance scope 对真实实现 | W0 | 1 | ✅ 已完成 |
| W1-T2a | session/checkpoint **格式与行级 IO** 搬运 + shim | `crates/agent-adapter/src/store/**`、`crates/agent-core/src/{session,checkpoint}/**`（仅 shim 行） | 既有测试全绿（调用点不动） | W0 | 2 | ✅ 已完成 |
| W1-T2b | `FsSessionStore`（`SessionStore` 端口实现） | `crates/agent-adapter/src/store/fs_store.rs` | 真实实现往返/重启/清洗/compact 语义 5 项 | W1-T2a | 1.5 | ✅ 已完成 |
| W1-T3 | `CodingPromptSource` | `crates/agent-adapter/src/prompt/**` | 与 legacy 输出结构化等价（见 §13.2） | W0 | 1 | ✅ 已完成 |
| W1-T4 | `CancelHandle` / `CancelToken` | `crates/agent-adapter/src/cancel.rs` | conformance cancel（新） | W0 | 0.5 | ✅ 已完成 |
| W1-T5 | `WsEventSink` | `crates/agent-core/src/server/events.rs` | conformance event 契约（通用）+ agent-core 单测（真实 sink） | W0 | 1 | ✅ 已完成 |
| W1-T6 | `ApprovalGate`（策略+执行+三档） | `crates/agent-adapter/src/approval/**`、`crates/agent-core/src/approval/**` | conformance approval 契约 + agent-core 单测（真实 gate） | W1-T2a | 2.5 | ✅ 已完成 |
| W1-T7 | conformance 补齐：为 Scope/Store/Approval/Event/Prompt/Cancel 提供真实实现的契约用例 | `crates/agent-conformance/src/ports/**` | 全部 conformance 仍绿 | W1-T1…T6 | 0.5 | ✅ 已完成 |

**W1 进度（2026-10-09）：8/8 任务全部完成。**
`cargo xtask verify-wiring` 的端口缺失清单 **6 → 0**（10/10 端口有生产实现）；
该闸总违约 **37 → 31**；`cargo test --workspace` **212 passed / 0 failed**（W1 前 149）。详见 §13.2。

---

### W2 工具端口化与名单消除（10 人日）

| ID | 任务 | 现状证据 | 产出（write scope） | 守门测试 | 依赖 | 人日 | 状态 |
|---|---|---|---|---|---|---|---|
| W2-T1 | 工具包**真源**（名字→工具） + 三包工厂接线进 `CompositeToolCatalog` | 工厂 `fs_tools()`/`core_tools()`/`command_tools()` 零调用者 | `crates/agent-toolkit/src/toolkits.rs`（新）、`decision/mod.rs`、`crates/agent-runtime/src/builder.rs`、`products/*/agent.spec.json` | `verify-wiring` 可达 18→20；`verify-spec` 7→4；骨架产品端到端装配 | W1 | 3 | ✅ 已完成 |
| W2-T2 | 内置插件工具改 `Tool` 并进 catalog | `execute_builtin_plugin_tool` 是字符串分派 | `crates/agent-toolkit/src/plugin_tools.rs`（新）、`toolkits.rs`、`crates/agent-core/src/runner/builtin_tools.rs` | 23/23 描述符来自工具包工厂；产品级装配断言 | W2-T1 | 2 | ✅ 已完成 |
| W2-T3 | `run_background` / `read_url_content` **补实现或删声明**（R3 二选一，先出结论再动手） | [registry.rs:95](../crates/agent-toolkit/src/registry.rs#L95) / [:205](../crates/agent-toolkit/src/registry.rs#L205) 无执行器 | `crates/agent-toolkit/src/background.rs`（新）、`registry.rs`、`command/mod.rs` | `verify-wiring` 不可达 2 → **0** | W2-T1 | 1.5 | ✅ 已完成 |
| W2-T4 | `inspect_project` 别名可见性 | 别名不在任何插件清单，模型看不到 | — | **✅ 已由 W2-T2 覆盖**（它现在是 `project` 工具包里的真实 `Tool`，与 `project_inspect` 同一实现、各有注册表描述符） | W2-T2 | 0.5 | ✅ 已完成 |
| W2-T5 | **消除第 6 张名单**：`PLUGIN_BUILTIN_CATALOG` 改为从 registry 派生 | [dispatch.rs:898-922](../crates/agent-core/src/server/dispatch.rs#L898-L922) 写死 21 个 | `crates/agent-core/src/server/dispatch.rs`、`tools/xtask/src/gates.rs` | 结构断言（派生 + 无内联字面量）+ 行为断言（逐名相等） | W2-T1 | 1 | ✅ 已完成 |
| W2-T6 | 子智能体白名单/黑名单清理：删掉 8 个幽灵工具名 | `subagents/builtins.rs`；`NEVER_FOR_SUBAGENT` 5 个里 4 个是幽灵 | `crates/agent-core/src/subagents/**` | 新增断言：名单里的名字必须真实存在（注册表 ∪ legacy 显式名单） | W2-T1 | 1 | ✅ 已完成 |
| W2-T7 | 插件 `is_write` 清理：字段由 descriptor 派生，第三方插件不再恒 `false` | `plugins/types.rs:44`；`builtins.rs:10` 死数据；`manager.rs:363` 硬编码 | `crates/agent-core/src/plugins/**` | 新增断言：未知工具 `is_write == true`（失败安全）+ 内置插件与描述符一致 | W2-T1 | 0.5 | ✅ 已完成 |

**W2 进度（2026-10-09）：7/7 ✅ 里程碑完成（W2-T1…T7）**。
`verify-wiring` 违约 **31 → 2**（只剩 2 个孤儿协议方法，归 W5-T2）；
**23/23 描述符全部由工具包工厂提供**；`verify-spec` 违约 **7 → 4**；
`cargo test --workspace` **212 → 235 passed / 0 failed**。详见 §13.3–§13.7。

---

### W3 产品切引擎（12.5 人日，**最高风险**）

> **本里程碑必须用运行时开关双跑**（R8）：`A_DA_ENGINE=base|legacy`，直到 W3-T4 删 legacy。
> 每切一处都要能用 golden 回放证明"事件序列等价"。

| ID | 任务 | 现状证据 | 产出（write scope） | 守门测试 | 依赖 | 人日 | 状态 |
|---|---|---|---|---|---|---|---|
| W3-T1 | `agent-host`：`run_from_spec(spec)` 装配真实端口 + ws 传输挂钩 | 设计 §9 要求 `agent-host`；当前不存在 | `crates/agent-host/**`（新建）、workspace `Cargo.toml`、`products/ada-skeleton/**` | 单测：从 spec 到 `AgentRuntime` 全链路；骨架产品端到端走宿主层 | W1、W2 | 3 | ✅ 已完成 |
| W3-T2 | 3 处 `run_agent_loop` 调用点切到 `AgentRuntime::run_turn`（**先加开关**） | [dispatch.rs:534](../crates/agent-core/src/server/dispatch.rs#L534)/[:1476](../crates/agent-core/src/server/dispatch.rs#L1476)/[:1562](../crates/agent-core/src/server/dispatch.rs#L1562) | `crates/agent-core/src/runner/engine_bridge.rs`（新）、`server/dispatch.rs`、`server/ws.rs`、`crates/agent-adapter/src/cancel.rs`、`products/ada-coding/**` | 选择矩阵（含降级）+ **真引擎实跑**集成断言 + 产品冒烟 | W3-T1 | 3 | ✅ 已完成（默认仍 legacy，见 P1-14） |
| W3-T3 | 事件投影：`AgentEvent` → 现有 UI 事件/快照（**前端零改动**，R7） | legacy `AgentLoopEvent` 与领域事件形状不同（[agent_loop.rs:17-36](../crates/agent-core/src/runner/agent_loop.rs#L17-L36)） | `crates/agent-core/src/runner/engine_bridge.rs`、`server/dispatch.rs`、`state/store.rs` | 投影表**覆盖全部 12 种领域事件**；审批通道端到端；`tauri-ui` 无 diff | W3-T2 | 2 | ✅ 已完成 |
| W3-T4 | **删除 legacy**：`run_agent_loop`、`executor::execute_tool_call_extended`、legacy 工具分派 | 事实 B | `crates/agent-core/src/runner/**`、`server/dispatch.rs`、`server/ws.rs`、`src-tauri/**`、`tools/verify-archive.ts` | `verify-archive`：主干**无** `fn run_agent_loop`；无引擎注入 = 硬失败 | W3-T2/T3 | 2 | ✅ 已完成 |
| W3-T5 | **golden 回放夹具**（M0-T3 补课）：脚本化模型 → 冻结事件序列 JSON | `crates/agent-conformance/golden/` 不存在 | `crates/agent-conformance/golden/**`、`crates/agent-conformance/src/golden.rs`（新） | 夹具跑通且与冻结产物逐字段相等；篡改期望必须被检出 | W3-T1 | 1.5 | ✅ 已完成 |
| W3-T6 | 握手能力位与产品身份**从声明派生**（不再硬编码） | `session.initialize` 回 `ServerCapabilities::default()` + 硬编码的 `ada-coding` 产品信息 | `crates/agent-core/src/server/dispatch.rs`、`crates/agent-host/src/lib.rs`、`products/ada-coding/src/main.rs` | 断言：声明 `rollback=false`/无 `plugins` 的产品 → 握手回 `false`（default 是 `true`，两者结果不同）；产品 id/name/archetype 来自声明 | W3-T1 | 1 | ✅ 已完成 |

**W3 进度（2026-10-09）：6/6 ✅ 里程碑完成**（W3-T6 已于 W6-T7 之后补齐，见 §13.25）。

> **订正记录（2026-10-09）**：W6-T7 期间发现"进度写 6/6、`W3-T6` 状态列却写未开始"的矛盾。
> 核实结果：**任务确实未完成**——`session.initialize` 返回的是硬编码的
> `ServerCapabilities::default()` 与硬编码的产品信息，**没有**从产品声明派生。
> 当时把进度如实改为 5/6；随后在 §13.25 补齐该任务，**现为 6/6**。
**legacy 主循环已删除**：仓库里只剩 `agent-base` 一份多轮循环（`AgentRuntime::run_turn`，INV-1）。
`A_DA_ENGINE` 开关与"降级"语义一并移除——**没注入引擎 = 硬失败**（`EngineError::EngineNotInjected`）；
`ada-coding` / `src-tauri`（headless + 同进程）两个宿主都已无条件注入真引擎。
`verify-archive` 的引擎断言已从"恰好一处"翻转为"**一处都不许有**"。
`cargo test --workspace` **259 → 255 passed / 0 failed**（删掉的开关矩阵用例多于新增）。
⚠️ **代价如实登记**：子智能体委派（`invoke_subagent`）随 legacy 消失 → **W4-T5**；
子智能体执行循环仍是第二份引擎 → **W4-T6**。详见 §13.12。

**下一步：W4（取消贯穿收口 + 补回子智能体）。**
W4-T1/T2 修 `run_command` 的假接线与状态判定；W4-T3 子智能体取消透传；
W4-T4 强化 INV-8 断言；**W4-T5/T6 是 W3-T4 的欠账**（委派能力 + 第二份引擎），
按"能力不长期缺席"的要求应优先于 W5/W6。

**W3 完成后的运行时判据**（必须贴进 PR）：
`products/ada-coding` 冒烟跑一轮真实回合，日志里 `run_turn` 被执行、`run_agent_loop` 不存在于二进制符号表。

---

### W4 取消贯穿收口（4 人日）

| ID | 任务 | 现状证据 | 产出（write scope） | 守门测试 | 依赖 | 人日 | 状态 |
|---|---|---|---|---|---|---|---|
| W4-T1 | **修假接线**：`RunCommandTool` 把 `CancelToken` 接到 `abort_tx`（执行中轮询转发） | [command/run.rs:83-86](../crates/agent-toolkit/src/command/run.rs#L83-L86)：只在"已取消"时发一次，而那之前已 return → **死代码**；[cmd_tools.rs](../crates/agent-toolkit/src/cmd_tools.rs) 的 abort 分支因此不可达 | `crates/agent-toolkit/src/command/run.rs` | 断言：执行中翻转取消 → 命令被真的杀掉且**立刻**返回 | W1-T4 | 1 | ✅ 已完成 |
| W4-T2 | 状态判定改为结构化，不再靠输出子串猜 `"取消"`/`"超时"` | [run.rs:102-110](../crates/agent-toolkit/src/command/run.rs#L102-L110) | `crates/agent-toolkit/src/{fs_tools,cmd_tools,command/run}.rs` | 断言：输出含 `aborted` 的**成功**命令仍判 `Success` | W4-T1 | 1 | ✅ 已完成 |
| W4-T3 | 子智能体取消透传（`cancel.child_token()` → 子体执行循环内检查） | legacy 侧 `abort_rx: None`；子体循环不检查取消 | `crates/agent-core/src/subagents/tool.rs`、`runner.rs` | 断言：取消后子智能体停止产出且报告取消（连穿三层） | W4-T1 | 1 | ✅ 已完成 |
| W4-T4 | **强化 INV-8 断言**：从"启动前已取消"改为"运行中取消穿透到工具" | [inv8:23-24](../crates/agent-conformance/src/invariants/inv8_cancellation_penetration.rs#L23-L24) 只测启动前（引擎没走到 `Tool::execute`） | `crates/agent-conformance/src/invariants/inv8_cancellation_penetration.rs` | 断言：工具执行中取消 → 工具**观察到**取消 → 停机 `Aborted` → 回执 `Aborted` | W4-T1 | 1 | ✅ 已完成 |
| W4-T5 | **补回子智能体委派**：`invoke_subagent` 端口化为一等 `Tool` 进 catalog（W3-T4 删 legacy 时它随之消失，见 §2 P1-15） | `executor.rs` 的 `invoke_subagent` 分支已删；catalog 里无该工具 | `crates/agent-core/src/subagents/tool.rs`（新）、`crates/agent-toolkit/src/registry.rs`、`crates/agent-host/src/lib.rs`、`tools/xtask/src/gates.rs` | 断言：`capabilities.subagents=true` → catalog 含 `invoke_subagent` 且描述符来自注册表 | W4-T3 | 2 | ✅ 已完成 |
| W4-T6 | **消除第二份引擎**：子智能体执行循环并入 `AgentRuntime::run_turn` | `run_subagent` 自带多轮循环 + 用 legacy `execute_tool_call` | `crates/agent-core/src/subagents/{runner,ports,tool}.rs`、`crates/agent-host/src/lib.rs`、`tools/verify-archive.ts` | `verify-archive` 增断言"主干不存在第二份多轮引擎"；取消穿透断言 | W4-T5 | 3 | ✅ 已完成 |

**W4 进度（2026-10-09）：6/6 ✅ 里程碑完成**。
**P0-4 完全收口**：主循环侧（W4-T1/T2/T4）+ 子智能体侧（W4-T3/T6）；
**P1-16 第二份引擎已消除**——仓库里只有 `agent-base` 一份多轮循环，`verify-archive` 会盯着这一点。
`cargo test --workspace` **255 → 277 passed / 0 failed**（零警告）。详见 §13.13–§13.15。

**下一步：W5（协议与能力位对齐，7 人日）。**
W5-T1 `xtask gen` 或如实降级；W5-T2 孤儿方法（`WORKSPACE_SET`/`CONFIG_UPDATE`）；
W5-T3 `spec/proto/README.md` + `docs/protocol/*.md`；W5-T4 spec 能力位消费者；
W5-T5 `hooks` 如实降级。W5 完成后 `verify-wiring` 的 2 条孤儿方法与 `verify-spec` 的 4 条
（`identity`/`capabilities` 无消费者）应全部清零。

---

### W5 协议与能力位对齐（7 人日）

| ID | 任务 | 现状证据 | 产出（write scope） | 守门测试 | 依赖 | 人日 | 状态 |
|---|---|---|---|---|---|---|---|
| W5-T1 | `xtask gen`：四产物生成（Rust 常量 / client-ts / dispatch 臂 / 文档表）。**若判定不划算，则正式降级为"手写 + 双向校验"并改设计文档 §6.2**（R3） | 无 `gen`；`client-ts/*.ts` 手写维护 | `docs/agent-base-design.md` §6.2/§6.3、`crates/agent-proto/src/methods.rs` | 三对副本**双向**校验齐备且**能红**（3 种故障注入） | W0 | 2.5 | ✅ 已完成（**判定降级**） |
| W5-T2 | 孤儿方法处置：`WORKSPACE_SET` / `CONFIG_UPDATE` **补臂或从 spec 删除** | 无 dispatch 臂 | `crates/agent-proto/src/methods.rs`、`crates/agent-proto/client-ts/methods.ts`、`spec/proto/ada-coding.ext.json` | `verify-wiring` 孤儿=0 | W0-T3 | 1 | ✅ 已完成 |
| W5-T3 | `spec/proto/README.md`（命名规则与 `x.<product>.*` 约定）+ `docs/protocol/*.md` | 两者均不存在 | `spec/proto/README.md`、`docs/protocol/README.md`、`tools/xtask/src/gates.rs` | 文档存在且与登记表一致（`verify-docs` 新增守门） | W5-T1 | 1 | ✅ 已完成 |
| W5-T4 | **能力位与 spec 声明脱钩**：`Spec.capabilities`/`toolkits`/`identity` 必须有真实消费者 | [builder.rs:113-117](../crates/agent-runtime/src/builder.rs#L113-L117) 只用 `policies` | `crates/agent-adapter/src/prompt/coding.rs`、`crates/agent-host/src/lib.rs` | `verify-spec` 全绿；断言：身份进提示词、`images=false` 拒图片 | W1、W3 | 1.5 | ✅ 已完成 |
| W5-T5 | `FailDirection` 真实消费：`ApprovalGate::direction()` 接进判定；`Scope` 失败方向**如实定为固定拒绝** | 全仓无 `.direction()` 调用（3 处实现、0 处调用） | `crates/agent-base/src/engine/runtime.rs`、`crates/agent-base/src/ports/scope.rs`、`crates/agent-conformance/src/invariants/inv4_fail_direction.rs`、`tools/xtask/src/gates.rs` | 断言：无判定依据时按 `Closed` 拒绝、按 `Open` 放行、确定答案不被覆盖；`verify-wiring` 新门 | W1-T6 | 1 | ✅ 已完成 |

**W5 进度（2026-10-09）：5/5 ✅ 里程碑完成**。
**两条红例基线清零**：`verify-wiring` **2 → 0**（孤儿方法=0；`FailDirection` 现有真实消费者）；
`verify-spec` **4 → 0**（两个产品的 `identity`/`capabilities`/`toolkits` 都有生产消费者）。
**协议侧**：三对副本全部双向校验且**能红**（5 次故障注入）；`spec/proto/README.md` 与
`docs/protocol/README.md` 就位并有机械守门。
`cargo test --workspace` **277 → 289 passed / 0 failed**（零警告）。详见 §13.16–§13.18。

**下一步：W6（产品运行时缺口与文档收口，6 人日）。**
W6 是最后一段：3 个 dispatch 桩（`SUBAGENT_RESUME`/`STATS_PROMPT_CHARS`/`WORKSPACE_RESCAN`）、
CLI `a-da run`、`revert_*` 返回值与 `THREAD_EDIT_AND_RESEND`、INV-7 模型空断言、
前端重连、以及文档重写（`verify-docs` 的 **60 条**是唯一剩余基线，属 W6-T7）。

> **钩子（P1-3）**：MVP 只承诺"机制 + 3 个点位"，而当前是 **0 个点位**。

**W6 进度（2026-10-09）：7/7 ✅ 里程碑完成**。
两处**空壳断言**已被替换为可证伪的真断言：INV-7 从无条件 `Ok(())` 改为
"依赖纯度 + 领域源码纯度"（4 条测试，含 2 条自检抗体）；`ModelClient` 契约从
"`caps` 自己和自己比"改为**跑一次流、用行为对账声明**（3 条测试，含 2 条故障注入）。
另外发现并修掉一处**过时口径**：`verify-wiring` 的端口清单硬编码 10 个名字（漏 `Tool`），
而实际有 11 个 trait——现已改为**从 `pub trait` 派生**。
**3 个 dispatch 桩已处置**：`subagent.resume` / `workspace.rescan` **删声明**（协议方法 74 → 72，
三处同步），`stats.promptChars` **真实现**（实测提示词与工具规格字符数，不再返回编造的 1200/800）。
**CLI `a-da run` 已从"只建会话就退出"变成真执行**：装配引擎 → 建会话 → 落用户消息 →
跑一轮 → 从会话读回回复 → 落盘 JSONL。新增 `--dry-run`（不联网验证装配，也是集成测试抓手）。
**`revert_*` 与编辑重发已收口**：回滚回执带回恢复/删除清单（无检查点 → `ok:false`）；
`THREAD_EDIT_AND_RESEND` 与 `thread.start` **共用同一条执行泵**（事件不再被丢弃、错误不再被吞、可 abort）。
**前端重连已收口**：固定 2s → 指数退避（500ms 起、30s 封顶、±20% 抖动）+ 代次保护
（旧连接回调不再覆盖新连接）+ 断线立刻失败在途请求（不再干等 15s 超时）。
策略抽成**无副作用纯模块** `reconnect-policy.ts`，`verify-wiring` 新增四项结构性守门。
`cargo test --workspace` **302 passed / 0 failed**（零警告）。详见 §13.19–§13.23。

**W0–W6 全部完成。** 六条门禁全绿、**零红例基线**。本计划此后只作**回归守门**：
`cargo xtask verify` 系列是日常入口；新增功能请顺手更新 §5 状态列与 §13 执行记录。

> **遗留（不在本次范围）**：`hooks` 点位仍为 **0 个**——`dto.rs` 声明 `hooks: false`，
> 而 `docs/plugin-sdk/v1.md` 声称 25 个点位由 Rust 引擎调用，两者需对齐（实现 3 个点位，
> 或把文档与 SDK 契约改为"未实现"）。已登记在
> [docs/unfinished-features.md](unfinished-features.md) §一.9；
> 全量 25 点位归 `agent-base-plan.md` 附录 B 的 M6。

---

### W6 桩补齐与文档收口（10.5 人日）

| ID | 任务 | 现状证据 | 产出（write scope） | 守门测试 | 依赖 | 人日 | 状态 |
|---|---|---|---|---|---|---|---|
| W6-T1 | `SUBAGENT_RESUME` 真实现（或按 R3 删声明并让前端不再调用） | dispatch.rs 只回 `{ok:true}`；前端 `resumeSubagent` 先置 `running=true` 再调它 | `spec/proto/base.json`、`crates/agent-proto/{src/methods.rs,client-ts/methods.ts}`、`tauri-ui/src/{App.tsx,client/ws-client.ts,components/Composer.tsx}` | 断言：协议方法数与 dispatch 臂数一致（72/72）；`typecheck` 绿 | W3 | 1.5 | ✅ 已完成（**判定删声明**） |
| W6-T2 | `STATS_PROMPT_CHARS` / `WORKSPACE_RESCAN` 真实现或删声明 | `:891-896` 硬编码 `1200/800`；`WORKSPACE_RESCAN` 回 `Ok(Null)` | `crates/agent-core/src/server/dispatch.rs`、`spec/proto/*.json`、`crates/agent-proto/**` | 断言：`systemChars` = 实测提示词长度、`toolSpecsChars` = schema 字符数之和、且**不等于** 1200/800；无引擎时如实报错 | W3 | 1.5 | ✅ 已完成（`stats` 真实现 / `rescan` 删声明） |
| W6-T3 | CLI `a-da run`：加载模型 + 跑一轮 + 落盘 | `main.rs` 只建内存会话就退出（**不跑模型、不落盘**） | `src-tauri/src/cli_run.rs`（新）、`src-tauri/src/main.rs`、`src-tauri/Cargo.toml` | 断言：`--dry-run` 真跑一轮并落盘 JSONL；空指令退出码 2；无 provider 如实报错 | W3 | 1.5 | ✅ 已完成 |
| W6-T4 | `revert_*` 返回值回传前端；`THREAD_EDIT_AND_RESEND` 不再吞错/丢事件/可 abort | 三个 revert 臂丢弃 `_outcome` 回 `{ok:true}`；resend 另 spawn 空循环把事件抽干丢弃、只 `warn` 错误、`abort_rx: None` | `crates/agent-core/src/server/dispatch.rs` | 断言：回执含恢复/删除清单（`None` → `ok:false`）；resend 与 `thread.start` **共用执行泵**；抗体测试钉住"抽干丢弃"形态 | W3 | 1.5 | ✅ 已完成 |
| W6-T5 | 清掉两处空壳：`inv7` 真断言；`ports/model.rs` 自比断言 | `inv7:5-8`；`ports/model.rs:43-44` | `crates/agent-conformance/src/**`、`tools/xtask/src/gates.rs` | 两处都有真实断言且**能红**；端口清单**从 trait 派生**（11 个，缺实现 0） | W1 | 1 | ✅ 已完成 |
| W6-T6 | 前端重连：指数退避 + 上限 + generation guard + `onclose` 清 `pendingRequests` | 固定 2s 重连；无代次保护（旧回调覆盖新连接）；`onclose` 不清在途请求 | `tauri-ui/src/client/{ws-client.ts,reconnect-policy.ts}`、`tools/xtask/src/gates.rs` | `verify-wiring` 新门（四项结构性断言）+ 3 种故障注入证明能红；退避策略独立脚本验证 | — | 1.5 | ✅ 已完成 |
| W6-T7 | 文档收口：README 正文重写、`unfinished-features.md` 重写、`feature-catalog.md` 更新路径、本文件状态列 | README 656 行几乎全是 TS/GPUIX 时代内容；另两份文档 19 条引用归档路径 | `README.md`、`docs/unfinished-features.md`、`docs/feature-catalog.md`、本文件 | `verify-docs` **60 → 0**；新文档零新增违约 | 全部 | 2 | ✅ 已完成 |

---

## 6. 验收门与命令

| 门 | 命令 | 期望 | 失败时怎么办 |
|---|---|---|---|
| 构建 | `cargo build --workspace` | 绿 | — |
| 合规 + 归档 | `cargo xtask verify` | **始终绿**（它是主门禁，不承载红例基线） | 修实现，**不许放宽断言**（R4） |
| 声明一致 | `cargo xtask verify-spec` | W0 红 7 → W5-T4 转绿 | 补适配器或删声明（R3） |
| 接线结构 | `cargo xtask verify-wiring` | W0 红 37 → W2/W6 转绿 | 按 A/B/C/D 分项处置 |
| 文档时效 | `cargo xtask verify-docs` | W0 红 60 → W6-T7 转绿 | 重写那三份文档 |
| 跨产品 | `cargo xtask compat` | 绿（W0 起即为绿） | 冲突即"产品改内核"，退回声明层 |
| 归档 | `cargo xtask verify-archive` | **始终绿**；W3-T4 后断言 3 换成断言 9 | 见 R6 |
| 门二 | `cargo test --workspace -- --test-threads=1` | 绿（**必须串行**：2 个用例共享全局态） | 修实现 |
| 门一 | `bun run typecheck` | 绿 | — |
| 回归 | golden 回放（W3-T5） | 事件序列与冻结产物逐字段相等 | 查是否改了提示词/事件形状 |
| **运行时** | 冒烟：`ada-coding` 跑一轮 | `run_turn` 被执行；审批/取消/工具三条链路走端口 | 回退开关而非回退代码 |

> **红例基线的承载方式（W0 定的口径）**：预期为红的检查**一律不进** `cargo xtask verify`
> 与 `cargo test`，只作为独立子命令存在。这样"里程碑可独立交付"（R8）与
> "主门禁始终有信号"同时成立；每个缺口靠自己的子命令从红转绿，转绿证据就是该命令 exit 0。
>
> 本机注意：`CARGO_TARGET_DIR=../cargo_target_ada`（默认 target 编 `ring` 会报 MSVC D8050）。

---

## 7. 排期

**净工作量 57.5 人日**（= §4 合计，与各里程碑任务明细逐行相加一致）。

- 单人：≈ 11.5 周（按 5 人日/周）。
- 双人：≈ 8 周（含缓冲）。并行度损耗来自两条硬约束：**W3 全程只能一人**（同一批文件）、
  **`dispatch.rs` 同一时刻只能有一个写者**（见 §11 冲突预警）。

**双人分工**（A = 关键路径负责人兼 `dispatch.rs` 唯一写者；B 做正交工作）：

| 周 | A（关键路径） | B（正交，避开 `dispatch.rs`） |
|---|---|---|
| 1 | W0-T1…T7（闸与红例，3.5） | W5-T1（`xtask gen` 或降级定案，2.5） |
| 2 | W1-T1、W1-T2a、W1-T2b（4.5） | W1-T3、W1-T4、W1-T5（2.5） |
| 3 | W1-T6（执行侧）、W1-T7（3） | W1-T6（策略侧，adapter）+ W2-T1 起步 |
| 4 | W2-T2、W2-T5（3，`dispatch.rs` 唯一写者） | W2-T3、W2-T4、W2-T6、W2-T7（4） |
| 5 | **W3-T1**（agent-host，3） | W4-T1、W4-T2（2）+ W5-T2 |
| 6 | **W3-T2、W3-T3**（5） | W4-T3、W4-T4（2）+ W5-T3 |
| 7 | **W3-T4、W3-T5、W3-T6**（4.5） | W6-T3、W6-T5、W6-T6（4）+ W5-T4/T5 |
| 8 | W6-T1、W6-T2、W6-T4、W6-T7（6.5） | 与 A 一起跑 §1 六条命令 + 运行时判据 |

> **排期纪律**：任务可以并行，**文件不行**。开新任务前先查 §11 的 write scope；
> 若两者共有 `crates/agent-core/src/server/dispatch.rs`，则必须排成前后关系而不是同时开工。

---

## 8. 明确不做（防蔓延）

| 不做 | 理由 | 归属 |
|---|---|---|
| 25 个钩子点位全量实现 | MVP 只承诺 3 个；本次只做**文档与能力位对齐**（W5-T5 注） | M6 |
| 细粒度事件 `evt.item.*` / `session.resync` / `ping` / 多客户端抢答 | 设计明确后置 | M6 |
| 图片进模型（`ContentPart::Image` → 三家 provider） | 现行能力位如实 `images: false` | M6 |
| 第二真实产品 `ada-life` / `ada-assistant` | 已有 `ada-skeleton` 作复用性验收 | M6 |
| 插件 SDK v1 发布、WASM、非 Windows 打包 | 设计后置 | M7+ |
| `x.<product>.*` 命名空间迁移（`fs/change/plugin/...` 改名） | 需要老客户端兼容期，独立排期 | 独立 |
| 成本统计 / i18n / 文件树 UI | 产品功能缺口，与本次"接线收口"无关 | 见 unfinished-features |

---

## 9. `archive/ts-legacy` 处置

**不许复活**（INV-12）。本计划所有任务都**不得**引用 `archive/**` 作为参考实现；
需要 TS 时代的行为口径时，读 `docs/agent-conventions.md` 的 §N 表格，而不是读归档代码。
`verify-archive` 已锁定这一点，W0-T7 只增加"文档不得把归档路径当现行路径"这一条。

---

## 10. 断言变更记录（R4 要求）

> 放宽/删除任何 conformance 断言都必须在这里留档，否则 PR 不得合并。
> **加强**断言（新增断言点）也记在这里，便于回溯契约演进。

| 日期 | 断言 | 变更 | 理由 | 批准 |
|---|---|---|---|---|
| 2026-10-09 | `ApprovalGate` 端口契约 | **加强**：新增"`needs_approval()` 必须幂等"断言；`ApprovalRequest` 去掉 `mode` 字段（引擎不再伪造档位） | W1-T6 端口变更：策略判定从引擎移进端口（`needs_approval`），因为档位/危险命令/白名单/Named 策略都在适配器，而引擎在 `agent-base` 不依赖适配器。原 `Named(_) => true` 把"名字"当成了"永远要问"。**没有任何断言被放宽或删除** | 本计划 §5 W1-T6 |
| 2026-10-09 | `EventSink` 端口契约 | **加强**：新增 `event_probe_stream`（覆盖全部 12 种事件类型）+ `verify_event_sink_contract`（`emit` 必须同步接受任何类型） | W1-T5：原契约只测 `seq` 单调与 Turn 配对，**没有任何用例覆盖"12 种事件类型是否都被接受"**，抓不到 `todo!()`/漏分支 | 本计划 §5 W1-T5 |
| 2026-10-09 | `PromptSource` 端口契约 | **新增**（此前完全没有）：非空 + 幂等 + 无残留模板占位 | W1-T7：`PromptSource` 之前没有合规套件 | 本计划 §5 W1-T7 |
| 2026-10-09 | `CancelToken` 端口契约 | **新增**（此前完全没有）：幂等 + 状态如实（排除恒 true / 恒 false 假实现） | W1-T4/T7：`CancelToken` 之前没有合规套件 | 本计划 §5 W1-T4 |
| 2026-10-09 | 引擎步数预算 | **加强**：`max_steps=1` 的用例补上 `steps_taken == 1`（原来只断言 `stop_reason`） | W3-T5：golden 夹具 `budget_exhausted` 首次照出 off-by-one——被预算**拒绝的那次尝试**也被算进了 `steps_taken`（报 2 而非 1）。补上这条断言，同类回归再也躲不过 | 本计划 §5 W3-T5 |
| 2026-10-09 | `provider.delete` 用例 | **加强**（并去偶发）：改为自造专用供应商再删，不再删共享配置里的 `providers[0]` | W3-T5：原用例依赖共享配置里"至少 2 个供应商"，而该状态取决于同进程其他用例（INV-8），全量跑偶发红、单独跑必过。**没有放宽任何断言**，反而新增了"专用供应商已被删除" | 本计划 §5 W3-T5 |
| 2026-10-09 | 事件投影完备性 | **新增**：投影表必须覆盖**全部 12 种** `AgentEventBody`（每种恰好一次），且每条结论显式（投影 or 明确无对应物） | W3-T3：`ApprovalRequested` 当初就是被静默丢进"差集"导致 P1-14。穷尽 `match` 让新增领域事件**编译失败**，强迫作者回来补结论 | 本计划 §5 W3-T3 |
| 2026-10-09 | 审批界面契约 | **新增**：`set_tool_waiting_approval` 必须把工具卡片置为字符串 `"waiting_approval"`，且收尾要覆盖该状态 | W3-T3：前端按这个字符串决定是否渲染批准/拒绝按钮（`Transcript.tsx:683`）。改了字符串而没同步前端，按钮会**静默消失** | 本计划 §5 W3-T3 |
| 2026-10-09 | 归档门"引擎入口恰好一处" | **翻转**：从「`fn run_agent_loop` 恰好一处」改为「**一处都不许有**」 | W3-T4：legacy 主循环已删除，仓库里只剩 `agent-base` 一份多轮循环（INV-1）。**这是断言方向的反转，不是放宽**——它从"恰好一份实现"升级为"零 legacy 残留" | 本计划 §5 W3-T4 |
| 2026-10-09 | 无引擎注入的行为 | **新增**：`run_agent_turn(None, ...)` 必须返回 `EngineError::EngineNotInjected` 硬失败 | W3-T4：legacy 兜底删除后，"没装配引擎"是配置错误。若静默返回 `Ok`，界面会永远停在"运行中"而无任何报错 | 本计划 §5 W3-T4 |
| 2026-10-09 | **INV-8 取消贯穿** | **强化**：新增"**执行中**取消穿透到工具"断言（工具须**观察到**取消 → 停机 `Aborted` → 回执 `Aborted`） | W4-T4：原断言只测"启动前已取消"——那是**最弱**的情形，引擎在进循环前就返回了，根本没走到 `Tool::execute`，因此 P0-4 的假接线它一条都抓不到。**没有放宽任何断言** | 本计划 §5 W4-T4 |
| 2026-10-09 | `ToolResult` 失败原因 | **新增**：`failure: Option<ToolFailure>`，`run_command` 必须给出 `Timeout`/`Aborted`/`NonZeroExit`/`Other`；成功必须为 `None` | W4-T2：原先上层靠 `output.contains("取消")` 猜状态——输出是给人看的，换个措辞或本地化就会把 `Aborted` 静默判成 `Error` | 本计划 §5 W4-T2 |
| 2026-10-09 | 接线审计 check C 的"可达"口径 | **升级**：来源 (1) 从"legacy 字符串分派里出现过"换成"**core 侧 `impl Tool for` 文件里出现该名字**" | W3-T4 删掉 legacy 后原来源已失效；W4-T5 的 `invoke_subagent` 是**宿主耦合**工具（要 SubagentManager/父 provider/检查点），工具包工厂构造不出来。**结构证据只能证明"有实现"**，"被装配进 catalog"由 `agent-host` 的行为断言保证 | 本计划 §5 W4-T5 |
| 2026-10-09 | 归档门"第二份引擎" | **新增**：主干不得存在第二份多轮引擎（`agent-core` 无 `for step in`；`subagents/runner.rs` 必须调 `run_turn` 且不得用 `execute_tool_call`） | W4-T6：并入前子智能体自带多轮循环，INV-1 在那一侧不成立。签名的选择是刻意的——只用"**具体且不会误报**"的形态，不做宽泛的启发式 | 本计划 §5 W4-T6 |
| 2026-10-09 | `ALL_METHODS` 数量钉 | **收紧**：`76 → 74` | W5-T2 按 R3 删掉两个孤儿方法（`workspace.set` / `config.update`：无 dispatch 臂、前端从未调用）。这是**删除声明**后的必然结果，不是放宽断言——名字唯一性与去重断言原样保留 | 本计划 §5 W5-T2 |
| 2026-10-09 | **协议副本一致性（Rust ↔ client-ts）** | **强化**：从"方法值在 TS 文件里出现过"改为**`name → value` 映射双向相等** | W5-T1：旧写法是**假校验**，三个洞——**单向**（TS 多出常量不报）、**子串匹配**（注释里出现也算过）、**不校验 name↔value 映射**（把 `CONFIG_GET` 的值写成另一个存在的方法照样通过）。已用 3 种故障注入证明新断言会红 | 本计划 §5 W5-T1 |
| 2026-10-09 | 协议文档时效性 | **新增**：`spec/proto/README.md` 与 `docs/protocol/README.md` 必须存在；**每个事件主题必须被协议文档提到** | W5-T3：文档是"协议怎么用"的唯一入口，没有机械守门的话，删掉它们或加了事件主题却不更新文档都没有任何信号 | 本计划 §5 W5-T3 |
| 2026-10-09 | **INV-4 失败方向** | **强化**：从"断言 `FailDirection::default() == Closed`"改为断言**引擎行为**（4 个场景：`Closed`→拒绝 / `Open`→放行 / 声明与行为打架→以声明为准 / 确定答案不被覆盖） | W5-T5：旧断言是在断言枚举的 `#[default]` 属性，**是重言式**——把引擎里所有方向裁决都删掉它照样绿。**没有放宽任何断言** | 本计划 §5 W5-T5 |
| 2026-10-09 | `FailDirection` 消费门 | **新增**：`crates/agent-base/src/engine` 里必须有 `.direction()` 调用点 | W5-T5：该端口方法曾在 3 个闸门实现里都写了却**没有调用点**（trait 方法有默认实现，编译器不会提醒）。只断言这一件具体的事，不做宽泛的"所有 trait 方法都要被调用"启发式 | 本计划 §5 W5-T5 |
| 2026-10-09 | **INV-7 领域投影分离** | **强化**：从**无条件 `Ok(())`** 改为"依赖纯度（`agent-base` 依赖白名单）+ 领域源码纯度（禁止 UI/产品专有标识）" | W6-T5：旧函数体是 `Ok(())`——**永远通过**，把整条不变量删掉也没有任何信号。新增 3 条"抗体"测试（白名单收紧必须报错、扫描必须覆盖到文件、命中判定必须能识别），确保不会退化成空转 | 本计划 §5 W6-T5 |
| 2026-10-09 | **ModelClient 端口契约** | **强化**：签名去掉 `caps` 参数（**自比恒真**），改为"跑一次流、用观察到的行为对账 `capabilities()` 声明" | W6-T5：旧测试传 `caps = client.capabilities()`，检查的是"客户端报告的 streaming 等于客户端报告的 streaming"。新增 2 条故障注入（空 `stop_reason`、缺 `Done` 必须被拒） | 本计划 §5 W6-T5 |
| 2026-10-09 | 端口清单口径 | **升级**：`verify-wiring` check D 从硬编码 10 个名字改为**从 `pub trait` 派生**（11 个） | W6-T5：硬编码清单漏了 `Tool`，于是"`Tool` 有没有生产实现"根本没人在管。派生后新增端口自动纳入审计（与 W2-T5 同一条原则） | 本计划 §5 W6-T5 |
| 2026-10-09 | `ALL_METHODS` 数量钉 | **收紧**：`74 → 72` | W6-T1/T2 按 R3 删掉 `subagent.resume` / `workspace.rescan` 两个声明。这是**删声明**后的必然结果，不是放宽断言 | 本计划 §5 W6-T1/T2 |

---

## 11. 任务级 write scope 总表（R5）

| 任务 | write scope（前缀） |
|---|---|
| W0-T1…T7 | `tools/xtask/src/`（`main.rs` + `gates.rs`）、`tools/xtask/Cargo.toml` |
| W1-T1 | `crates/agent-adapter/src/scope/` |
| W1-T2a | `crates/agent-adapter/src/store/`、`crates/agent-core/src/{session,checkpoint}/` |
| W1-T2b | `crates/agent-adapter/src/store/` |
| W1-T3 | `crates/agent-adapter/src/prompt/` |
| W1-T4 | `crates/agent-adapter/src/cancel.rs` |
| W1-T5 | `crates/agent-core/src/server/events.rs` |
| W1-T6 | `crates/agent-adapter/src/approval/`、`crates/agent-core/src/approval/` |
| W1-T7 | `crates/agent-conformance/src/ports/` |
| W2-T1 | `crates/agent-toolkit/src/{fs,core,command,decision}/`、`crates/agent-runtime/src/builder.rs` |
| W2-T2…T7 | `crates/agent-toolkit/src/registry.rs`、`crates/agent-core/src/{plugins,subagents}/`、`crates/agent-core/src/server/dispatch.rs` |
| W3-T1 | `crates/agent-host/`（新建）、`Cargo.toml` |
| W3-T2…T4 | `crates/agent-core/src/server/dispatch.rs`、`crates/agent-core/src/runner/`、`src-tauri/src/lib.rs` |
| W3-T5 | `crates/agent-conformance/golden/`、`crates/agent-conformance/src/` |
| W3-T6 | `crates/agent-runtime/src/builder.rs`、`crates/agent-core/src/server/dispatch.rs` |
| W4-T1/T2 | `crates/agent-toolkit/src/command/run.rs`、`crates/agent-toolkit/src/cmd_tools.rs` |
| W4-T3 | `crates/agent-core/src/subagents/` |
| W4-T4 | `crates/agent-conformance/src/invariants/inv8_cancellation_penetration.rs` |
| W5-T1/T3 | `tools/xtask/src/`、`spec/proto/README.md`、`docs/protocol/` |
| W5-T2/T4/T5 | `crates/agent-core/src/server/dispatch.rs`、`crates/agent-runtime/src/`、`spec/proto/` |
| W6-T1/T2/T4 | `crates/agent-core/src/server/dispatch.rs`、`crates/agent-core/src/subagents/` |
| W6-T3 | `src-tauri/src/main.rs` |
| W6-T5 | `crates/agent-conformance/src/` |
| W6-T6 | `tauri-ui/src/client/ws-client.ts` |
| W6-T7 | `README.md`、`docs/**` |

**冲突预警**（R5）：`crates/agent-core/src/server/dispatch.rs` 被 W0-T3/T4、W2-T5、W3-T2、
W5-T2/T4、W6-T1/T2/T4 共用 → **这些任务必须串行**，不得并行改该文件。

---

## 12. 每个 PR 的自检清单（贴进 PR 描述）

```text
[ ] 任务 ID 出现在 commit message 里（R9）
[ ] 先有失败断言，再有实现（R1）；PR 描述贴了"红 → 绿"两条输出
[ ] 没有新增第二处真源（R2）；若新增，说明为什么不是缺陷
[ ] 缺口要么补实现要么删声明，没有留半吊子（R3）
[ ] 没有放宽/删除既有断言；若有，已在计划 §10 登记（R4）
[ ] 只改了自己任务声明的 write scope（R5）；与 §11 冲突预警一致
[ ] 搬运留了兼容 shim，调用点零改动（R6）
[ ] 前端无 diff，或属于 W6-T6（R7）
[ ] 里程碑可独立回退（R8）；W3 用运行时开关双跑
[ ] 同一提交更新了本文件状态列 + unfinished-features.md（R10）
[ ] cargo xtask verify / cargo test --workspace -- --test-threads=1 / bun run typecheck 全绿
```

---

## 13. 执行记录

### 13.1 W0 闸与红例先行（2026-10-09，已完成）

**交付物**（write scope = `tools/xtask/src/` + `tools/xtask/Cargo.toml`）：

| 文件 | 内容 |
|---|---|
| `tools/xtask/src/gates.rs` | 新建。4 个闸：`verify_spec` / `compat` / `verify_wiring`（A/B/C/D 四项）/ `verify_docs` |
| `tools/xtask/src/main.rs` | 新增子命令 `verify-spec` / `compat` / `verify-wiring` / `verify-docs`；`gate()` 按违约数决定退出码 |
| `tools/xtask/Cargo.toml` | 新增依赖 `agent-toolkit`（直接调 `standard_tool_descriptors()`，避免在 xtask 里另抄工具名单）+ `serde_json` |

**红例基线（W1–W6 的转绿目标）**：

| 闸 | 违约数 | 内容 |
|---|---|---|
| `cargo xtask verify-wiring` | **37** | 方法缺臂 2（`workspace.set`/`config.update`）；内置目录多 12 漏 13；描述符不可达 4（`todo`/`finish`/`run_background`/`read_url_content`）；端口缺生产实现 6 |
| `cargo xtask verify-spec` | **7** | `patch` toolkit 无模块 1；`identity`/`capabilities`/`toolkits` 无生产消费者 6（两个产品各 3） |
| `cargo xtask verify-docs` | **60** | README 41 + `unfinished-features.md` 13 + `feature-catalog.md` 6 |
| `cargo xtask compat` | **0（绿）** | base 24 方法、`ada-coding.ext.json` 52 方法、base ∪ ext 无交集 |

**既有门禁复核（W0 改动后实跑，全绿）**：`cargo build --workspace` exit 0；
`cargo test --workspace -- --test-threads=1` **149 passed / 0 failed**；`bun run typecheck` exit 0；
`cargo xtask verify-archive` exit 0（7 条断言）；`cargo xtask compat` exit 0。

**记录在案的口径偏差（4 条，均已说明理由，断言强度未削弱）**：

1. **T3–T6 不写成 `#[test]`，改为 `verify-wiring` 子命令。**
   理由：这四项当前**预期为红**，写进 `cargo test` 会让门二长期变红，掩盖其它信号，违反 R8。
   断言本身一条没少（缺臂数、集合差集、不可达清单、端口清单逐条打印）。
2. **T7 从 `verify-archive.ts` 移到 `verify-docs` 子命令。**
   理由：归档门的语义是"主干没有第二份实现"，文档时效性是另一件事；混在一起会让
   主门禁 `cargo xtask verify` 长期变红。断言未削弱（仍逐文件逐行报告，白名单逐项写明理由）。
3. **T1 不跑 `ProductBuilder::validate`，改为"解析 + toolkit 模块存在性 + 字段生产消费者"。**
   理由：实测 `CompositeToolCatalog::validate`（[catalog.rs:102-106](../crates/agent-runtime/src/catalog.rs#L102-L106)）
   只遍历**已注入的工具**，未注入时恒返回空 → 对空 catalog 跑 validate 是**空断言**
   （正是 W6-T5 要清理的那类"看起来有断言其实没有"）。等 W2-T1 有了 toolkit→tools 映射后再补真实装配校验。
4. **`--all` 参数没有实现**：`verify-spec`/`compat` 无条件全扫，不需要 `--all`。
   §1/§6 的命令名已同步为实际实现（避免文档漂移）。

**检查器实现注记（供后续任务参考，避免踩同一个坑）**：

- `#[cfg(test)]` 截断启发式（判定"生产段"）在 **2 个文件**上不适用，检查器**如实跳过并记 note**、
  不猜：[`crates/agent-adapter/src/model/stream.rs`](../crates/agent-adapter/src/model/stream.rs)、
  [`crates/agent-core/src/session/manager.rs`](../crates/agent-core/src/session/manager.rs)（各含 2–3 处 `#[cfg(test)]`）。
- **`pub use` 再导出不算消费者**：`crates/agent-runtime/src/lib.rs` 的
  `pub use spec::{AgentSpec, CapabilitySpec, IdentitySpec, PolicySpec}` 曾让 `identity`/`capabilities`
  被误判为"有消费者"（假阴性）。`uses_symbol()` 现在会跳过再导出行（含跨行块）。
  这条是 W0 自查出来的**检查器 bug**，不是产品缺口。
- match 体提取靠花括号配平；arm 头用"大写标识符 + `|`"严格校验，不依赖正则依赖。

**W0 期间新发现的缺口**（已补进 §2 总账，避免漏项）：**P1-9** —— `ada-coding` 声明 `patch`
工具包但 `crates/agent-toolkit/src` 下没有对应模块（`patch` 能力实际藏在 `fs` 的 edit/write 里）；
反向地，`decision` 有模块却未在任何 spec 里声明。归属 **W2-T1**。

**下一步**：W1（生产端口实现）。入口任务 W1-T1 `WorkspaceScope`，其转绿判据是
`cargo xtask verify-wiring` 的端口清单从 6 条降到 5 条。

### 13.2 W1 生产端口实现（2026-10-09，已完成：8/8 任务）

**已完成任务与产出**：

| 任务 | 产出 | 新增测试 | 转绿证据 |
|---|---|---|---|
| W1-T1 | `crates/agent-adapter/src/scope/{mod,workspace}.rs` | adapter 2 项 + conformance 1 项（真实实现） | `verify-wiring` 端口 `Scope → scope/workspace.rs` |
| W1-T2a | `crates/agent-adapter/src/store/{types,slug,checkpoint_types,jsonl}.rs` + 3 个 agent-core shim | adapter 7 项（jsonl） | 端口 `SessionStore` 可解析；`SessionManager` 改走同源实现 |
| W1-T2b | `crates/agent-adapter/src/store/fs_store.rs` | adapter 5 项 | 端口 `SessionStore → store/fs_store.rs` |
| W1-T3 | `crates/agent-adapter/src/prompt/{mod,coding}.rs` + `agent-core` shim | adapter 3 项 + agent-core 1 项 | 端口 `PromptSource → prompt/coding.rs` |
| W1-T4 | `crates/agent-adapter/src/cancel.rs` + `agent-conformance/src/ports/cancel.rs`（新建） | adapter 5 项 + conformance 5 项 | 端口 `CancelToken → cancel.rs` |
| W1-T5 | `crates/agent-core/src/server/events.rs` + conformance `event_probe_stream` / `verify_event_sink_contract` | agent-core 7 项 + conformance 3 项 | 端口 `EventSink → server/events.rs` |
| W1-T6 | `crates/agent-adapter/src/approval/policy.rs`（策略搬运）+ `crates/agent-core/src/approval/gate.rs`（`HostApprovalGate`） | adapter 8 项 + agent-core 10 项 + conformance 2 项 | 端口 `ApprovalGate → approval/gate.rs` |
| W1-T7 | conformance 补真实实现用例：`FsSessionStore`、`CodingPromptSource`（新契约）、真实策略驱动的探针 gate | conformance 6 项 | 6 个端口都有真实实现侧的契约用例 |

**累计指标变化（W1 收官）**：`verify-wiring` 违约 **37 → 31**；端口缺失 **6 → 0**
（**10/10 端口都有生产实现**）；`cargo test --workspace -- --test-threads=1` **149 → 212 passed / 0 failed**；
`cargo build --workspace` / `bun run typecheck` / `cargo xtask verify` 全部 exit 0；**零编译警告**。

**W1 期间的新发现（都已按 R3 处置，不是绕开）**：

1. **`build_system_prompt` 与插件扫描纠缠**——它内部调 `PluginManager::scan_plugins`，
   整段搬进适配器会形成 `agent-core → agent-adapter → agent-core` **循环依赖**。
   处置：按"**排版归适配器、数据归调用方**"拆开——`compose_system_prompt(workspace, extensions)`
   是适配器里的纯函数（唯一排版真源），`agent-core` 侧保留 `build_system_prompt(workspace)`
   作为**采集数据的 shim**（R6），调用点零改动。
2. **内置 9 个插件的工具恒被追加进系统提示词**（11 条 `- name: desc`）。
   这是 W1-T3 的**第一次断言写错时暴露的**：我原以为"临时工作区 + 临时 app_home ⇒ 没有扩展"，
   实测 `scan_plugins:232-237` **无条件**加入 9 个内置插件。断言改为结构化。
3. **`CancelToken` trait 保持只有 `is_cancelled()`**。`wait_cancelled()` / `subscribe()`
   实现为 `CancelHandle` 的**固有方法**——给端口加方法会影响所有实现与合规套件，
   属于**端口变更**，按 R3/R4 必须单独排任务并留档。
4. **`EventSink` 的真实实现无法进 conformance**：`agent-conformance` **不依赖** `agent-core`。
   拆两层——通用契约（"`emit` 必须同步接受全部 12 种事件类型"）在 conformance；
   具体实现的不变量（保序 / `seq` 违约记账 / 驱动广播器 / 账本上限）在 `agent-core` 单测。
5. **粗粒度传输的诚实边界**：`WsEventSink` 只驱动 16ms 合帧的**粗快照**，细粒度 `evt.item.*` 属 M6。
6. 🔴 **AppHome 单例**不能搬进适配器（W1-T2a 的硬约束）。`session/manager.rs` 里的
   `OnceLock<Box<dyn AppHome>>` 有一个 `#[cfg(test)]` 分支把默认目录指向临时目录；
   **`#[cfg(test)]` 在依赖 crate 里不生效**——`cargo test -p agent_core` 编译 `agent-adapter`
   时是**非 test 模式**，单例一旦搬过去，agent-core 的测试就会写到**用户真实的 `~/.a-da`**，
   直接违反 AGENTS.md 的硬保证。
   处置：**单例留在 `agent-core`**（过渡缝隙，INV-8 的已记录例外）；只把**格式与行级 IO**
   搬到适配器；`FsSessionStore` 通过构造参数接收 sessions 根目录（由组合根注入，符合 INV-8）。
7. 🔴 **落盘重复 `type` 键的坑**（W1-T2a 第一版的真 bug）。
   `SessionEntry` 是 `#[serde(tag = "type")]` 的内部标签枚举，而各条目结构体自己也有一个
   重命名为 `type` 的字段。把 `SessionEntry::Message(x)` **整个序列化**会产出
   `{"type":"message","type":"message",...}` —— **重复键**，读回时整行解析失败、
   被"坏行跳过"吃掉，表现为**会话静默变空**（当时 6 个测试同时红）。
   修正：**写内层结构体、读走枚举**（`append_message/append_compact/append_notice` 三个
   类型化入口，杜绝再传枚举），并加 `test_written_line_has_exactly_one_type_key` 钉住。
8. **坏行不再静默**：`read_entries` 返回 `ReadOutcome { entries, skipped_lines }`；
   `FsSessionStore` 与 `SessionManager` 都在 `skipped_lines > 0` 时 `tracing::warn!`。
   原实现 `continue` 掉坏行且无任何计数——那正是"看起来读到了其实少了东西"。
9. 🔴 **W1-T6 是一次端口变更**（已登记在 §10）：策略判定从引擎移进端口。
   原引擎自己判 `ApprovalPolicy::Named(_) => true`——把"名字"当成了"永远要问"，
   既不准确（`approval-guard` 的真实语义是"按档位 + 危险命令 + 白名单"），
   又把策略劈成了两处（引擎一处、适配器一处）。
   处置：给 `ApprovalGate` 加 `needs_approval(tool, policy, args)`（**策略归实现**，
   AGENTS.md §14），`ApprovalRequest` 去掉被伪造的 `mode` 字段；引擎只短路 `Never`。
   断言**只加强未放宽**：新增"`needs_approval` 必须幂等"。
10. **审批等待的取消只能是轮询**：`CancelToken` 端口只有 `is_cancelled()`，
    `HostApprovalGate` 用 50ms 轮询等待取消（秒级场景可接受）。
   要变成事件驱动就得给端口加"等它发生"的方法——那属于端口变更，按 R3/R4 单独排任务。
11. **超时/中止的 waiter 泄漏**：`ApprovalManager::resolve_approval` 只在**前端真的答复**时
    移除条目；超时与中止走别的分支。若不清理，map 里会留下永久泄漏的 waiter。
    处置：新增 `remove_waiter()` + `pending_count()`，`decide` 在所有分支后都清理（幂等），
    并加 `test_timeout_denies_and_cleans_up` / `test_cancellation_denies_and_cleans_up` 钉住。

**口径偏差（3 条）**：

1. **W1-T2a 的搬运范围收窄**：计划原写"`session/{types,manager}.rs` + `checkpoint/**`"。
   实际搬了 `session/types.rs`、`session/slug.rs`、`checkpoint/types.rs`（+ 新增 `jsonl.rs`），
   **两个 `manager.rs` 仍留在 `agent-core`**。理由：① 上面第 6 条的 AppHome 陷阱；
   ② 两个 manager 是 legacy 权威实现，**W3-T4 会整段删除**，现在搬它是纯 churn。
   端口实现（`FsSessionStore`）已在适配器，`verify-wiring` 的 `SessionStore` 已转绿；
   且 **R2 已满足**——`SessionManager` 的 `append_message` / `append_compact_entry` / `load_session`
   已改为委派适配器的**同一份** `jsonl` 实现，折叠语义不再有两份。
2. **`HostApprovalGate` 已实现并测通，但尚未接到产品路径**。
   W1-T6 的交付是"实现 + 契约 + 单测"；把引擎/分发切到它属于 **W3-T2**（切引擎）。
   在此之前它在生产路径上不会被构造——这一点如实记着，不假装已接线。
   （这正是 §2 P0-3 标为"W1-T6 ✅ / W3-T2 待接线"的原因。）
3. **W1-T7 原写"删除 6 个端口的 testing double 依赖泄漏"**，实际按
   "**为每个端口补真实实现的契约用例**"推进（Scope/Cancel/Event/Store/Prompt/Approval 全部已做）——
   conformance 里 double 的用例是**契约本身**的守门（不能删，R4），要补的是真实实现那一侧。

### 13.3 W2 工具端口化与名单消除（2026-10-09，已完成：7/7 任务）

**已完成任务与产出（W2-T1）**：

| 产出 | 内容 |
|---|---|
| `crates/agent-toolkit/src/toolkits.rs`（新） | **工具包真源**：`TOOLKIT_NAMES` + `tools_for_toolkit` + `tools_for_toolkits` + `all_toolkit_tool_names`。未知名/重名都返回 `Err`（失败安全，不许少装几个工具就继续跑） |
| `crates/agent-toolkit/src/decision/mod.rs` | 新增 `decision_tools(workspace)` 工厂（此前 `decision` 是唯一没有工厂的工具包） |
| `crates/agent-runtime/src/builder.rs` | 新增 `ProductBuilder::with_declared_toolkits(workspace)` —— `spec.toolkits` 的**真实消费者** |
| `products/ada-coding/agent.spec.json` | `["core","fs","command","patch"]` → `["core","fs","command","decision"]`（P1-9 处置：`patch` 是假声明，`decision` 有实现未声明） |
| `products/ada-skeleton/src/main.rs` | 从手工 `with_tool(FinishTool)` 改为 `with_declared_toolkits(...)`，并断言 `core` 三个工具都进了 catalog |
| `tools/xtask/src/gates.rs` | `verify-spec` 的 toolkit 判定改查 **`TOOLKIT_NAMES` 真源**（不再靠"目录是否存在"猜）；`verify-wiring` 的"可达"口径升级为 **legacy 分派 ∪ 工具包工厂** |

**转绿证据**：

| 指标 | W2 前 | 现在 |
|---|---|---|
| `verify-wiring` 总违约 | 31 | **29** |
| 其中：工具不可达 | 4（`todo`/`finish`/`run_background`/`read_url_content`） | **2**（只剩 `run_background`/`read_url_content`） |
| `verify-spec` 总违约 | 7 | **4**（`patch` 假声明与 `toolkits` 无消费者各消 2 处） |
| `cargo test --workspace` | 212 | **217 passed / 0 failed** |
| 端到端 | — | `cargo run -p ada-skeleton -- --demo` → 装配出 `["ask_user","todo","finish"]` 并跑完 1 轮 |

**W2-T1 期间的两条记录**：

1. **"可达"的判定口径必须随架构演进**（已升级 `verify-wiring`）。
   升级前只认"legacy 字符串分派里出现过这个名字"，于是 `todo`/`finish` 这类
   "实现早已存在、只是没被装配"的工具被误报为不可达。**误报会掩盖真缺口**——
   这正是 R1（红先绿后）要求"红灯必须与总账逐条对应"的原因：口径错了，
   红灯就不再是可信信号。
2. **`patch` 的处置是"删声明"而不是"补模块"**（R3）。
   `patch` 的能力（unified diff 生成）实际由 `fs` 的 edit/write 提供，
   独立成一个工具包没有任何工具可放。所以按"补实现或删声明"的删声明一侧处置，
   并把判定改成查真源——这样以后任何"声明了但没实现"的工具包都会在装配期直接报错。
3. **`with_declared_toolkits` 目前只被 `ada-skeleton` 使用**（端到端证明）；
   `ada-coding` 的产品入口仍走 legacy `WsHostServer`（P0-1），接线属 **W3-T2**。
   这一点如实记着，不假装 ada-coding 已经按声明装配了。

### 13.4 W2-T3 `run_background` / `read_url_content` 处置（2026-10-09，已完成）

**R3 决策：整族补实现 + 单个删声明**（两个工具的情况不同，不能一刀切）。

| 工具 | 决策 | 理由 |
|---|---|---|
| `run_background` + `check_task` + `kill_task` | **补实现**（整族） | 只实现 `run_background` 是**半吊子**——能起 dev server 却看不到输出、也停不掉。整族实现才是完整能力（界面也一直广告这三个）。基础设施已具备：`cmd_tools` 的 `kill_process_tree` / `truncate_buffer` 可直接复用 |
| `read_url_content` | **删声明**，登记 M6 移植项 | 需要 HTTP 客户端 + HTML→Markdown。设计 §1.2 已把 `web` 工具包明确后置到 M6；在 `agent-toolkit` 里塞 reqwest 会破坏分层（网络属适配器）。删声明比留一个假描述符诚实 |

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-toolkit/src/background.rs`（新） | `BackgroundTasks`（**实例态**任务表，INV-8）+ 三个 `Tool` 实现 + `background_tools()` 工厂 |
| `crates/agent-toolkit/src/registry.rs` | 新增 `check_task`/`kill_task` 描述符；删除 `read_url_content` 描述符 |
| `crates/agent-toolkit/src/command/mod.rs` | `command_tools()` 纳入后台任务族（三个工具**共享同一张任务表**） |
| `crates/agent-toolkit/src/cmd_tools.rs` | `kill_process_tree` / `truncate_buffer` 改 `pub(crate)` 供复用（不复制一份） |

**转绿证据**：`verify-wiring` 违约 **29 → 26**；**工具不可达 2 → 0**（注册表 23 个描述符全部可达，
其中 16 个来自工具包工厂）；`cargo test --workspace` **217 → 224 passed / 0 failed**；零编译警告。

**W2-T3 期间的三条记录**：

1. **`Access` 表达不了"有副作用但不碰文件/不跑命令"**。`kill_task` 终结进程树，
   既不是 `ReadOnly`、也不是文件 `Mutates`、也不是命令 `Executes`。
   当前归到 `Executes { command_arg: "task_id" }` 并写清理由——
   给它加一个 `Access` 变体是**领域变更**，按 R3/R4 应单独排任务，不该顺手做。
2. **后台任务故意不跟随轮次取消**（与 `run_command` 的刻意不对称）：
   它的语义就是"活过这次工具调用"；要停它必须走 `kill_task`（用户显式动作）。
3. **`BackgroundTasks::start` 必须在 Tokio 运行时内调用**（要 spawn 输出泵与收尾任务）。
   这一条是测试写错时暴露的：我最初用非 async 的 `#[test]` 调它，直接 panic
   "there is no reactor running"。已写进方法文档，并把该测试改成 `#[tokio::test]`。

**顺带产生的下游影响**（都已登记归属，不遗漏）：
`read_url_content` 从注册表删除后，两处"引用它"的地方变成悬空引用——
`PLUGIN_BUILTIN_CATALOG`（[dispatch.rs:904](../crates/agent-core/src/server/dispatch.rs#L904)）归 **W2-T5**，
子智能体白名单（[subagents/builtins.rs:66](../crates/agent-core/src/subagents/builtins.rs#L66)）归 **W2-T6**。
`verify-wiring` 现在会如实报出前者（"广告了注册表里不存在的工具 `read_url_content`"）。

---

### 13.5 W2-T5 消灭第 6 张名单（2026-10-09，已完成）

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-core/src/server/dispatch.rs` | `PLUGIN_BUILTIN_CATALOG` 臂改为 `standard_tool_descriptors().iter().map(builtin_tool_info)`；新增投影函数 `builtin_tool_info`（`label`←`summary`，`description`←由声明派生的"读写性 · 执行模式 · 审批要求"） |
| `crates/agent-core/src/server/dispatch.rs`（测试） | 新增 `test_builtin_catalog_matches_registry_exactly`：**逐名相等** + 四字段齐全 + `isReadOnly` 与描述符一致 |
| `tools/xtask/src/gates.rs` | 审计 B **口径升级**：从"解析臂内字面量比集合"改为"断言这一段是**派生的**"（引用了 `standard_tool_descriptors()` 且内联字面量为 0） |
| `crates/agent-core/src/lib.rs` | 那条 `assert_eq!(catalog.len(), 21)` 改为与注册表长度比较 |

**转绿证据**：`verify-wiring` 违约 **26 → 2**（第 6 张名单的 24 条清零，只剩 2 个孤儿协议方法）；
`cargo test --workspace` **224 → 225 passed / 0 failed**；零编译警告；主门禁全绿。

**W2-T5 期间的两条记录**：

1. **审计口径必须跟着架构演进**（与 W2-T1 同一个教训，第二次出现）。
   旧的审计 B 解析臂里的 `"name": "x"` 字面量再与注册表比集合——一旦把清单改成**派生**，
   它反而会报"漏了 23 个"。正确的不变量不是"字面量集合相等"，而是
   **"这一段永远是派生的"**。所以现在断言：臂体引用 `standard_tool_descriptors()`、
   且内联工具名字面量为 0。**行为侧的逐名相等**放在能真的调 dispatch 的 `agent-core` 单测里。
2. 🔴 **一条 panic 的测试会污染共享状态，让另一个测试变红**（本次实测）。
   修完 W2-T5 后 `test_dispatcher_extension_methods` 因那条写死的 `21` 失败；
   它 panic 在**中途**，跳过了后续步骤，于是把共享的临时 `config.json`
   留在了不一致状态，导致**另一个**测试 `test_dispatcher_provider_management` 也红。
   单独跑后者是通过的。
   → 教训正是 R1 的理由：**红灯必须与总账逐条对应**。第二条红是症状不是病因；
   照着它去"修 provider.delete"会完全修错地方。修好第一条后两条一起绿。
   这条也是 AGENTS.md「Rust 测试必须串行跑」那段警告的又一实例（共享全局态，INV-8 未根治）。


### 13.6 W2-T2 插件工具改 `Tool` 并进 catalog（2026-10-09，已完成）

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-toolkit/src/plugin_tools.rs`（新） | 6 个**自由函数**（逻辑真源：`project_inspect` / `code_outline` / `git_status` / `git_diff` / `git_log` / `run_tests`）+ `PluginTool`（描述符**取自注册表**，不另写 schema）+ `git_tools` / `project_tools` / `test_tools` 工厂 |
| `crates/agent-toolkit/src/toolkits.rs` | `TOOLKIT_NAMES` 增加 `git`、`project`（`git` 与设计 §9 的工具包列表一致） |
| `crates/agent-toolkit/src/command/mod.rs` | `command` 工具包纳入 `run_tests`（它本质就是"跑一条探测出来的命令"） |
| `crates/agent-core/src/runner/builtin_tools.rs` | 6 个分支改为**委派** `agent_toolkit::plugin_tools::*`；删除已搬走的 `execute_project_inspect` / `execute_code_outline` 及其测试 |
| `products/ada-coding/agent.spec.json` | `toolkits` 增补 `git`、`project` |
| `products/ada-coding/src/main.rs` | 新增 `test_ada_coding_declared_toolkits_assemble`：声明 → 工具包真源 → catalog 逐名对齐 |

**转绿证据**：`verify-wiring` **23/23 描述符全部来自工具包工厂**（此前只有 16 个，其余靠 legacy 分派"算可达"）；
违约数保持 **2**（只剩孤儿协议方法）；`cargo test --workspace` **225 → 231 passed / 0 failed**；零编译警告；主门禁全绿。

**W2-T2 期间的两条记录**：

1. 🔴 **`code_outline` 一直在静默失效**（本次搬运才暴露）。
   它拿 `read_file` 的输出做 `starts_with("pub fn ")` 匹配，但 `read_file` 每行都带
   `"<行号> | "` 前缀（`fs_tools.rs:120`）→ **永远匹配不到任何签名**，
   每次返回"文件 [...] 未提取到显著类、函数或接口大纲"。
   这是一个**装上了但从来没工作过**的工具，而且它的失败形态是"成功返回一段空结论"——
   比报错更糟：模型会以为这个文件真的没有函数。
   修法：匹配前剥掉 `"N | "` 前缀；`test_code_outline_extracts_rust_signatures` 钉住。
   → 这正是"搬运时补测试"的价值：**R1 的红先绿后不只是防回归，它会照出从没被验证过的路径**。
2. **描述符取自注册表，不另写 schema**。`PluginTool::new` 用
   `registry::find_tool_descriptor(name)` 取描述符并在找不到时 **panic**：
   实现了却没声明 = 立刻炸，而不是造出一个没有描述符的工具。单测
   `test_every_wrapped_tool_has_a_registry_descriptor` 逐字段比对，确保两边不会各写一份。

**顺带解决**：W2-T4（`inspect_project` 别名模型看不到）——它现在是 `project` 工具包里的真实 `Tool`，
与 `project_inspect` 同一实现、各自有注册表描述符。W2-T4 可标记为已被覆盖。


### 13.7 W2-T6 / W2-T7 名单收敛（2026-10-09，已完成）

**W2-T6：子智能体白名单/黑名单幽灵名清理**

| 改动 | 内容 |
|---|---|
| `subagents/builtins.rs` | 三个 profile 的 `allowed_tools` 删除 8 个**本仓无实现**的名字：`read_files`、`find_symbol`、`get_outline`、`read_url_content`、`Skill`、`design_decision`、`edit_files`、`run_test_focused`；其中 `get_outline`→`code_outline`、`run_test_focused`→`run_tests` 是**改名到真实工具** |
| 同上 | 四个 profile 的 `disallowed_tools` 由 `Some([5 个名字])` 改为 `None`——那 5 个里 4 个是幽灵，剩下 1 个由 `NEVER_FOR_SUBAGENT` 全局覆盖；写工具由 Readonly 模式的结构判定覆盖 |
| `subagents/runner.rs` | `NEVER_FOR_SUBAGENT` 从 5 个名字收敛为 **1 个**（`invoke_subagent`，唯一真实存在者）；新增 `LEGACY_ONLY_TOOLS` 显式豁免表 |
| `subagents/mod.rs` | 原测试 mock 了 4 个幽灵工具并断言"它们被过滤掉"——**空转断言**（那些工具不可能出现在工具表里），改为只 mock 真实工具；新增 `test_subagent_tool_lists_have_no_ghost_names` |

**W2-T7：插件 `is_write` 收敛**

| 改动 | 内容 |
|---|---|
| `plugins/mod.rs` | 新增 `plugin_tool_is_write()`：按 `ToolDescriptor` 判定，**未知工具失败安全地当作写操作**；新增 3 条断言（描述符一致性 / 未知→写 / 内置插件与描述符一致） |
| `plugins/builtins.rs` | 删掉工具定义三元组里的 `is_write` 布尔（9 个插件、11 条工具）——它早就是死数据（装配时用 `_` 丢弃） |
| `plugins/manager.rs` | 第三方插件的 `is_write: false` 硬编码改为调用 `plugin_tool_is_write()` |

**转绿证据**：`cargo test --workspace` **231 → 235 passed / 0 failed**；零编译警告；主门禁全绿；
`verify-wiring` 保持 2（只剩孤儿协议方法，属 W5）。

**W2-T6/T7 期间的两条记录**：

1. 🔴 **"断言过滤掉了不存在的工具"是空转断言**。
   `test_resolve_subagent_tools_never_for_subagent` 用 `mock_tool("check_subagent")` 造了一个
   **本仓根本没有实现**的工具，然后断言它被过滤掉——这条断言**永远为真**，
   即使过滤逻辑完全删掉也照样绿。它给出的安全感是假的。
   → 教训：mock 的名字必须来自真源；否则测试在验证一个不存在的世界。
   新增的 `test_subagent_tool_lists_have_no_ghost_names` 把"名字必须真实存在"变成硬断言
   （注册表 ∪ 显式 legacy 名单），且反向断言"一旦某名字进了注册表，就必须从 legacy 豁免表移除"——
   防止豁免长期滞留。
2. **失败安全的方向要写死在代码里，不能靠调用方自觉**。
   第三方插件的 `is_write` 原先恒 `false`：写工具在界面上被标成"只读"，
   在只读档位/plan 模式/只读子智能体里都可能被放行（AGENTS.md §2 正是这条）。
   现在未知工具一律当写——**宁可多问一次审批，也不要悄悄给出写权限**。


### 13.8 W3-T1 宿主装配层 `agent-host`（2026-10-09，已完成）

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-host/`（新建） | `run_from_spec(spec, HostOptions) -> HostedProduct`：装配**真实端口**并构建 `AgentRuntime`；另有 `run_from_spec_json` 入口与 `HostedProduct::run_turn` |
| `Cargo.toml`（workspace） | 注册 `crates/agent-host` 成员与依赖别名 |
| `products/ada-skeleton/` | 产品**不再自己拼端口**：改为 `run_from_spec` + `run_turn`，代码里看不到任何端口类型（这正是"产品是薄声明"的形态） |

**真实端口一览（无一个测试替身）**：`WorkspaceScope` / `FsSessionStore` / `CodingPromptSource` /
`SystemClock` / `NetworkModelClient` / `HostApprovalGate` / `WsEventSink`。
`HostOptions.model` 与 `broadcast_tx` 是**注入点**，生产默认值都是真实实现。

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent-host` | **4/4 通过**（含 spec → 真实端口 → AgentRuntime → 一轮 → 事件账本 → 会话落盘 的全链路） |
| `cargo run -p ada-skeleton -- --demo` | `宿主装配成功！包含工具: ["ask_user","todo","finish"]`，1 步、4 事件、exit 0 |
| `cargo test --workspace` | **235 → 239 passed / 0 failed**；零编译警告 |
| 全部门禁 | 主门禁 / `verify-wiring`(2) / `verify-spec`(4) / `verify-docs`(60) / `compat` / `verify-archive` 全部绿 |

**W3-T1 期间的三条记录**：

1. **不建第二个 ws 服务**（防漂移）。计划原文写的是"`ws` 传输宿主"，但 `WsHostServer`
   已经存在于 `agent_core::server`——再写一个就是第二份实现。所以本任务的交付是
   **把 runtime 与事件出口准备好**：`HostOptions::with_broadcast(tx)` 让 ws 层接管广播通道，
   不接则走**离线形态**（`WsEventSink::with_capacity(None, 4096)`，只记账不广播）。
   把 `WsHostServer` 的分发切到本 crate 的 runtime 是 **W3-T2**。
2. **离线形态是刻意的，不是偷懒**：若无条件创建一个 `StateBroadcaster`，
   它的 `UnboundedSender` 在没有 ws 层消费时会把每条广播堆在内存里
   （长轮次下无界增长）。`None` 分支避免了"没人消费的无界通道"。
3. **"组合根存在" ≠ "产品已在用它"**。W3-T1 只让 **`ada-skeleton`（演示产品）** 走通新路径；
   **`ada-coding` 与 Tauri 宿主仍走 legacy `run_agent_loop`**（P0-1 只解决了一半）。
   这一点如实记着，不假装引擎已切换——真正的切换判据在 W3-T2（3 处调用点）与 W3-T4
   （`verify-archive` 断言"主干无 `fn run_agent_loop`"）。

### 13.9 W3-T5 golden 回放夹具（2026-10-09，已完成）

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-conformance/src/golden.rs`（新） | `GoldenFixture` 结构 + `replay()`（走**真实引擎** `run_turn`，只把模型换成脚本）+ `compare()` + **目录扫描**发现夹具 + `A_DA_GOLDEN_BLESS=1` 维护工具 |
| `crates/agent-conformance/golden/text_only.json` | 纯文本一轮（含 thinking.delta）：5 事件、1 步、`completed` |
| `crates/agent-conformance/golden/todo_tool_then_text.json` | 真实工具调用（`core` 的 `todo`）后收尾：7 事件、2 步、`completed` |
| `crates/agent-conformance/golden/budget_exhausted.json` | `maxSteps=1` 且模型每轮都要工具：**必须显式** `budget_exhausted` 收尾 |

**冻结粒度（刻意设计）**：`seq` / `kind` / 文本 / 工具名 / 停止原因**冻结**；
`at_ms` / `total_duration_ms` **不冻结**（墙钟），但断言时间**非递减**。

**转绿证据**：`cargo test --workspace` **239 → 245 passed / 0 failed**（连跑 2 次稳定、零警告）；
主门禁 / `verify-wiring`(2) / `verify-spec`(4) / `verify-docs`(60) / `compat` / `verify-archive` 全绿。

**🔴 W3-T5 立刻抓到两个真 bug**（这正是"先有尺子再切引擎"的价值）：

1. **引擎步数预算 off-by-one**（P1-12）。夹具 `budget_exhausted` 显示 `maxSteps=1` 却
   `stepsTaken=2`。根因：[runtime.rs:137-144](../crates/agent-base/src/engine/runtime.rs#L137-L144)
   **先 `step_index += 1`、后判 `step_index > max_steps`**，于是被预算**拒绝的那次尝试**
   也被算进了"已执行步数"。
   既有单测只断言了 `stop_reason`，**没断言步数**，所以这个 off-by-one 一直没被发现。
   修法：**先判预算、后自增**；并补上 `steps_taken == 1` 断言（记入 §10）。
2. **`provider.delete` 用例偶发红**（P1-13）。它在共享配置里删 `providers[0]`，
   而"列表里至少 2 个供应商"这个前提取决于**同进程其他用例**（`app_home()` 是进程级单例）——
   当列表只剩 1 个时，`provider.delete` 会按设计拒绝（"至少保留一个供应商"）。
   表现：**单独跑必过、全量跑偶发失败**。这正是 AGENTS.md「Rust 测试必须串行跑」那段
   警告的又一次实例，根因仍是共享全局态（INV-8）。
   修法：自造一个专用非激活供应商再删它 → 测试与执行顺序/共享状态无关。
   验证：`agent_core` lib 连跑 3 次 + 全量连跑 2 次，稳定绿。

**设计上刻意做的三件事**：

1. **夹具用目录扫描发现，不维护文件清单**。新增夹具 = 放一个 `.json` 进 `golden/`，
   测试自动覆盖——避免"加了夹具忘了登记"的静默失效（与 W2-T5 同一个教训）。
2. **`A_DA_GOLDEN_BLESS` 必须显式设置才改写期望**。默认绝不自动更新冻结产物，
   否则夹具会退化成"把当前行为抄一遍"，失去约束力。
3. **夹具的 `compare()` 自己也被测试**：`test_tampered_expectation_is_detected` 篡改期望
   （少一个事件、步数 +99）并断言比对**必须失败**——防止"compare 永远返回 Ok"这种
   让整套夹具形同虚设的缺陷。

### 13.10 W3-T2 引擎切换开关与注入（2026-10-09，已完成；默认仍 legacy —— 该开关已在 §13.12 删除）

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-core/src/runner/engine_bridge.rs`（新） | `EngineChoice`/`EngineSelection`（含**降级**识别）+ `LoopEventBridge`（`EventSink` 实现，把 `AgentEvent` 投影回 legacy 事件推进原 `mpsc`）+ `run_agent_turn`/`run_agent_turn_with`（**3 处调用点的统一入口**） |
| `crates/agent-core/src/server/dispatch.rs` | `Dispatcher` 增 `engine` 字段 + `with_engine`/`pipe_engine`/`has_engine`/`engine_selection`；**3 处调用点**改为调 `run_agent_turn`；新增 `EngineInjection` |
| `crates/agent-core/src/server/ws.rs` | `WsHostServer::bind_with_engine(port, token, store, Option<EngineInjection>)`；`bind` 委托给它（既有调用点零改动） |
| `crates/agent-adapter/src/cancel.rs` | 新增 `WatchedCancel`（只观测外部 `watch` 通道的 `CancelToken`）+ `never()`；2 条断言 |
| `crates/agent-host/src/lib.rs` | `HostOptions::with_store`：复用宿主的会话态（审批档位必须与界面一致） |
| `products/ada-coding/` | 按声明装配真引擎并注入（失败**如实降级**为 legacy，不让宿主起不来） |

**开关语义**（`A_DA_ENGINE`）：

| 环境变量 | 注入引擎 | 实际走 | 说明 |
|---|---|---|---|
| 未设 / 其他值 | 任意 | legacy | **默认**——切换必须显式开启，否则回滚无从谈起 |
| `runtime` | 有 | **runtime** | 真引擎 |
| `runtime` | 无 | legacy + **标记降级** | 不假装切了：打 warn 并在 `EngineSelection::describe()` 里如实标注 |

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent_core --lib engine_bridge` | **9/9 通过**（含"真引擎实跑"集成断言） |
| `cargo test --workspace` | **245 → 256 passed / 0 failed**；零编译警告 |
| 产品冒烟（`A_DA_ENGINE=runtime`） | `host.log`: **`已按产品声明装配真引擎：工具 23 个`** + `WebSocket 服务端已在 127.0.0.1:6780 成功绑定` + `A_DA_HOST_READY` |
| 产品冒烟（`A_DA_ENGINE=legacy`） | 同样起得来、行为不变（对照） |
| 全部门禁 | 主门禁 / `verify-wiring`(2) / `verify-spec`(4) / `verify-docs`(60) / `compat` / `verify-archive` 全绿 |

**W3-T2 期间的四个关键决定**：

1. **用"事件桥"而不是改 UI 投影**。legacy 产出 `mpsc<AgentLoopEvent>`，真引擎产出
   `EventSink::emit(AgentEvent)`；`dispatch.rs` 里那套投影到 `AgentStore` 的代码有几千行，
   是最不该动的地方。`LoopEventBridge` 把领域事件投影**回** legacy 事件推进同一条通道 →
   **切换的 diff 只落在"谁在跑这一轮"上，前端与 UI 投影路径一行不改**（R7）。
   W3-T3 再把这条投影换成直接消费 `AgentEvent`。
2. **3 处调用点收敛到一个入口**。三个调用点原来是同一段 `run_agent_loop(...)` 复制品；
   如果各自加 `if`，迟早"改了两处漏一处"。现在只有 `run_agent_turn` 一个判断点。
3. 🔴 **`EngineInjection` 强制共用审批 waiter 表**。新引擎的闸门把 waiter 注册在它持有的
   `ApprovalManager` 上，而 UI 的"批准"经 `APPROVAL_DECIDE` 落到 `Dispatcher.approval_mgr`。
   两者若是不同实例，**UI 的答复永远送不到闸门，每次审批静默等满 300s 超时**。
   所以注入结构体里 `approval_mgr` 不是可选项，且 `pipe_engine` **只取 runtime**——
   事后替换会留下"闸门挂在旧表上"的坑。
4. **测试不靠改环境变量切换路径**。`A_DA_ENGINE` 是进程级的，测试里 `set_var` 会污染
   同进程其他用例（AGENTS.md 明确警告过）。所以拆出 `select_engine_with` /
   `run_agent_turn_with` 显式入口，选择矩阵与真引擎实跑都在**不碰环境变量**的前提下断言。

**🔴 如实记录：切换**还不能**作为默认（P1-14）**

真引擎会发 `ApprovalRequested` 领域事件，但 legacy 事件集 `AgentLoopEvent` **没有对应变体**，
桥接时按差集丢弃。后果：`A_DA_ENGINE=runtime` 且处于"询问档"时，受审批约束的工具调用
会**静默等满 300s 超时**，界面不显示任何批准按钮。

这不是 W3-T2 的疏漏而是**顺序**问题：补审批事件的界面通道正是 **W3-T3（事件投影）** 的工作，
而 W3-T4（删 legacy）排在 W3-T3 之后。所以在 W3-T3 完成前，**默认保持 legacy**，
开关只用于受控验证——这一点必须写进 PR，不能因为"测试全绿"就以为可以直接翻默认值。

### 13.11 W3-T3 事件投影补全与审批界面通道（2026-10-09，已完成）

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-core/src/runner/agent_loop.rs` | `AgentLoopEvent` 新增 `ApprovalRequested { id, tool }` 变体 |
| `crates/agent-core/src/runner/engine_bridge.rs` | `ApprovalRequested` 从"差集丢弃"改为**投影**；新增投影表完备性断言 |
| `crates/agent-core/src/server/dispatch.rs` | 两处 UI 投影点处理新变体 → `store.set_tool_waiting_approval(...)` |
| `crates/agent-core/src/state/store.rs` | 新增 `set_tool_waiting_approval`（状态置 `"waiting_approval"` + `details.approval`）+ 界面契约断言 |

**为什么"前端零改动"能成立**：前端**早就**支持这个状态——
`Transcript.tsx:683` 的 `isAwaiting = toolStatus === 'waiting_approval' || 'awaiting'`
会渲染批准/拒绝按钮，点击发 `approval.decide`。缺的一直是**后端从没把状态置成 `waiting_approval`**
（全仓 `waiting_approval` 在 Rust 侧原本**零出现**）。所以这一轮补的是后端最后一公里，
界面一行不用改（R7 成立；`git diff tauri-ui` 为空，只有 `bun run typecheck` 生成的未跟踪 `bun.lock`）。

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent_core --lib engine_bridge` | **11/11**（含审批通道端到端） |
| 审批通道端到端断言 | 引擎发 `ApprovalRequested` → 桥转发到界面通道 → `resolve_approval` 命中 waiter → **轮次继续并收尾**（断链则超时失败） |
| 投影表完备性 | 12 种领域事件**每种恰好一次**，结论显式；穷尽 `match` 使新增变体编译失败 |
| `cargo test --workspace` | **256 → 259 passed / 0 failed**；零编译警告 |
| 全部门禁 | 主门禁 / `verify-wiring`(2) / `verify-spec`(4) / `verify-docs`(60) / `compat` / `verify-archive` 全绿 |

**W3-T3 期间的三条记录**：

1. 🔴 **"差集"是静默失效的温床**。W3-T2 里我把 `ApprovalRequested` 归入"两代事件集的差集"
   并显式返回 `None`——当时看起来是"诚实地列出了没有对应物的事件"，但**诚实 ≠ 安全**：
   它让一个必须有界面通道的事件变成了静默丢弃。
   现在改为**投影表完备性断言**：12 种事件每种恰好一次，且每条结论必须显式；
   穷尽 `match` 让新增领域变体**编译失败**，强迫作者回来回答"投影到哪里 / 为什么没有"。
2. **编译器的非穷尽 `match` 是最好的迁移工具**。给 `AgentLoopEvent` 加一个变体后，
   编译器立刻指出 `dispatch.rs:632` 与 `:1656` 两处投影点——不用 grep、不会漏。
   同时它产生的"未使用变量"警告是**连带产物**（闭包类型未知），补上分支即消失；
   差点被误读成"代码被改坏了"。
3. **R7 的"前端零改动"是靠先查前端做出来的，不是靠假设**。我先读了
   `Transcript.tsx` / `client-ts/dto.ts`，确认界面认 `waiting_approval` 且已有
   `decideApproval` 通道，才敢断言"补后端即可"。如果前端没有这个状态，
   正确的做法是**新开任务改前端**，而不是硬塞进 `awaiting` 复用语义。

### 13.12 W3-T4 删除 legacy 主循环（2026-10-09，已完成）

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-core/src/runner/agent_loop.rs` → `ui_events.rs` | **删除 `run_agent_loop`（389 行）**；文件只剩界面事件枚举 `AgentLoopEvent`（现在是纯投影层类型）+ 穷尽性断言 |
| `crates/agent-core/src/runner/engine_bridge.rs` | 删除 `EngineChoice`/`EngineSelection`/`EngineUsed`/`engine_choice`/`select_engine*`/`A_DA_ENGINE` 开关与"降级"语义；新增 `EngineError::EngineNotInjected`；`run_agent_turn` 少了 legacy 专用的 `workspace`/`session_mgr`/`checkpoint_mgr` 三个参数 |
| `crates/agent-core/src/runner/executor.rs` | `execute_tool_call_extended` 并回 `execute_tool_call`（4 个参数已恒为 `None`）；删除 `invoke_subagent` 分支 |
| `crates/agent-core/src/server/{dispatch,ws}.rs` | 3 处调用点改用新签名并 `warn!` 记录失败；`Dispatcher::engine_selection()` 删除（概念已不存在） |
| `crates/agent-host/src/lib.rs` | 新增 `build_engine_injection(store, ws, spec_json, sessions_root)`——两个宿主共用一处装配（R2） |
| `src-tauri/**` | headless 与同进程两条路径都注入真引擎；装配失败**致命**（不再有 legacy 兜底）。spec 用 `include_str!` 引用产品目录那一份，不另写 |
| `tools/verify-archive.ts` | 断言 3 **方向反转**：从"恰好一处"改为"**一处都不许有**" |

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo xtask verify-archive` | ✔ `主干不存在 legacy 主循环（fn run_agent_loop）`（7 条断言全绿） |
| `cargo test --workspace` | **259 → 255 passed / 0 failed**（删掉的开关矩阵用例多于新增）；零编译警告 |
| 无引擎注入 | `run_agent_turn(None, ...)` → `EngineError::EngineNotInjected`（硬失败，新断言钉住） |
| 产品冒烟（**无任何开关**） | `host.log`: `已按产品声明装配真引擎：工具 23 个` + ws 绑定 + `A_DA_HOST_READY` |
| 全部门禁 | 主门禁 / `verify-wiring`(2) / `verify-spec`(4) / `verify-docs`(60) / `compat` / `verify-archive` 全绿 |

**W3-T4 期间的四条记录**：

1. 🔴 **删除之前必须先测绘"谁在用"**。`run_agent_loop` 的删除面不止它自己：
   `execute_tool_call_extended` 的 4 个扩展参数只服务于它；`invoke_subagent` 分支只被它调用；
   `get_all_tools_for_workspace`/`build_system_prompt` 也只剩它一个消费者。
   测绘时还发现**子智能体有自己的多轮循环**——删掉主循环后它成了唯一残留的第二份引擎（P1-16）。
2. 🔴 **能力缺口必须显式登记，不能让它"顺带消失"**。`invoke_subagent` 今天在 legacy 主循环里
   是**可用**的（模型工具表里有它、`executor.rs` 有实现）。删 legacy 会让这个能力消失。
   按 R3 的"补实现或删声明"，正确处置是：**删声明 + 登记缺口 + 排任务**（P1-15 → W4-T5），
   而不是留着一个没有实现的名字，也不是假装它还在。
3. **"降级"是隐藏配置错误的方式**。W3-T2 的 `A_DA_ENGINE` 开关与"没引擎就退回 legacy"在
   迁移期是有价值的（可回滚），但 legacy 一删就必须同时删掉它——否则"没装配引擎"会表现成
   "界面永远停在运行中"。现在它是 `EngineError::EngineNotInjected`。
4. **两个宿主的装配必须收敛到一处**。`ada-coding` 与 `src-tauri`（headless + 同进程）需要
   完全一样的装配逻辑（复用 store、会话落 app home、把 `ApprovalManager` 带出去共用）。
   把 `build_engine_injection` 放进 `agent-host` 而不是各写一份——这正是 W2-T1 假声明的教训。
   另外 `src-tauri` 的 spec 用 `include_str!("../../products/ada-coding/agent.spec.json")`
   **引用同一份文件**，不另写。

### 13.13 W4-T1 / W4-T2 / W4-T4 取消贯穿主循环侧收口（2026-10-09，已完成）

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-toolkit/src/command/run.rs` | **W4-T1**：把 `CancelToken` 用 `select!` 轮询**转发**到命令的中止通道（原两行是死代码）；**W4-T2**：状态改由 `ToolFailure` 判定 |
| `crates/agent-toolkit/src/fs_tools.rs` | **W4-T2**：`ToolResult` 新增 `failure: Option<ToolFailure>`（`Aborted`/`Timeout`/`NonZeroExit`/`Other`）+ `ToolResult::failed(...)` |
| `crates/agent-toolkit/src/cmd_tools.rs` | **W4-T2**：超时/中止/非零退出各自给出结构化原因；新增 `CANCEL_POLL_INTERVAL` |
| `crates/agent-conformance/src/invariants/inv8_cancellation_penetration.rs` | **W4-T4**：新增 `assert_cancellation_penetrates_running_tool`——阻塞工具在**执行中**被取消 |

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent-toolkit --lib command::run` | **5/5**（含"执行中取消真的杀掉命令"） |
| `cargo test -p agent-conformance --lib inv8` | **2/2**（启动前 + **执行中穿透**） |
| `cargo test --workspace` | **255 → 263 passed / 0 failed**；零编译警告 |
| 全部门禁 | 主门禁 / `verify-wiring`(2) / `verify-spec`(4) / `verify-docs`(60) / `compat` / `verify-archive` 全绿 |

**W4-T1/T2/T4 期间的三条记录**：

1. 🔴 **"假接线"的形态是"看起来接了、其实永不触发"**。原代码：
   ```rust
   if ctx.cancel.is_cancelled() { return ...; }      // 已取消 → 提前返回
   let (abort_tx, abort_rx) = watch::channel(false);
   if ctx.cancel.is_cancelled() { abort_tx.send(true); }   // ← 永远不可达
   ```
   第二处判断在第一处之后，条件恒假 → **死代码**。它的危害不是"没写"，而是"看起来写了"：
   代码里有 `abort_tx`、有 `abort_rx`、有 `send(true)`，静态审查很容易放过。
   **判定标准必须是"有没有一条测试能让它真的触发"**——所以 W4-T4 的断言特意要求
   工具**观察到**取消，而不只是"轮次最终 Aborted"。
2. **`CancelToken` 是轮询式端口，所以转发也必须轮询**。端口只有 `is_cancelled()`，
   没有"等它发生"的入口（加方法是端口变更，需单独排任务）。`ToolContext<'a>` 里的
   `cancel: &'a dyn CancelToken` 也不能被 move 进 `tokio::spawn`（生命周期不够）。
   解法是 `select!` 把"轮询翻转中止通道"和"等待命令"放在**同一个 future** 里，
   既不引入端口变更，也不引入 `'static` 约束。50ms 延迟对"用户按停止键"完全够用。
3. **状态判定不能解析给人看的文本**。原实现 `output.contains("取消")` / `contains("aborted")`：
   换措辞、做本地化、甚至**命令自己输出了这个词**都会误判。现在失败原因由**产生它的那层**
   给出（`ToolFailure`），测试里专门放了一条"输出含 `aborted` 但命令成功"的用例钉住这一点。

### 13.14 W4-T5 补回子智能体委派（2026-10-09，已完成）

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-toolkit/src/registry.rs` | 新增 `invoke_subagent` 描述符（注册表 23 → **24**）；`Access::Executes{command_arg:"task"}`——**绝不能标只读**（失败安全：它会驱动一个能写文件的子智能体） |
| `crates/agent-core/src/subagents/tool.rs`（新） | `InvokeSubagentTool`：描述符**取自注册表**；读**父会话真实 provider 配置**；4 条断言 |
| `crates/agent-core/src/subagents/runner.rs` | `LEGACY_ONLY_TOOLS` **清空**（`invoke_subagent` 不再是 legacy 专属；W2-T6 的反向断言自动要求这一步） |
| `crates/agent-host/src/lib.rs` | 组合根按 `capabilities.subagents` 注入；`HostedProduct` 暴露 `subagent_mgr`；新增行为断言 |
| `tools/xtask/src/gates.rs` | check C 的"可达"口径升级（见 §10 记录） |
| `products/ada-coding/src/main.rs` | 产品断言从"工具包恰好覆盖注册表"改为"工具包 ⊆ 注册表 + 唯一宿主注入项是 `invoke_subagent`" |

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent-core --lib subagents` | **9/9**（含描述符来自注册表、非只读、未知 profile 报错、**父 provider 缺失如实失败**） |
| `cargo test -p agent-host` | **5/5**（含 `test_subagent_delegation_tool_is_in_the_catalog`） |
| `verify-wiring` check C | `注册表 24 个描述符，可达 24 个（工具包工厂 23 个、core 侧 Tool 实现 1 个），不可达 0 个` |
| `cargo test --workspace` | **263 → 268 passed / 0 failed**；零编译警告 |
| 产品冒烟 | `host.log`: **`已按产品声明装配真引擎：工具 24 个`** |
| 全部门禁 | 全绿 |

**W4-T5 期间的四条记录**：

1. **"工具包工厂"装不下宿主耦合的工具**。工具包工厂是纯函数 `(workspace) -> tools`，
   而委派需要 `SubagentManager` + 父 provider 配置 + 检查点管理器，三样都无法从路径构造。
   硬塞进工具包就要给工厂加"宿主能力"参数，并把这份依赖一路穿过
   `tools_for_toolkits` → `ProductBuilder` → 所有调用点——**为一个工具改一圈签名不划算**。
   最终分工：**注册表放描述符**（元数据真源）、**core 放实现**、**组合根负责装配**。
2. 🔴 **"可达"的结构证据 ≠ "被装配"**。check C 只能证明"存在一个 `impl Tool for`"，
   证明不了它真的进了 catalog。所以 W4-T5 的出口判据里**必须**有一条行为断言
   （`agent-host` 的 `test_subagent_delegation_tool_is_in_the_catalog`）——
   否则就是 W2-T1 那类"实现存在但没接线"的老毛病换个位置复发。
3. **能力开关必须真的决定装配**（INV-10）。`capabilities.subagents=false` 的产品**不该**拿到
   `invoke_subagent`：否则"声明说不支持子智能体，模型却看得到这个工具"。
   装配点直接读 `spec.capabilities.subagents`，并各有一条断言（true → 有 / false → 无）。
4. **不编造配置**。legacy 在父 `parent_config` 缺失时会造一个 `gemini-2.5-flash` + 空 api_key
   的配置继续跑——那等于**静默用一个用户没配过的模型**。现在读父会话真实配置，
   读不到就如实失败（`test_missing_parent_provider_fails_instead_of_fabricating_one` 钉住）。

### 13.15 W4-T6 / W4-T3 子智能体并入单一引擎（2026-10-09，已完成）

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-core/src/subagents/runner.rs` | **删除自有循环**，改为装配 `AgentRuntime` 跑 `run_turn`；新增 `filter_subagent_tools`（描述符驱动）+ `SubagentProgressSink`（`AgentEvent` → `SubagentStepUpdate`） |
| `crates/agent-core/src/subagents/ports.rs`（新） | 3 个**生产**端口实现：`ProfilePrompt` / `EphemeralSessionStore` / `ReadonlyEnforcingGate`（+ `denial_text`） |
| `crates/agent-core/src/subagents/tool.rs` | W4-T3：`select!` 轮询把 `ToolContext.cancel` 转发进 `run_subagent` 的 `abort_rx` |
| `crates/agent-host/src/lib.rs` | 把 `spec.toolkits` 传给委派工具（子智能体走**同一处装配**） |
| `tools/verify-archive.ts` | 断言 8：**主干不存在第二份多轮引擎** |

**隔离性由装配保证**（不再靠"另写一个循环"）：

| 隔离维度 | 靠什么 |
|---|---|
| 只拿到 profile 人格（AGENTS.md §3） | `ProfilePrompt` |
| 上下文一次性、不污染主会话 | `EphemeralSessionStore` |
| 只读档位挡写工具（运行期第二道防线） | `ReadonlyEnforcingGate`（`by: Policy`，不是"等用户批准"） |
| 只装裁切后的工具（第一道防线） | `filter_subagent_tools` + `CompositeToolCatalog` |
| 父会话取消**真的**传进来（W4-T3） | `WatchedCancel` → 引擎 `CancelToken` → `Tool::execute` |

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent_core --lib subagents` | **18/18**（含取消穿透、描述符裁切、进度投影） |
| `cargo xtask verify-archive` | ✔ **`主干不存在第二份多轮引擎`**（8 条断言全绿） |
| `cargo test --workspace` | **268 → 277 passed / 0 failed**；零编译警告 |
| 全部门禁 | 主门禁 / `verify-wiring`(2) / `verify-spec`(4) / `verify-docs`(60) / `compat` 全绿 |

**W4-T6/T3 期间的五条记录**：

1. 🔴 **"另写一个循环"是隔离性的错误实现方式**。子智能体需要的是"受限的上下文与工具集"，
   而它被实现成了"再写一份引擎"：于是取消不穿透、工具只读判定另有一套、
   审批与回执结构各写一遍。并入之后，隔离性由**装配**表达（5 个端口实现），
   行为由**同一份引擎**保证——这才是 INV-1 想要的形态。
2. **测试里要能"让工具真的阻塞"**。第一版取消测试用脚本化模型 + 瞬时工具，
   结果子智能体在取消发出前就跑完了（测试失败在"不得报告成功"）——
   断言本身是对的，是**场景没构造出"正在运行"**。
   改成让它跑一条真实长命令后，测试同时验证了 W4-T3 与 W4-T1（取消连穿三层）。
   → 教训：**验证"取消"必须先保证"确实在跑"**。
3. **注入点是给测试的，不是给生产的**。子智能体原来直接用 `stream_model_chat` 拉模型，
   改成 `AgentRuntime` 后走 `ModelClient` 端口；`RunSubagentOptions.model` 默认 `None`
   → 真实 `NetworkModelClient`。**默认值是真实实现**，注入点只是让测试能换成脚本模型。
4. **门禁断言要选"具体且不误报"的签名**。"主干不存在第二份多轮引擎"如果写成
   "没有别的 `loop {`"，会把大量正常代码判红。最终选的是：`agent-core` 里不得出现
   `for step in`（手写引擎最典型的写法）+ `subagents/runner.rs` 必须调 `run_turn`
   且不得用 `execute_tool_call`。**宁可窄而准，也不要宽而吵**。
5. **子智能体没有"用户"可问**，所以只读档位的写工具必须是**策略拒绝**（`by: Policy`），
   而不是"注册 waiter 等批准"。这也是 `ReadonlyEnforcingGate` 与 `HostApprovalGate`
   分开的原因——前者的答案是确定的，后者需要界面通道。

### 13.16 W5-T4 / W5-T2 能力位接真消费者 + 孤儿方法清零（2026-10-09，已完成）

**两条红例基线清零**（自 W0 起一直挂着）：

| 门 | 前 | 后 |
|---|---|---|
| `cargo xtask verify-spec` | 4 条违约 | **✔ 无违约** |
| `cargo xtask verify-wiring` | 2 条违约（孤儿方法） | **✔ 无违约**（`协议方法 74 个，match 臂 74 个，缺失 0、孤儿 0`） |

**W5-T4 产出**（`identity`/`capabilities` 从"解析后没人读"变成真消费者）：

| 声明字段 | 真实消费者 | 行为 |
|---|---|---|
| `identity.name` / `.persona` | `CodingPromptSource::with_identity`（agent-host 显式读 `IdentitySpec`） | 产品名字与人格进入系统提示词首行 |
| `identity.locale` | 同上（`language_line_for`） | 非中文环境换掉语言指引；`zh-*` 保持逐字节一致 |
| `capabilities.subagents` | agent-host（W4-T5） | 决定是否装配 `invoke_subagent` |
| `capabilities.images` | `HostedProduct::run_turn_with_images` | 声明 `false` → **拒绝**图片输入 |
| `toolkits` | `tools_for_toolkits` + 子智能体装配 | 决定 catalog |

**W5-T2 产出**：`workspace.set` / `config.update` 在 **spec / Rust 常量 / client-ts 三处同步删除**。

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent-adapter --lib prompt` | **8/8**（含"无身份声明时逐字节一致"） |
| `cargo test -p agent-host` | **7/7**（含身份进提示词、`images=false` 拒图片） |
| `cargo test --workspace` | **277 → 284 passed / 0 failed**；零编译警告 |
| `verify-wiring` / `verify-spec` / `compat` / `verify-archive` | 全绿 |
| `verify-docs` | 60（W6-T7 待办，唯一剩余基线） |

**W5-T4/T2 期间的四条记录**：

1. 🔴 **"审计口径"和"实现"会互相拖住**。`verify-spec` 的判据是"生产代码里**使用了类型名**
   `IdentitySpec`/`CapabilitySpec`"。原先 agent-host 只写 `spec.capabilities.subagents`
   （读字段、不出现类型名），所以即使 W4-T5 已经把能力位接上了，门**仍然红**。
   正确做法不是放宽门，而是让消费者**显式取类型**（`let caps: &CapabilitySpec = &spec.capabilities;`）——
   这也让"读了哪份声明"在代码里看得见。
2. **新行为不能悄悄改变旧行为**。把 `identity` 接进提示词时，最容易出的事是
   "没声明身份的产品提示词变了"。因此 `compose_system_prompt_with_identity(.., None)`
   必须与旧的 `compose_system_prompt` **逐字节一致**，并有一条断言钉住它（含"空名字等同未声明"）。
3. **能力位要真的决定行为，而不是被记下来**。`capabilities.images` 如果只是存进结构体，
   就还是"看起来装上了"。现在它在 `run_turn_with_images` 里**真的拒绝**图片，
   且"无图片时照常可跑"也有断言——只挡它声明不支持的那件事。
4. **R3 的"删声明"要三处同步**。`workspace.set`/`config.update` 同时存在于
   `spec/proto/ada-coding.ext.json`、`methods.rs`（常量 + `ALL_METHODS`）、`client-ts/methods.ts`。
   只删一处会立刻被 `compat`/`verify-wiring`/`typecheck` 之一抓住——这正是多端同步守门存在的意义
   （AGENTS.md §16：加协议方法要同步三处，删也一样）。

### 13.17 W5-T1 / W5-T3 协议生成降级 + 副本校验补齐 + 协议文档（2026-10-09，已完成）

**W5-T1 判定结论：不引入 `cargo xtask gen`，正式降级为「手写 + 双向校验」**（已写入设计文档 §6.2）。

判定依据（可验证，不是"感觉不划算"）：

| 依据 | 事实 |
|---|---|
| 变更频率 | 整轮 M1–M5 计划里方法集合**只动过一次**（W5-T2 删 2 个）。生成器收益与变更次数成正比 |
| 生成器自身也要被校验 | 生成器写错 / 产物过期 / 忘了提交，都是"看起来装上了其实没接线"的经典形态 |
| 防漂移靠校验 | 三对副本的校验已能覆盖，且**已证明能红** |
| 成本 | 生成器 = 新机器 + 模板 + CI 集成 + 维护；补齐校验 = 一个测试函数 |

**产出**：

| 文件 | 内容 |
|---|---|
| `docs/agent-base-design.md` §6.2/§6.3 | 重写：记录降级决策与依据；如实登记两项**明确不做**（`gen` 四产物、`enum` 化常量 + 编译器穷尽性） |
| `crates/agent-proto/src/methods.rs` | 副本校验 #3 从**假校验**升级为 `name → value` **双向**映射比对（含 `parse_rust_consts` / `parse_ts_consts`） |
| `spec/proto/README.md`（新） | 登记表真源说明：命名规则、`x.<product>.*` 约定、字段表、四处同步、三对副本校验 |
| `docs/protocol/README.md`（新） | 协议总览：握手鉴权、三层协议、帧类型、错误码全表、审批/提问通道、客户端用法 |
| `tools/xtask/src/gates.rs` | `verify-docs` 新增守门：协议文档必须存在 + 每个事件主题必须被文档提到 |

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent-proto` | **2/2**（含升级后的副本对账） |
| **故障注入**（值写错 / 多出常量 / 缺失常量） | 三种**全部被抓**：`常量值不一致` / `多出这些常量` / `缺失这些常量`；还原后绿 |
| **故障注入**（删文档 / 改事件主题） | 两种**全部被抓**：`协议文档 ... 缺失` / `未提及事件主题 evt.card.updated`；还原后 60 条 |
| `verify-docs` | **仍为 60 条**（新文档零新增违约），且新增 `协议文档已覆盖全部 3 个事件主题` |
| `cargo test --workspace` | **284 passed / 0 failed**；零编译警告 |
| 全部门禁 | `verify` / `verify-wiring` / `verify-spec` / `compat` / `verify-archive` 全绿 |

**W5-T1/T3 期间的五条记录**：

1. 🔴 **"看起来校验了"比"没校验"更危险**。旧断言是
   `ts_content.contains(&format!("'{}'", m))`——它长得像校验，实际有三个洞：
   **单向**（TS 多出常量不报）、**子串匹配**（注释里出现也算过）、
   **不校验 name↔value 映射**（把 `CONFIG_GET` 的值改成另一个存在的方法照样通过）。
   判定一个断言是否有效，唯一可靠的方法是**注入故障看它会不会红**——本轮做了 5 次。
2. **降级要写进设计文档，不能只写在计划里**。R3 说"实现或删除声明"，对**设计承诺**同样适用：
   设计文档 §6.2 原先写着 `cargo xtask gen` 产四份产物，如果不改，它就变成一句
   永远不兑现的承诺。现在 §6.2 明确写了"决策更新"与两条**明确不做**。
3. **`find('[')` 会命中类型里的那个 `[`**。解析 `pub const ALL_EVENTS: &[&str] = &[...]` 时，
   第一次实现取的是**类型**里的 `[`（`&[&str]`），于是区域里只有 `&str`，一个事件都解析不出。
   门自己报了"解析不出…检查无效"——**这条防御性断言救了它**：
   解析失败时报错而不是静默通过，否则这个门就是空转的。
4. **`ALL_EVENTS` 里是常量名，不是字面量**。要拿事件主题得再查一次常量表
   （`EVT_STATE_SNAPSHOT` → `evt.state.snapshot`）。文档守门因此是"两跳"解析。
5. **文档守门只断言"具体且不会误报"的东西**：文档存在 + 每个事件主题被提到。
   不去解析文档结构、不数表格行数——那会变成一改格式就红的脆弱启发式
   （与 §13.15 记录 4 同一条原则）。

### 13.18 W5-T5 `FailDirection` 真实消费（2026-10-09，已完成）

**问题**：`ApprovalGate::direction()` 在 **3 个闸门实现**里都写了（`HostApprovalGate`、
`ReadonlyEnforcingGate`、测试替身），但**全仓 0 个调用点**——也就是"声明了安全默认，
代码路径上不存在"。引擎直接采信 `outcome.approved`。

**修法**：引擎成为**唯一裁决点**——按 `outcome.by` 分流：

| `outcome.by` | 含义 | 处理 |
|---|---|---|
| `User` | 有人明确点了批准/拒绝 | **原样采信**（方向不得覆盖人的决定） |
| `Policy` | 策略给出确定判定 | **原样采信** |
| `Timeout` | 等到超时也没人回答 | 按 `direction`：`Closed`→拒绝，`Open`→放行 |
| `Aborted` | 等待期间会话被中止 | 同上 |

**产出**：

| 文件 | 内容 |
|---|---|
| `crates/agent-base/src/engine/runtime.rs` | `apply_fail_direction`（纯函数）+ 引擎调用点；5 条断言（3 纯函数 + 2 端到端） |
| `crates/agent-base/src/testing/mod.rs` | `RecordingApprovalGate::with_direction`、`InMemoryToolCatalog::with_tools` |
| `crates/agent-conformance/src/invariants/inv4_fail_direction.rs` | **重写**：从重言式改为 4 场景的引擎行为断言 |
| `crates/agent-base/src/ports/scope.rs` | 明确 `Scope` **不做**可配置方向（设计决定 + 理由） |
| `tools/xtask/src/gates.rs` | `verify-wiring` 新门：`FailDirection` 必须有生产消费者 |

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent-base --lib engine::runtime` | **13/13**（含 5 条新断言） |
| `cargo test -p agent-conformance --lib inv4` | **1/1**（4 场景全过） |
| **故障注入**（把方向裁决写死成 `Closed`，不再问端口） | ✘ `FailDirection 没有被真实消费：...没有任何 .direction() 调用点`；还原后 ✔ |
| `cargo test --workspace` | **284 → 289 passed / 0 failed**；零编译警告 |
| 全部门禁 | `verify` / `verify-wiring` / `verify-spec` / `compat` / `verify-archive` 全绿 |

**W5-T5 期间的四条记录**：

1. 🔴 **重言式断言是最难发现的空转**。旧 INV-4 是
   `assert_eq!(FailDirection::default(), FailDirection::Closed)`——它断言的是**枚举的
   `#[default]` 属性**，与"系统拿不到判定依据时真的会拒绝"毫无关系。
   把引擎里所有方向裁决删掉，它照样绿。**判据：断言必须能在"实现被删掉"时变红。**
2. **"有实现"不等于"被调用"**。3 处 `direction()` 实现、0 处调用——静态审查几乎发现不了，
   因为 trait 方法有默认实现，编译器也不会提醒。加了一条**只针对这件事**的门
   （不做宽泛的"所有 trait 方法都要被调用"启发式）。
3. **故障注入要选"能编译"的形态**。第一次注入把 `.direction()` 改名成 `.direction_XX()`，
   结果 `agent-base` 编译失败 → `cargo run -p xtask` **根本没跑到门**（门依赖 agent-base）。
   改成"写死成 `FailDirection::Closed`"（能编译的回归）才证明门真的会红。
   → 教训：**验证门的时候要确认门真的执行了**，别把编译错误当成"门没红"。
4. **该给开关的地方给开关，不该给的地方写清理由**。`ApprovalGate` 需要 `direction()`
   （"等审批"确实会没有答案）；`Scope` **刻意不给**——路径越界是**确定的判定**，
   给它一个 `Open` 开关等于"拿不准时允许逃出沙箱"。这条写进了端口文档，
   而不是留一个"计划里说要接"的空头承诺。

### 13.19 W6-T5 清掉两处空壳断言 + 端口清单派生（2026-10-09，已完成）

**两处空壳的形态不同，但都是"永远通过"**：

| 位置 | 旧形态 | 为什么是空转 |
|---|---|---|
| `invariants/inv7_*.rs` | 函数体 `Ok(())` | **无条件通过**——把整条 INV-7 删掉也没有信号 |
| `ports/model.rs` | `verify_model_client_contract(client, caps)`，测试传 `caps = client.capabilities()` | **自己和自己比**——检查"客户端报告的 streaming 等于客户端报告的 streaming" |

**修法**：

| 位置 | 新形态 |
|---|---|
| INV-7 | **依赖纯度**：`agent-base` 的 `[dependencies]` 必须在白名单内（`serde`/`serde_json`/`thiserror`/`tokio`）；**源码纯度**：`agent-base/src/domain/**` 不得出现 UI/产品专有标识（`settings_open`/`gpui`/`tauri`/`card`/产品名…） |
| ModelClient 契约 | 去掉 `caps` 参数；**跑一次流**，用观察到的行为对账 `capabilities()`：声明 `streaming` 却无增量 → 错；`Done` 不恰好一个 → 错；`stop_reason` 空 → 错；两次结果不一致 → 错；提前取消仍有增量 → 错 |

**顺带修掉一处过时口径**：`verify-wiring` check D 硬编码 10 个端口名，而
`crates/agent-base/src/ports/` 实际有 **11 个** `pub trait`（漏了 `Tool`）——
"清单"与"事实"各说各话。现改为**从 `pub trait` 派生**，并加一条"解析不出端口 trait 就报错"
的防御（避免扫描失效后静默通过）。

`crates/agent-conformance/src/lib.rs` 的注释也曾写"8 个端口（… `RollbackStore`,
`PluginHost`, `ScopePolicy` …）"——`RollbackStore`/`PluginHost` **在本仓从未存在**。
已按 trait 实际列出。

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent-conformance` | **46/46**（含 INV-7 的 3 条抗体、契约的 2 条故障注入） |
| `verify-wiring` | `端口 trait 11 个（从 agent-base/src/ports 派生），缺生产实现 0 个` + ✔ 无违约 |
| `cargo test --workspace` | **289 → 293 passed / 0 failed**；零编译警告 |
| 全部门禁 | `verify` / `verify-wiring` / `verify-spec` / `compat` / `verify-archive` 全绿 |

**W6-T5 期间的四条记录**：

1. 🔴 **空壳断言有两种，都要认出来**：一种是无条件 `Ok(())`（连读都不用读），
   一种是"参数由被测对象自己提供"（**自比恒真**）。后者的迷惑性更强——它有签名、
   有比较、有错误分支，看起来是正经断言。
2. **给每条静态断言配"抗体"**。INV-7 新增的三条测试里有两条不是测被测系统，
   而是测**断言自己**：白名单收紧后必须报出 offender；扫描必须覆盖到文件。
   如果哪天有人把函数体改回 `Ok(())`，主断言仍会过，但抗体测试会红。
   → 这是对抗"空转"的通用手法。
3. **依赖白名单是一种"有意识的决定"机制**。它不是为了限制依赖数量，而是让
   "领域层新增依赖"必须**显式改表并说明理由**——否则领域层会慢慢知道外部世界。
4. **注释也会漂移，而且没有门在管**。`lib.rs` 里"8 个端口 + 三个不存在的名字"
   挂了很久没人发现。修法是**把事实放在会被校验的地方**（端口清单由门派生），
   而不是只改注释——注释改正了，下次还会漂。

### 13.20 W6-T1 / W6-T2 三个 dispatch 桩的处置（2026-10-09，已完成）

三个桩的性质不同，处置也不同（R3：**补实现或删声明**，不许留着说谎）：

| 方法 | 旧行为 | 处置 | 理由 |
|---|---|---|---|
| `subagent.resume` | 回 `{threadId, ok:true}`，**什么都没恢复**；前端还先把 `snapshot.running` 置 true | **删声明** | W4-T6 之后子智能体上下文**刻意是临时的**（一次性委派，不污染主会话）→ 语义上不存在"恢复"。要实现就得推翻两轮前的设计决定 |
| `workspace.rescan` | 回 `Ok(Null)` | **删声明** | 本仓**没有工作区缓存**可失效：`list_sessions_for_workspace` 每次直接读盘，`WORKSPACE_ENTRIES` 已是新鲜数据 → "重新扫描"没有独立语义；前端也从未调用 |
| `stats.promptChars` | 回**编造的** `{systemChars: 1200, toolSpecsChars: 800}` | **真实现** | 它确实是可测量的诊断值：系统提示词长度 ← `PromptSource`，工具规格长度 ← `ToolCatalog` 描述符 schema 之和 |

**删声明要四处同步**（AGENTS.md §16）：`spec/proto/base.json` / `ada-coding.ext.json`、
`crates/agent-proto/src/methods.rs`（常量 + `ALL_METHODS`）、`client-ts/methods.ts`、
**以及 dispatch 臂**（只删声明不删臂会变成"孤儿臂"，`verify-wiring` check A 会红）。

**前端死路径一并清掉**（W6-T1）：

- `ws-client.ts` 的 `resumeSubagent`（它先置 `running = true` 再调桩——**界面显示"正在运行"而实际什么都没发生**）；
- `Composer.tsx` 的"恢复执行"按钮与 `onResumeSubagent` prop；
- `App.tsx` 的 `onResumeSubagent={...}` 传参。

> ⚠️ 这里差点漏判：用 PowerShell 的 `**/*.tsx` 通配搜"谁传了这个 prop"时**只找到 Composer 自己**，
> 于是最初判断"没有父组件传 → 按钮本就是空转"。改用 `grep` 工具复查才发现
> `App.tsx:468` **确实传了**——按钮是**活的**，一直在对用户撒谎。
> **教训：`**` 通配不可靠，跨目录搜索一律用 `grep` 工具。**

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent-core --lib stats_prompt` | **2/2**（实测值 + 无引擎如实报错） |
| `verify-wiring` | `协议方法 72 个，match 臂 72 个，缺失 0、孤儿 0` + ✔ 无违约 |
| `bun run typecheck` | ✔ 0（前端删除后仍编译） |
| `cargo test --workspace` | **293 → 295 passed / 0 failed**；零编译警告 |
| 全部门禁 | `verify` / `verify-wiring` / `verify-spec` / `compat` / `verify-archive` 全绿 |

**W6-T1/T2 期间的四条记录**：

1. 🔴 **"桩 + 前端乐观状态"= 对用户撒谎**。`resumeSubagent` 先 `snapshot.running = true`
   再调桩——用户看到"正在运行"，实际什么都没发生，而且**永远不会有结果回来**。
   这比"按钮点了报错"糟得多：报错至少是诚实的。
2. **删声明必须连 dispatch 臂一起删**。只删声明 → 臂变成孤儿 → check A 报红；
   只删臂 → `ALL_METHODS` 里的方法没有臂 → check A 报"缺失"。
   两处必须同一次改完，这也正是 check A 双向断言的价值。
3. **"可测量"是"实现 vs 删除"的判据**。`stats.promptChars` 能真算出来（提示词 + 工具 schema
   都在手上），所以补实现；`workspace.rescan` 算不出任何**不同**的东西（没有缓存可失效），
   所以删声明。**如果"实现"只是换个写法返回同样的空值，那就该删。**
4. **`**` 通配会骗你**（见上）。本仓多处搜索都踩过：PowerShell 的 `Get-ChildItem -Recurse` /
   `Select-String -Path a/**/*.tsx` 对嵌套目录不可靠。**跨目录内容搜索一律用 `grep` 工具。**

### 13.21 W6-T3 CLI `a-da run` 真执行（2026-10-09，已完成）

**问题**：`main.rs` 的 `Run` 分支只做三件事——建一个**内存** `AgentStore`、
`create_thread`、打印两行，然后退出。命令行用户看到"创建任务会话成功"之后
**什么都没有发生**：不装配引擎、不调模型、不落盘。

**产出**：

| 文件 | 内容 |
|---|---|
| `src-tauri/src/cli_run.rs`（新） | `run_task(workspace, prompt, model)`：装配引擎 → 建会话 → 落用户消息 → 跑一轮 → 从会话读回回复；`dry_run_model`（回显替身）；`check_provider_available`（纯函数判定） |
| `src-tauri/src/main.rs` | `Run` 分支改为调 `run_task`，新增 `--dry-run`；**退出码分明**：0 成功 / 1 执行失败 / 2 缺指令 |
| `src-tauri/src/lib.rs` | `pub mod cli_run;` |
| `src-tauri/Cargo.toml` | 新增 `agent-runtime`、`agent-base` 依赖 |

**落盘证据**（真实进程，`--dry-run`）：

```
$ a-da-tauri run --workspace <tmp> --dry-run '审查代码'
a-da 命令行任务执行器，工作区: <tmp>
模式: --dry-run（不联网，验证装配与会话落盘）
会话: thread_1791523984162_1（Completed）
[dry-run] 已收到任务：审查代码
会话文件目录: <tmp>\.a-da\sessions
退出码=0

<tmp>\.a-da\sessions\ff3fdd1b9965c51edce1\thread_1791523984162_1.jsonl  (459 bytes)
  {"type":"message",...,"message":{"role":"user","content":"审查代码",...}}
  {"type":"message",...,"message":{"role":"assistant","content":"[dry-run] 已收到任务：审查代码",
   "stopReason":"stop",...,"turnDurationMs":1}}
```

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p a-da-tauri --lib cli_run` | **4/4**（落盘 + 空指令 + 路径不存在 + provider 判定） |
| 真实进程冒烟 | `--dry-run` 退出码 **0** + JSONL 落盘；空指令退出码 **2** |
| `cargo test --workspace` | **295 → 299 passed / 0 failed**；零编译警告 |
| 全部门禁 | `verify` / `verify-wiring` / `verify-spec` / `compat` / `verify-archive` 全绿 |

**W6-T3 期间的四条记录**：

1. 🔴 **"打印成功"是最廉价的假实现**。旧代码打印"创建任务会话成功: {tid}"——
   它**没有说谎**（会话确实建了），但用户会以为任务在跑。判定标准是
   **"用户看到的承诺有没有对应的副作用"**，而不是"这行日志是否属实"。
2. **`--dry-run` 是"装配自检"，不是测试专用后门**。装配（产品声明 → 工具包 → catalog →
   引擎 → 会话 → 落盘）是出错最多的一段，而它**不需要网络**就能验证。
   把"跑一轮"里的模型换成回显替身，其余全部走真实路径——用户和 CI 用同一个抓手。
3. **测试不要读开发机的真实配置**。第一版 `test_missing_provider_*` 直接跑 `run_task(None)`，
   结果本机**恰好配了 provider** → 测试真的发网络请求并**挂住 60 秒**。
   改成对纯函数 `check_provider_available` 断言：环境无关、毫秒级、语义相同。
   → 教训：**测试触碰外部状态（配置/网络/磁盘）时必须先问"这台机器上它会怎样"**。
4. **退出码要分明**。0 成功 / 1 执行失败 / 2 用法错误——脚本能据此分流。
   旧实现无论发生什么都是 0（`Ok(())`），调用方无法区分"跑完了"和"什么都没跑"。

### 13.22 W6-T4 回滚回执 + 编辑重发并入执行泵（2026-10-09，已完成）

**两类缺陷**：

| 缺陷 | 旧形态 | 危害 |
|---|---|---|
| `revert_*` 丢弃结果 | 三个臂都写 `let _outcome = ...` 然后回 `{ok:true}` | 界面显示"已回滚"，而**可能一个文件都没动**（`None` = 没找到检查点） |
| `THREAD_EDIT_AND_RESEND` 三连缺陷 | ① 另 spawn `while let Some(_e) = rx.recv()` 把事件抽干丢弃；② 错误只 `tracing::warn`；③ `abort_rx: None` | 界面看不到流式文本与工具卡片、出错无提示、点"停止"无效 |

**修法**：

1. **`revert_response(outcome)`**（纯函数）：`Some` → `ok:true` + `restored/deleted/skipped/invalidated`
   四份清单 + 计数；`None` → **`ok:false`** + `"什么都没回滚"`。
2. **提取执行泵 `Dispatcher::spawn_thread_loop(thread_id, first_prompt, provider_config)`**：
   把 `thread.start` 里那 150 行（事件写 store → 广播 → 排队消费 → 收尾）搬成**唯一实现**，
   两处调用。`THREAD_EDIT_AND_RESEND` 因此自动获得：事件进 store 并广播、错误进 `Error` 事件、
   `abort_tx` 注册进 `abort_senders`（`THREAD_ABORT` 能停到它）。
   返回口径也与 `thread.start` 对齐（`{accepted:true}`），并在会话已在运行时返回
   `{accepted:false, reason:"会话正在运行中"}`。

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent_core --lib server::dispatch` | **8/8**（含回执清单、`None` 不报 ok、抗体测试） |
| **抗体测试**（源码断言：生产段不得有"抽干丢弃"循环 + 必须调 `spawn_thread_loop`） | 加上后**立刻变红**——发现断言把测试自身的字面量也扫了进去；切掉测试段后绿 |
| 产品冒烟 | `已按产品声明装配真引擎：工具 24 个`（执行泵提取未破坏主路径） |
| `cargo test --workspace` | **299 → 302 passed / 0 failed**；零编译警告 |
| 全部门禁 | `verify` / `verify-wiring` / `verify-spec` / `compat` / `verify-archive` 全绿 |

**W6-T4 期间的四条记录**：

1. 🔴 **"回一个 ok 就算完"是静默失败**。`let _outcome = ...` 这个下划线本身就是信号：
   **算了但不用**。三个 revert 臂都这样——接口承诺了"回滚"，实现却把结果扔掉。
   判据：**只要出现 `let _x = <有意义的计算>`，就要问"这个值为什么没人要"**。
2. **"另一份实现"必然缺东西**。resend 那份手写版缺了三样（事件、错误、abort），
   而这三样在 `thread.start` 里都有。**它们不是三个独立的 bug，是同一个 bug 的三个症状**：
   没有共用同一条路径。提取执行泵一次修掉三个。
3. **抗体测试会抓到自己**。`include_str!("dispatch.rs")` 把测试自身的字面量也扫了进去，
   于是"不得包含 X"的断言因为**它自己写了 X** 而变红。切掉 `#[cfg(test)]` 之后的段即可
   （与 `gates.rs` 的 `production_prefix` 同一手法）。
   → 顺带说明这条抗体是**有效**的：它第一次运行就报红，而不是默默通过。
4. **改主路径要用冒烟兜底**。执行泵提取动了 `thread.start`（生产主路径）。
   单测覆盖不到"宿主启动 + 装配 + 绑定"，所以补了产品冒烟：
   `已按产品声明装配真引擎：工具 24 个`。

### 13.23 W6-T6 前端重连收口（2026-10-09，已完成）

**三处缺陷**：

| 缺陷 | 旧形态 | 危害 |
|---|---|---|
| 固定间隔重连 | `onclose` 里 `setTimeout(..., 2000)` | 服务端长期不在时被无限次等间隔敲打 |
| 无代次保护 | `connect()` 可被多处触发（`onclose` 定时器 + `setConnection`），旧连接的回调照样执行 | **多条 socket 并存**、消息重复处理、旧回调覆盖新连接的 url/token |
| 断线不清在途请求 | `onclose` 只置 `_connected = false` | `pendingRequests` 里的 Promise 无人兑现，调用方**干等 15s 超时** |

**修法**：

1. **退避策略抽成无副作用纯模块** `tauri-ui/src/client/reconnect-policy.ts`：
   `reconnectDelayMs(attempt, random?)` = `500ms * 2^attempt`，**封顶 30s**，叠加 **±20% 抖动**。
   单独成文件的原因很实际：放在 `ws-client.ts` 里的话，任何 `import` 都会执行该模块顶层
   （创建客户端实例 → 发起连接），"只想算一下退避"的脚本会**挂住进程**（踩过一次）。
2. **代次保护**：`connect()` 里 `const gen = ++this.connectionGeneration`，
   所有异步回调（`onopen`/`onmessage`/`onclose`/`onerror`）开头
   `if (gen !== this.connectionGeneration) return`。
   开新连接前把旧 socket 的**回调全部摘掉**再 `close()`——不摘的话它的关闭事件会再排一个重连定时器。
3. **断线立刻失败在途请求**：`failAllPending(reason)` → `pendingRequests.clear()` + 逐个 `reject`；
   `onclose` 与"创建失败"两条路径都调它。
4. **连接成功清零退避计数**（`onopen` 里 `reconnectAttempts = 0`）。

**证据**：

| 证据 | 结果 |
|---|---|
| 独立脚本验证退避（bun，纯模块可直接加载） | `0:500 1:1000 2:2000 3:4000 4:8000 5:16000 6:30000 8:30000`；抖动落在 ±20% 内；单调；封顶生效 |
| `verify-wiring` 新门（四项结构性断言） | `前端重连：退避（指数+封顶+抖动）、代次保护、断线清空在途请求 均就位` |
| **故障注入 ×3**（塞回固定 2000 / 去掉代次比对 / `onclose` 不调 `failAllPending`） | 三种**全部被抓**；还原后 ✔ |
| `bun run typecheck` | ✔ 0 |
| `cargo test --workspace` | **302 passed / 0 failed**；零编译警告 |
| 全部门禁 | `verify` / `verify-wiring` / `verify-spec` / `compat` / `verify-archive` 全绿 |

**W6-T6 期间的五条记录**：

1. **本仓没有前端测试运行器**（TS 测试已随旧实现整体冻结），所以这四项改用
   **源码结构性断言**放进 `verify-wiring`。判定标准没降低：**3 种故障注入必须全部被抓住**，
   否则就是"看着有门、其实不红"。
2. 🔴 **我的期望值算错了，不是代码错了**。第一次验证打印出抖动范围 `3200–4800`，
   我按"理论 3200±20%"判它错——实际 `attempt=3` 是 `500 × 2³ = 4000`，
   ±20% 正是 `3200..4800`。**先怀疑自己的算式，再怀疑被测代码**；
   这次如果直接去"修"代码，就会把正确的实现改坏。
3. **`import` 会执行模块顶层代码**。"只想算一下退避"的脚本因为导入了客户端模块
   （其顶层创建实例并连接）而**挂住进程 5 分钟**。把纯逻辑抽到无副作用模块是通用解法。
4. **门禁报红可能是"门在管别的事"**。新加的 `gates.rs` 注释里写了旧归档目录的字面量，
   结果 `verify-archive`（"主干不得引用归档"）**立刻报红**——注释也算主干内容。
   已改写措辞。**教训：在主树里写注释也要遵守归档门。**
5. **代次保护的完整性靠"回调全摘"补足**。只加 `gen` 比对还不够：旧 socket 关闭时
   仍会触发它自己的 `onclose`（那一刻 `gen` 还是当前值），于是又排一个定时器。
   必须在 `close()` 前把 `onopen/onclose/onerror/onmessage` 置空。

### 13.24 W6-T7 文档收口（2026-10-09，已完成）——**最后一条红例基线清零**

**目标**：`cargo xtask verify-docs` 的 **60 条**违约 → **0**。
判据是"主干 markdown 不得把**归档时代的布局**当**现行**路径描述"。

**违约分布（修前）**：

| 文件 | 条数 | 主因 |
|---|---|---|
| `README.md` | 41 | 全文是 TS/GPUIX 时代：`src/agent/`（22）、`src/ui/`（4）、`../gpuix`（3）、`app.tsx`（3）、`src/platform/`（3）、`bun test`（3）… |
| `docs/unfinished-features.md` | 13 | 逐条现状写的是 TS 侧文件 |
| `docs/feature-catalog.md` | 6 | 一整节"两套 UI 实现对比表 (GPUIX vs Tauri)" |

**修法**：

| 文件 | 处置 |
|---|---|
| `README.md` | **按当前实现重写**（656 → 约 330 行）：架构与依赖方向、产品声明式装配、24 个工具、一轮对话的流程、审批与取消（含 `FailDirection` 表）、子智能体隔离、检查点回滚、运行与 CLI（含 `run --dry-run` 与退出码）、代码结构、**唯一真源清单**、六条门禁、已知限制 |
| `docs/unfinished-features.md` | **重写**为 10 条**经核实**的缺口 + 明确不做项 + 工程注意 |
| `docs/feature-catalog.md` | 第四节"两套 UI 对比表"改为**单列**的 `tauri-ui` 功能清单（含实现位置）；第五节 CLI 补上 `run --dry-run` 与退出码 |

**顺带纠正的三处过时口径**（旧文档说反了）：

| 旧文档说 | 实际 |
|---|---|
| "只有 OpenAI 兼容协议" | `agent-adapter` 已实现**三家**：`OpenAiChat` / `Anthropic` / `OpenAiResponses` |
| "审批有全局三档" | 确实只有三档，但 `ApprovalGuardConfig.auto_approve` **存在却零消费者**（新登记为缺口） |
| "`bun test` 470 pass" | TS 测试已整体冻结，`bunfig.toml` 已把旧实现排除在测试发现外 |

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo xtask verify-docs` | **60 → 0**（`✔ 无违约`） |
| 三份新文档是否引入新违约 | **零**（逐轮复检：60 → 54 → 42 → 41 → **0**） |
| `cargo test --workspace` | **302 passed / 0 failed**；零编译警告 |
| 六条门禁 | `verify` / `verify-wiring` / `verify-spec` / `verify-docs` / `compat` / `verify-archive` **全绿** |
| `bun run typecheck` | ✔ exit 0 |

**W6-T7 期间的三条记录**：

1. **"文档重写"也要按证据写**。`unfinished-features.md` 的新版本里每条缺口都带
   **可复现的判据**（`grep mcp` 零命中、`with_images` 零消费者、`auto_approve` 零消费者…）。
   凭印象写的缺口清单会立刻过期——旧版就是证据。
2. 🔴 **重写时最容易引入"新的假话"**。旧 README 说"只有 OpenAI 兼容协议"，
   而代码里三家都有——照抄旧文案就等于把错误带进新文档。
   所以每一条技术断言都先 `grep`/读码核实（本轮核了协议分支、`auto_approve`、
   `with_images`、`mcp`、图片资源、xtask 子命令）。
3. **PowerShell 的 `+` 会做数值加法**。用数组字面量拼多行替换串时，
   `"a" + "b"` 被当成整数运算并报 `InvalidArgument`，结果**部分替换已落盘**、
   留下孤立的引用块。教训：多行替换用 `edit` 工具，或先在变量里拼好再替换。

### 13.25 W3-T6 握手能力位从产品声明派生（2026-10-09，已完成）

**怎么发现的**：W6-T7 更新状态列时，发现 §4 的"W3 进度 6/6 ✅"与 §5 W3 表里
`W3-T6` 的"未开始"**互相矛盾**。核实后确认：**任务确实没做完**，
进度行写错了。先把进度如实改成 5/6，再补齐任务。

**缺陷**（两处，同一根因）：

```rust
capabilities: ServerCapabilities::default(),          // 硬编码常量
product: Some(ProductInfo { id: "ada-coding",         // 硬编码产品身份
    name: "a_da 编程助手", archetype: "coding", ... }),
```

`session.initialize` 是客户端**第一个**请求，界面据此决定"要不要显示图片按钮、
回滚按钮、插件入口"。硬编码的结果是**声明的能力**与**界面看到的**各说各话——
与 P1-7（spec 字段无消费者）同一根因，只是这次消费者"假装读了"。

**修法**：

| 文件 | 内容 |
|---|---|
| `crates/agent-core/src/server/dispatch.rs` | `EngineInjection` 增 `spec: Arc<AgentSpec>`；`Dispatcher` 增 `product_spec`；`pipe_engine` 带进来；新增 `with_product_spec`；握手从声明派生 `ServerCapabilities` 与 `ProductInfo` |
| `crates/agent-host/src/lib.rs` | `build_engine_injection` 把 `spec` 一起注入 |
| `products/ada-coding/src/main.rs` | 同样带上声明 |

**如实降级**：`hooks` / `resync` **仍报 `false`**——机制未落地（hooks 点位为 0）。
**不许因为"设计里有"就报 `true`**。

**转绿证据**：

| 证据 | 结果 |
|---|---|
| `cargo test -p agent_core --lib server::dispatch` | **10/10** |
| 新断言：骨架声明（`rollback=false`、无 `plugins`）→ 握手回 `false` | ✔（`ServerCapabilities::default()` 是 `true`，**两者结果不同**，断言有判别力） |
| 新断言：产品 id/name/archetype 来自声明 | ✔ `ada-skeleton` / `骨架助手` / `assistant` |
| 新断言：无声明 → 退回 default 且**不捏造**产品身份 | ✔ |
| 既有断言（`ada-coding` 握手）改为**注入真实声明**后断言 | ✔（原先断言硬编码常量，发现不了"声明改了、握手没跟上"） |
| `cargo test --workspace` | **302 → 304 passed / 0 failed**；零编译警告 |
| 六条门禁 + 产品冒烟 | 全绿；`已按产品声明装配真引擎：工具 24 个` |

**W3-T6 期间的三条记录**：

1. 🔴 **"进度行"与"状态列"会各说各话**。§4 写 6/6、§5 某行写未开始，两者不可能同时为真。
   这类矛盾**不会自己暴露**——只有当有人真的去更新状态列时才会撞上。
   → 教训：状态汇总**必须从明细派生**（或至少逐行核对），不能凭记忆写。
2. **"读了声明"和"读了声明的值"是两回事**。P1-7 修完之后，`capabilities` 确实有了消费者
   （装配期决定装不装 `invoke_subagent`、收不收图片），但**握手仍然回常量**——
   于是同一份声明在两条路径上给出不同答案。判据：**同一个事实有几条出口，就要在每条出口上核对**。
3. **断言要选"两种实现结果不同"的样本**。用 `ada-coding`（`rollback:true, plugins:true`）
   测"是否派生"是**测不出来**的——它恰好与默认值相同。改用 `ada-skeleton`
   （`rollback:false`、无 `plugins`）后，硬编码与派生**结果不同**，断言才有判别力。


### 13.26 S1 删 991 行死代码 + 事件发射者门禁（2026-10-09，已完成）

**背景**：为"网关 = 管理平台 + agent 间交互平台 + 桥接平台"做准备，按
**删死代码 → 修分层倒置 → 拆包** 的顺序重构 `agent-core`。S1 = 删死代码。

#### ① 门禁先行：`check_event_emitters`（新增，`verify-wiring` check G）

判据：**每个 `AgentEventBody` 变体都必须在 `crates/agent-base/src/engine/**` 的生产段里有发射者**
（引擎是 `AgentEvent` 的唯一生产者，INV-6）。

这条门禁**一上来就抓到 3 个"定义了没人发"**——不是 1 个：

| 变体 | 缺口 |
|---|---|
| `QuestionAsked` | `ask_user` 不可用（P0-6） |
| `SubagentStarted` | 子智能体生命周期未上报（P1-17） |
| `SubagentFinished` | 同上 |

**为什么这类缺口能躺这么久**：消费方写好了、界面组件写好了、测试也在（喂的是**手工构造**的事件），
唯独生产路径上没有任何东西发出它。**测试全绿，功能不通。**

**豁免机制**（与 `LEGACY_ONLY_TOOLS` 同一手法）：三个变体登记在
`UNEMITTED_EVENT_ALLOW` 里，**逐项写明理由**；并且：

- **缺发射者**且不在豁免表 → 报红；
- **陈旧豁免**（已有发射者却还在表里）→ 报红，强制收敛；
- **豁免项拼错名字**（不是真实变体）→ 报红，防豁免表悄悄失效。

#### ② 删死代码：`runner/` 三个残渣文件

| 文件 | 行数 | 为何是死的 |
|---|---|---|
| `runner/builtin_tools.rs` | 436 | `execute_ask_user` / `execute_builtin_plugin_tool` 无调用者 |
| `runner/executor.rs` | 253 | `execute_tool_call` / `capture_tool_checkpoint` 无调用者 |
| `runner/prompt.rs` | 302 | `builtin_tools()` / `build_system_prompt` / `format_messages_for_model` 无调用者（后者是 `agent-base` 同名函数的**第二份实现**） |

合计 **991 行**。**不报 `dead_code` 的原因**：`runner/mod.rs` 用 `pub use` 把它们公开了——
`pub` 遮蔽死代码检测。所以同样的东西可以悄无声息地长回来 → 加了**防复活断言**。

**删除前逐条确认**（不敢凭"看起来没人用"）：
1. 外部零引用（用 `grep` 工具复查，不用 PowerShell `**` 通配——它在本仓已被证明漏文件）；
2. 行为有替代：`batch_write`/`decide`/`check_gate` 在 `agent-toolkit` 有真 `Tool` 实现与测试；
   `build_system_prompt` 由 `agent-adapter` 的 `CodingPromptSource` 取代（8 条测试）；
3. `evaluate_diff` / `manage_ponytail` **连描述符都不在**（不在 24 个工具里）——那些测试测的是死分支。

#### ③ 诚实交代：删掉的 7 个测试里有 1 个是"唯一实现"

`ask_user` 的问句流程：legacy `execute_ask_user` 是**唯一**会注册 waiter 并发 `QuestionAsked`
的实现。**这次删除没有引入回归**（该能力在新路径上本来就不通），但缺口已登记为 **P0-6**，
并由门禁豁免钉住。

#### 转绿证据

| 证据 | 结果 |
|---|---|
| `cargo test --workspace` | **304 → 299**（−7 删除的测试 +2 新增断言），**0 failed**；零编译警告 |
| **故障注入 1**：豁免项改名 | ✘ `事件变体 QuestionAsked 定义了却没有任何生产发射者` |
| **故障注入 2**：在生产段给 `SubagentStarted` 造发射者 | ✘ `陈旧豁免：SubagentStarted 已经有生产发射者了` |
| **故障注入 3**：建回 `runner/prompt.rs` | ✘ `是 W3-T4 的 legacy 残渣（991 行死代码）…请勿加回` |
| 六条门禁 | 全绿（`verify-wiring` 新增 `事件变体 12 个：有发射者 9 个、待补发射者 3 个（已登记豁免）`） |

#### S1 期间的四条记录

1. 🔴 **"有实现"≠"被调用"≠"真的做了那件事"**。我之前的审计判据是"描述符有 `Tool` 实现"
   （`ask_user` ✅ 有），所以漏掉了"实现直接返回错误"。**判据必须落到行为**——
   这次改成了"事件有没有发射者"，因为那是**机械可查**的行为证据。
2. **`pub use` 会遮蔽死代码检测**。991 行死代码零警告，就是因为 `runner/mod.rs` 把它们公开了。
   判据：**看一个 `pub mod` 的模块时，要问"外面真的有人用吗"**，而不是"编译器没说话"。
3. **故障注入必须能编译**。第一次注入我改了豁免表的元组元素，直接编译失败 → 门根本没跑到
   （上轮踩过同一个坑）。改成语义等价但语法完整的注入后才证明门有效。
4. **注入位置要落在"被判定的区域"内**。给 `SubagentStarted` 造发射者时，我把它追加在文件末尾——
   而那里已经在 `#[cfg(test)]` 之后，被 `production_prefix` 正确排除了。**门是对的，注入是错的。**
   → 教训：注入无效时，先确认注入**落在判据覆盖的范围内**。
5. 🔴 **同一个文档坑踩了第二次**：我用"替换 §13.25 的标题行"来插入 §13.26，结果是
   ① §13.26 落到了 §13.25 **前面**（顺序错），② §13.25 的**标题被吞掉**（只剩正文）。
   自检只查了"§13 段落数"，25 与 26 只差 1，**没看出异常**——是后来核对顺序才发现的。
   → 教训：**新增 §13.x 必须追加在最后一个 §13.x 之后（`## 附录 A` 之前）**，绝不用替换标题的方式；
   自检要加"**编号连续且递增**"，不能只看总数。

### 13.27 S2 断掉节点层对 UI 投影的依赖（2026-10-09，已完成）

**目标**：`approval/gate.rs` 与 `subagents/tool.rs` 不再读 `AgentStore`。

#### 实测：倒置比预想的更严重

两处调用点加起来看着有 14 处，但真正的**读取**只有两处：

| 文件 | 读什么 | 用途 |
|---|---|---|
| `approval/gate.rs` | `store.config.approval` | 决定"要不要问用户" |
| `subagents/tool.rs` | `store.provider` | 子智能体用哪个模型 |

问题不在"读了 UI 结构"，而在**`AgentStore.config` 的类型是 `agent_proto::ConfigSnapshot`——一个线上 DTO**：

```text
节点层（审批/委派） ──读──► AgentStore（UI 投影） ──含──► ConfigSnapshot（准备发给界面的那串 JSON）
```

也就是说：**节点行为在依据"发给界面的 JSON 的形状"做决定**。

#### 修法：端口 + 桥接层唯一知情者

```text
节点层 ──► NodeConfigSource（端口，crates/agent-core/src/node_config.rs）
                ▲
                └── StoreBackedNodeConfig（桥接层，server/node_config.rs）◄── AgentStore
                        ↑ 唯一知道"配置存在 AgentStore 里"的地方
```

| 新增/改动 | 内容 |
|---|---|
| `node_config.rs`（新） | `NodeConfigSource` 端口（`provider()` / `approval_mode()`）+ `FixedNodeConfig` 测试替身 |
| `server/node_config.rs`（新） | `StoreBackedNodeConfig`——**唯一**知情者；持共享句柄而非快照 |
| `approval/gate.rs` | 构造参数从 `Arc<RwLock<AgentStore>>` 换成 `Arc<dyn NodeConfigSource>`；`AgentStore` 只剩注释 |
| `subagents/tool.rs` | 同上 |
| `agent-host/src/lib.rs` | 组合根构造一次 `node_config`，同时喂给闸门与委派工具 |

**两个顺带的好处**：
1. **测试替身变小了**：原先要构造一整个 UI 投影（`AgentStore::new(...)` + 改字段），
   现在给一个 `FixedNodeConfig` 就够——这正是端口化的收益，`gate.rs` 的 `gate_with_mode` 从 6 行缩到 5 行且不再依赖 `state`。
2. **失效方向是安全的**：`StoreBackedNodeConfig` 拿不到锁时，档位倒向 **`Ask`（每次都问）**
   而不是 `Auto`（自动放行）——与 `FailDirection::Closed` 同一原则。

#### 新增门禁：`check_no_ui_store_in_node_layer`（`verify-wiring` check H）

判据：`crates/agent-core/src/{approval,subagents}/**` 的**生产段**里，**非注释行**不得出现 `AgentStore`。
注释不算依赖——注释里提 `AgentStore` 是在**解释这段历史**。

用**目录清单**（`NODE_LAYER_DIRS`）而不是文件清单：S4 拆包时这些目录整体搬进 `agent-node`，
清单跟着走；新增文件自动纳入，不用逐个登记。后续收敛 `session`/`checkpoint`/`plugins`/`skills` 时把目录名加进来即可。

**正向断言**：`NodeConfigSource` 必须有**生产实现**——否则"改用端口"这句话没有落点。
（实现与端口**同名不同路径**，所以按完整路径排除，不能按文件名。）

#### 转绿证据

| 证据 | 结果 |
|---|---|
| 节点层 `AgentStore` 代码引用 | **0 处**（`gate.rs` / `tool.rs` 剩余全是注释） |
| `cargo test --workspace` | **299 → 301 passed / 0 failed**（+2：端口读实时值、失效倒向安全侧）；零编译警告 |
| 六条门禁 | 全绿；`verify-wiring` 新增 `节点层（approval / subagents）已断 UI 投影依赖：扫了 13 个文件，0 处倒置；NodeConfigSource 生产实现 1 个` |
| **故障注入 A**：节点层加回一行 `AgentStore` | ✘ `节点层 approval/ 的 gate.rs:37 引用了 UI 投影 AgentStore——分层倒置` |
| **故障注入 B**：把实现的 `impl NodeConfigSource for` 改名 | ✘ `NodeConfigSource 没有任何生产实现——节点层「改用端口」没有落点` |

#### S2 期间的三条记录

1. **"依赖 X"与"依赖 X 的形状"是两件事**。修之前我以为问题只是"节点读了 UI 结构"；
   读了类型定义才发现 `AgentStore.config` 是**线上 DTO**——严重程度高一档。
   → 判据：**排查分层问题时，要顺着类型定义走到它的归属层**，不能停在字段名。
2. 🔴 **同名文件让"按文件名排除"失效**。我的正向断言排除 `node_config.rs`，
   结果把**实现**也排除了（端口与实现同名不同路径）→ 门报"生产实现 0 个"。
   → 教训：排除条件要按**完整路径**，尤其在"同名文件分处两层"这种刻意设计的结构里。
3. 🔴 **S1 里我复用了已存在的台账编号 `P0-5`**（台账里因此有两个 P0-5）——
   是这轮加 P0-6 时才发现的。**缺口编号是跨轮次追加的，凭记忆取号必然撞号**。
   → 处置：改为 **P0-6 / P0-7**；并把"**台账编号不得重复**"加入自检
   （对 `^\| \*?\*?(P[012]-\d+)\*?\*? \|` 去重，重复即报）。

### 13.28 S3 定 `AgentBus` 端口 + 本地实现（2026-10-09，已完成）

**目标**：把"委派"的**语义**从实现里抽出来，让"本地委派"与"经网关委派"成为
**同一语义的两个实现**——否则网关会变成**第二套委派机制**，而本仓已为"两份实现"付过代价。

```text
invoke_subagent (Tool) ──► AgentBus 端口 ──┬─ LocalAgentBus（进程内，= 重构前的行为）
                                           └─ GatewayAgentBus（S6，远端 agent 实例）
```

#### 端口为什么只有两个方法（`list_agents` + `dispatch`）

因为**今天真实存在的语义只有这些**：发现有哪些 agent、把任务交给其中一个并等它跑完（可取消）。
本仓的规矩是「实现或删声明」——**不许声明做不到的事**（P0-6 的 `ask_user` 就是"声明了却没接线"的教训）。

以下方法**刻意留到 S6**，因为一次性、临时上下文的本地实现**没有诚实的实现**：

| 方法 | 为什么现在不能定 |
|---|---|
| `send(agent_id, msg)`（多轮交互） | 本地子智能体上下文**刻意是临时的**（`EphemeralSessionStore`），跑完就没了；写成 `Unsupported` 桩就是 P0-6 那类问题 |
| `status(agent_id)`（异步查询） | 本地派活是**同一次 `await` 内**完成的，没有可查询的中间态 |
| `cancel(dispatch_id)`（按 id 取消） | 本地取消是**按 dispatch 传 cancel 令牌**（`DispatchRequest::cancel`）；按 id 取消需要网关那层的派发注册表 |

到 S6，网关实现会**同时**引入这三件事（远端 agent 可寻址、可多轮、可查询），那时才加方法——
加方法的同时两个实现都要给出**真实**行为。

#### 交付物

| 新增/改动 | 内容 |
|---|---|
| `agent_bus.rs`（新） | 端口 + `AgentHandle` / `DispatchRequest` / `DispatchOutcome`（含 `rejected()` 构造器） |
| `subagents/local_bus.rs`（新） | `LocalAgentBus`——**整体搬入**原先 `invoke_subagent` 里那段派活代码（含 W4-T3 的取消转发 `select!`），**不是重写** |
| `subagents/tool.rs` | 从 251 行降到 **~200 行**：只剩参数校验 + 回执映射；不再依赖 `SubagentManager` / `NodeConfigSource` / `CheckpointManager` / `run_subagent` |
| `agent-host/src/lib.rs` | 组合根构造 `LocalAgentBus`；S6 只需替换这一行 |

**关键设计：`details: Option<Value>` 区分"没跑"与"跑了但失败"。**
`details == None` = 未进入执行（目标不存在 / 已禁用 / 没有可用模型）→ 工具回
`ToolReceipt::error(原因)`；`Some(_)` = 执行过 → 带结构化细节。
这个区分让**回执与重构前逐字节一致**，界面与诊断也能分辨两种失败。

#### 转绿证据（**行为零变化**是这一步的出口判据）

| 证据 | 结果 |
|---|---|
| 原有工具测试（`test_missing_args_is_an_error` / `test_unknown_profile_is_an_error_not_a_silent_success` / `test_missing_parent_provider_fails_instead_of_fabricating_one`） | **全绿且未改断言** ← 行为零变化 |
| `cargo test --workspace` | **301 → 305 passed / 0 failed**（+4 本地总线测试）；零编译警告 |
| 六条门禁 | 全绿 |
| 门禁 check H 正向断言**泛化** | 从"硬编码 `NodeConfigSource`"改为 `NODE_LAYER_PORTS` 表，现覆盖 2 个端口（`NodeConfigSource`, `AgentBus`） |
| **故障注入**：把 `impl AgentBus for LocalAgentBus` 改名 | ✘ `节点层端口 AgentBus 没有任何生产实现——节点层「改用端口」没有落点` |

#### S3 期间的三条记录

1. 🔴 **链接器 OOM 是环境问题，不是代码缺陷**。全量跑时三个 test 二进制并行链接失败
   （`memory allocation of 2097152 bytes failed` + `link.exe 1102`）。
   **单独编译三个都通过**，重跑全量也通过 → 判定为瞬时资源竞争。
   → 判据：**链接期失败先单独复现**，别急着改代码。
2. **"搬移"要能证明是搬移**。`LocalAgentBus::dispatch` 里的取消转发 `select!` 循环是从
   `tool.rs` **原样搬过来**的——包括那条解释 W4-T3 的注释。证明方式不是"我看着一样"，
   而是**原有测试一条没改、全绿**。
3. **端口方法数要由语义决定，不由"将来可能需要"决定**。我最初设想 5 个方法
   （`list`/`dispatch`/`send`/`status`/`cancel`），实际只定了 2 个：
   另外 3 个在本地实现上**没有诚实的实现**。先把能做到的定下来，
   到 S6 网关带来新语义时再加——**端口加方法比留桩便宜**。


## 附录 A：缺口 → 任务反查表

| 审计项 | 任务 |
|---|---|
| 产品不起基座引擎 | W3-T1/T2 |
| 6 端口无生产实现 | W1-T1…T6 |
| 审批闸门缺失 | W1-T6 + W3 |
| 取消断链（含假接线） | W4-T1/T3 + W1-T4 |
| 三工具工厂死代码 | W2-T1 |
| 第 6 张名单 | W2-T5 |
| 孤儿方法 | W0-T3 + W5-T2 |
| 插件钩子 0 点位 | W5-T5 注 + §8 不做 |
| 子智能体白名单含假工具名 | W2-T6 |
| 插件 `is_write` 残留 | W2-T7 |
| 无执行器的 descriptor | W2-T3/T4 |
| spec 字段无消费者 | W5-T4 |
| `verify-spec`/`compat`/`gen` 缺失 | W0-T1/T2 + W5-T1 |
| golden 夹具缺失 | W3-T5 |
| 3 个 dispatch 桩 | W6-T1/T2 |
| CLI run 未实现 | W6-T3 |
| revert 返回值丢弃 / resend 吞错 | W6-T4 |
| conformance 空壳断言 | W6-T5 |
| 前端重连缺口 | W6-T6 |
| 文档漂移 | W0-T7 + W6-T7 |
| toolkit 声明与实现不一致（`patch`/`decision`） | W2-T1 |
