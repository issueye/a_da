# AGENT BASE 开发计划（含 MVP 与 TS 归档降级）

> 依据：[docs/agent-base-design.md](agent-base-design.md) v0.2（Rust-only、产品线基座）。
> 本文只给**开发计划**：MVP 边界、里程碑、任务清单、验收门、TS 归档的文件夹降级处理、排期与风险。
> 所有现状数字均为实测（命令与结果在文内标注）。

---

## 0. 计划口径

| 项 | 口径 |
|---|---|
| 目标 | 用基座**快速产出协议与规范一致的 AGENT CORE**；本计划把「基座可用」定义为 **MVP** |
| 语言 | **只 Rust**；TS 那套 agent 实现 + GPUIX 客户端**整体降级到 `archive/`** |
| MVP 时长 | **约 37 人日**（2 人并行 ≈ 4 周；1 人 ≈ 7–8 周） |
| 门 | `cargo xtask verify` + `verify-spec` + `verify-archive` + `cargo test`（合规套件） |
| 基线切换 | 现状「门二」`bun test` **800 项 / 104 文件**（实测）= `src/agent` 69 文件 + `src/ui` 29 文件 + GPUIX 根级/`platform` 6 文件 —— **归档后 TS 侧测试文件归零**，验收基线必须整体换成 `cargo test`（§5.10） |

---

## 1. MVP 定义

### 1.1 MVP 要交付什么（DoD）

1. **基座内核** `agent-base`：领域类型 + 端口 trait + 唯一引擎 + 策略（INV-1/2/3/4/5/6/8）。
2. **协议单源 + 闸**：`spec/proto/base.json` + `cargo xtask gen/verify`（Rust 常量、dispatch 臂、客户端类型、文档表格四者一致）。
3. **工具包与注册表**：`core`/`fs`/`command` 三个工具包；`ToolDescriptor` 取代现存五处名单；启动期 `validate()`。
4. **补空接线**（现状"看起来装上了"的部件，逐条处置见 §1.3）。
5. **TS 归档（文件夹降级）**：`src/agent` + `src/ui` + GPUIX 根级文件（`src/AgentWindow.tsx`、`src/icons.tsx`、`src/theme.ts`）+ `src/platform/**` + `app.tsx`/`screenshot.ts` + 18 个卫星脚本 → `archive/ts-legacy/`，并从构建/测试/CI 中摘除（§5）。
6. **合规套件核心子集** + **第一个产品 `ada-coding` 与现状等价**（现有 Tauri 客户端不改代码即可通过）。
7. **出包**：`cargo xtask ship --product ada-coding` 产出单文件 exe。
8. **第二消费者最小证明**：`products/ada-skeleton`（`core` 工具包 + `cli` 传输，**零内核改动**，≤ 200 行声明）——用它替代"生活类助手"作为 MVP 期的复用性验收（真正的第二产品放 M6）。

**MVP 完成判据（可机械执行）**

```bash
cargo xtask verify            # base 一致 + 声明↔实现双向 + 无孤儿臂/无无臂常量 → 全绿
cargo xtask verify-spec --all # 每个产品的声明都能被装配 → 全绿
cargo xtask verify-archive    # 仓库内不存在第二份引擎/宿主 → 全绿
cargo test --workspace        # agent-base 单测 + conformance + 产品测试 → 全绿
cargo xtask compat --all      # 所有产品的 base 协议哈希相等 → 全绿
cargo xtask ship --product ada-coding   # 产出单 exe，双击可用
```

### 1.2 MVP 明确不做（防蔓延）

| 不做 | 放到 |
|---|---|
| 25 个钩子点位全量落地 | M6（MVP 只做**机制 + 3 个点位**：`beforeToolCall` / `afterToolCall` / `beforeApproval`） |
| 细粒度事件（协议 §4 的 21 个 `evt.item.*` 等）、`session.resync`、`session.ping`、多客户端抢答 | M6（MVP 保留粗粒度 `evt.state.snapshot` + 补 `seq`） |
| 附件图片进模型 | M6（MVP 能力位如实回答 `images: false`） |
| 生活类助手 `ada-life` 与 calendar/reminder/web 工具包 | M6 |
| 能力协商的客户端降级逻辑 | M6 |
| 非 Windows 打包、WASM/多语言插件、插件市场 | M7+ |
| GPUIX 客户端功能开发 | 不排期（随 TS 归档冻结） |

### 1.3 现状缺陷的 MVP 处置表（**逐条必须决定"补实现"或"删声明"**）

| 现状缺陷（证据见设计 v0.2 §1.2） | MVP 处置 | 任务 |
|---|---|---|
| 审批闸门零接线 | **补实现**：引擎调用 `ApprovalGate`，`Denied` 以工具结果回模型；沿用现有 `approval.decide` 卡片协议 | M1-T4 |
| 取消断链（`run_command` / 子智能体收到 `None`） | **补实现**：`CancelToken` 贯穿到进程树（Windows `taskkill /T`）与子智能体 | M1-T5 |
| `ToolResult.terminate` 被丢弃 | **补实现**：`Termination::{ContinueTurn, EndTurn}`，引擎消费 | M1-T6 |
| 插件 `enabled` 检查顺序错误 | **补实现**：先判启用再执行（含内置插件工具） | M2-T3 |
| `max_retries` 无读取点 | **补实现**：`ModelClient` 内做 3 次指数退避（成本低，避免"幻觉配置"） | M1-T7 |
| 空壳工具 `check_gate`/`evaluate_diff`/`manage_ponytail` 恒回成功 | **删声明**（工具下线 + 能力位 false），TS 真实实现登记为 M6 移植项（`x.coding.decide`） | M2-T4 |
| `ask_user` schema 为空对象 | **补实现**：schema 与 handler 对齐 | M2-T2 |
| 附件图片被丢弃 | **如实声明**：能力位 `images:false`，握手与 UI 都如实；不做半吊子 | M3-T4 |
| 子智能体管理器注入 `None` | **补实现**：组合根注入 | M1-T8 |
| 每轮重扫插件（`prompt.rs` 2 处 + `executor.rs` 1 处） | **补实现**：`ToolCatalog` 由组合根装配一次，scope/插件事件时刷新 | M1-T3 |

**红线**：以上任一项都不许"静默带进新基座"——这正是本次重构的目的。

---

## 2. 里程碑总览

| # | 里程碑 | 人日 | 关键产出 | 验收门 |
|---|---|---|---|---|
| **M0** | 规范与闸先行（不碰引擎） | 5 | `spec/proto/base.json`、`tools/xtask`（`gen`/`verify`/`verify-spec`/`verify-archive`）、冻结回放夹具 | `gen` 产出的 Rust 常量与现 `protocol/methods.rs` 等价；`verify` 报出今日漂移（**预期红**，作为红例基线） |
| **M1** | 内核抽出 | 8 | `agent-base`（domain/ports/engine/policy/testing）、`agent-runtime` 组合根、`agent_core` 变 facade | 现有 Rust 行为不回归；冻结回放测试全绿；事件名与负载兼容（前端不动） |
| **M2** | 工具包与注册表 | 8 | `agent-toolkit{core,fs,command}`、`ToolDescriptor`、`validate()`、删五处名单 | §1.3 的补实现项全部落地并有断言；`is_readonly_tool` 等 5 处名单消失 |
| **M3** | 协议单源 + 投影 | 5 | 生成物替换手写；`snapshot` 补 `seq`；`UiSnapshot` 出领域；客户端类型生成 | `verify` / `compat` 全绿；Tauri 客户端用生成类型编译通过 |
| **M4** | **TS 归档（文件夹降级）** | 5 | `archive/ts-legacy/**`、插件 SDK 冻结、构建/测试/CI 摘除、兜底删除 | `verify-archive` 全绿；`bun test` 不再覆盖归档；Tauri 客户端与 `agent_core.exe` 联调通过 |
| **M5** | 合规套件 + 等价 + 出包 | 6 | `agent-conformance` 全量、`ada-coding` 产品、`ada-skeleton` 第二消费者、`ship` | MVP 完成判据 6 条命令全绿 |
| **M6+** | 钩子全量 / 细粒度事件 / 图片 / `ada-life` / decision 移植 | — | 见附录 B | — |

依赖关系：`M0 → M1 → M2 → M3 → M4 → M5`；M2/M3 内部子任务可并行；**M4 必须在 M3 之后**（客户端类型生成物就绪，归档才不会砸掉 Tauri 客户端的类型来源）。

---

## 3. 逐里程碑任务清单

### M0 规范与闸先行（5 人日）

| ID | 任务 | 产出 | 验收 | 人日 |
|---|---|---|---|---|
| M0-T1 | 把现 Rust 实现反推为 spec：76 个方法常量 + 3 个事件主题 + 错误码表 + 能力位登记 | `spec/proto/base.json` | `xtask gen` 产出的 `methods.rs` 与仓库现文件**逐行等价**（先冻结，再改） | 2 |
| M0-T2 | 写 `tools/xtask`：`gen` / `verify` / `verify-spec` / `verify-archive` / `ship` 骨架 | `tools/xtask/**` | `verify` 在今日代码上报出：**10 个 Rust-only 方法、2 个无 dispatch 臂常量（`workspace.set`/`config.update`）**（预期红） | 2 |
| M0-T3 | 冻结回放夹具：脚本化模型（delta/tool_call 序列）→ 现有 Rust 引擎的事件序列，存为 golden JSON | `crates/agent-conformance/golden/*.json` | 夹具在**当前** `agent_core` 上跑通（作为"未回归"的基准） | 1 |
| M0-T4 | 协议扩展命名规则与 `x.<product>.*` 约定落到 spec schema | `spec/proto/README.md` | 现有 `fs/change/plugin/provider/debug` 在 spec 中标为 `x.coding.*` 别名 | 0 |

### M1 内核抽出（8 人日）

| ID | 任务 | 产出 | 验收 | 人日 |
|---|---|---|---|---|
| M1-T1 | `agent-base`：domain（`Thread/Turn/Message/ContentPart/ToolDescriptor/ToolReceipt/FailDirection/AgentError`） | `crates/agent-base/src/domain/**` | 关键词门：无 `Path`/`workspace`/`a_da`/`std::fs` | 2 |
| M1-T2 | 端口 trait（无默认实现）+ `AgentRuntime::run_turn` 引擎 | `ports/**`、`engine/**` | 引擎单测：无工具调用即结束、取消即 `TurnFinished(Aborted)`、每轮恰好一个收尾 | 2 |
| M1-T3 | `ToolCatalog` 装配一次（替换每轮扫盘） | `agent-runtime` 装配 | 插件扫描只发生在启动/scope 变更/插件事件；有计数器断言 | 1 |
| M1-T4 | 审批接线 | `ApprovalGate` + 现有 `approval.decide` 协议 | 断言"每次受约束调用前被调一次"；`Denied` 以工具结果回模型；`answeredBy` 如实 | 1 |
| M1-T5 | 取消贯穿（含子进程与子智能体） | `CancelToken` | 断言：取消后 `run_command` 子进程被终止（Windows 进程树）、子智能体停止产出 | 0.5 |
| M1-T6 | `Termination` | 引擎消费 terminate | 断言：`EndTurn` 时整轮结束且只剩一个 `TurnFinished` | 0.5 |
| M1-T7 | 模型重试 | `ModelClient` 内 3 次退避 | 断言：HTTP 5xx 重试次数与最终错误如实上报 | 0.5 |
| M1-T8 | 组合根 | `ProductBuilder` + `agent_core` facade | `src-tauri` 与 `scripts/build.ts` 零改动编译通过 | 0.5 |

### M2 工具包与注册表（8 人日）

| ID | 任务 | 产出 | 验收 | 人日 |
|---|---|---|---|---|
| M2-T1 | `ToolDescriptor` + `validate()` | 注册表 + 启动期校验 | 五处名单（`tools/mod.rs:13-36`、`approval/types.rs:15-18`、`executor.rs:59-66/88-95`、`subagents/types.rs:44`、`plugins/types.rs:44`）全部删除；`validate` 复查五类消费者 | 3 |
| M2-T2 | `core` 工具包（`todo`/`ask_user`/`finish`） | `agent-toolkit/core` | `ask_user` schema 与 handler 一致（参数级断言）；`ask_user` 在无事件通道时**报错而非永久挂起** | 2 |
| M2-T3 | `fs` + `command` 工具包（含 `enabled` 顺序修复、`batch_*` 走 `rollback` 策略） | `agent-toolkit/{fs,command}` | 批量写每个目标都有回滚记录（现状**不拍**，`builtin_tools.rs:156` 的 `_checkpoint_mgr` 未使用） | 2 |
| M2-T4 | 空壳工具下线 | spec/工具表 | `check_gate`/`evaluate_diff`/`manage_ponytail` 不出现在任何产品目录里；M6 移植项已登记 | 0.5 |
| M2-T5 | 插件工具（第三方/工作区）接入注册表 | `PluginHost` 端口 | 未启用插件**不可执行**（顺序修复的回归断言） | 0.5 |

### M3 协议单源 + 投影（5 人日）

| ID | 任务 | 产出 | 验收 | 人日 |
|---|---|---|---|---|
| M3-T1 | 生成器产出四份产物 + `verify` | `agent-proto` 生成物 | 四者集合相等；孤儿臂/无臂常量均为 0 | 1.5 |
| M3-T2 | `snapshot` 补 `seq`；投影层抽出（`ClientSnapshot`/`Item` 保持兼容） | 投影模块 | Tauri 客户端不改代码通过；`UiSnapshot` 不再进领域类型 | 1.5 |
| M3-T3 | 客户端类型生成（`crates/agent-proto/client-ts/**`） | 生成物 | `tauri-ui` 的 `types/index.ts` 改为 re-export 生成物（消除第三份手写 DTO） | 1 |
| M3-T4 | 能力位如实回答（`images:false` 等） | 握手返回 | 客户端能读到 `images:false` 并按此隐藏附件入口（或明确提示） | 1 |

### M4 TS 归档（文件夹降级）（5 人日）—— 详见 §5

| ID | 任务 | 人日 |
|---|---|---|
| M4-T1 | 插件 SDK 冻结（把插件契约从 TS 类型抽成文档 + 单文件模板） | 1.5 |
| M4-T2 | 归档冻结：`git tag archive/ts-legacy-final` + 写 `archive/README.md` | 0.5 |
| M4-T3 | `git mv` 迁移到 `archive/ts-legacy/**`（含 18 个卫星脚本） | 1 |
| M4-T4 | 摘除构建/测试/CI/依赖（`package.json`、`tsconfig.json`、`bunfig.toml`、`scripts/build.ts`、`binary-check` 等） | 1 |
| M4-T5 | 删除 legacy 兜底（`app.tsx --host`、`in-process.ts`、`A_DA_FORCE_LEGACY_HOST`、`kernel`/`compiler` shim） | 0.5 |
| M4-T6 | `verify-archive` 五条断言 + 归档回归全绿 | 0.5 |

### M5 合规套件 + 等价 + 出包（6 人日）

| ID | 任务 | 人日 |
|---|---|---|
| M5-T1 | `agent-conformance`：8 个端口契约 + 8 条跨端口不变量 | 3 |
| M5-T2 | `products/ada-coding`（声明 + 生成物 + conformance 引用） | 1 |
| M5-T3 | `products/ada-skeleton`（`core` + `cli`，≤200 行声明）—— 复用性验收 | 0.5 |
| M5-T4 | `xtask ship`（沿用现 `scripts/build.ts` 的 PE 子系统补丁等步骤） | 1 |
| M5-T5 | 文档：`docs/protocol/*.md` 生成物 + README/AGENTS.md 更新（去掉 GPUIX/jiti/`bun run link`） | 0.5 |

---

## 4. 验收门与命令

| 门 | 命令 | 期望 | 失败时怎么办 |
|---|---|---|---|
| 协议一致 | `cargo xtask verify` | base 各产品哈希相等、无孤儿臂、生成物集合相等 | 修 spec 或生成器，**不许手改生成物** |
| 声明一致 | `cargo xtask verify-spec --all` | 声明↔实现双向无差 | 补适配器或删声明（§1.3 的口径） |
| 归档 | `cargo xtask verify-archive` | 5 条断言全绿（§5.8） | 见 §5.8 |
| 合规 | `cargo test --workspace` | 端口契约 + 跨端口不变量全绿 | 修实现，**不许放宽断言**（放宽必须走 INV-10 记录） |
| 跨产品 | `cargo xtask compat --all` | base 协议哈希相等；L1/L2 并集无冲突 | 冲突即为"产品改内核"，退回声明层 |
| 出包 | `cargo xtask ship --product ada-coding` | 产出单 exe；`--host` 与 UI 角色都能起 | — |
| 回归（Rust 侧） | `cargo check --all-targets` + 冻结回放 | 与 M0 冻结的 golden 事件序列一致 | — |
| 归档后 TS 侧 | `npm --prefix tauri-ui run build`（前端类型+构建） | Tauri 前端类型过、构建过（根 TS 树已无测试） | — |

> 现状提醒：`bun test` 全量是 **800 项 / 104 文件**（实测），分布为 `src/agent` **69 文件 / 599 项**、
> `src/ui` **29 文件**、GPUIX 根级与 `src/platform` **6 文件**（`src/assets.test.ts`、`src/theme.test.ts`、
> `src/AgentWindow.test.tsx`、`src/platform/{clipboard,explorer,notification}.test.ts`）。
> 这三组**全部属于归档范围**，因此 **归档后 TS 侧没有任何测试文件**；
> **MVP 的"门二"从 `bun test` 整体切换到 `cargo test --workspace`**，`bun test` 退出门禁、仅作临时手段。

---

## 5. TS 归档专项：文件夹降级处理

### 5.1 为什么必须"整套"降级，而不是只挪 `src/agent`

实测（`git grep` 结果，非测试文件）：

- `src/ui/**` 有 **30 处 / 17 个文件**的 import 指向 `src/agent/**`：`agent/store`（状态单例）、`agent/types`、`agent/config`、`agent/stats`、`agent/patch`、`agent/home`、`agent/subagents/types`、`agent/plugins/*`、`agent/compact/policy`、`agent/prompts/template`；`src/ui/client/*` 还直接 import `agent/host/{main,emitter,dispatch,server}`。
- `src/` 根级还有 **GPUIX 客户端资产**：`AgentWindow.tsx`(125 行) + `AgentWindow.test.tsx`、`icons.tsx`(114)、`theme.ts`(268) + `theme.test.ts`、`assets.test.ts`；`src/platform/**` 8 个文件（`clipboard`/`explorer`/`notification`/`win32`/`init`…）被 `src/ui/*.tsx` 专用（`Composer.tsx:16`、`TitleBar.tsx:14`、`main.tsx:11,18` 等），Tauri 侧零引用。
- `scripts/**` 有 **18 个卫星**耦合 TS 栈：`binary-check.ts`（import `agent/host/main` 的 `parseReadyLine`）、`extension-check.ts`、`projects-check.ts`、`drag-probe.ts`、`session-delete-check.ts`、`menu-check.ts`、`rows-check.ts`、`smoke.ts`、`settings-check.ts`、`startup-baseline.ts`、`scan-methods.ts`（`walk('src/ui')`）、`test-mode-switch.ts`、`test-regular-conversation.ts`、`window-controls.ts`、`verifier.ts`、`test-run.ts`、`build.ts`（entry `app.tsx`）、`launch-own.ts`（真窗口脚本共用的 spawn helper）。
- 根级：`app.tsx`（双角色入口）、`screenshot.ts`、`package.json`（`dev`/`start`/`build` 全指向 `app.tsx`）、`tsconfig.json`、`README.md` 大量章节。
- **`tauri-ui/**` 对 `src/agent`、`src/ui`、`src/platform` 零 import**（实测）——所以归档不会波及新客户端。

结论：`src/ui` + `src/` 根级 + `src/platform` 是 **TS 时代宿主的客户端**，它依赖 TS `store` 单例与 TS 纯函数，无法在 `src/agent` 归档后独立存在。
→ **归档 = 整套 TS 栈**：`src/agent/**` + `src/ui/**` + `src/` 根级 GPUIX 文件 + `src/platform/**` + `app.tsx` + `screenshot.ts` + 18 个卫星脚本 → `archive/ts-legacy/**`。
保留：`ts_engine/**`（插件运行时，独立 crate）、`tauri-ui/**`（新客户端）、`src-tauri/**`（宿主）、`agent_core` 及其后继 crates。
归档后 `src/` 树只余 `shared/protocol`（随后被生成物取代）——**Rust 侧成为仓库唯一实现**。

### 5.2 归档范围分类表

| 现状路径 | 处置 | 目标/说明 |
|---|---|---|
| `src/agent/**`（92 个源文件 + 69 个测试文件） | **归档** | `archive/ts-legacy/agent/**` |
| `src/ui/**`（含 `client/`、`platform/`、29 个测试文件） | **归档** | `archive/ts-legacy/ui/**` |
| `src/AgentWindow.tsx`、`src/icons.tsx`、`src/theme.ts`（+ `AgentWindow.test.tsx`、`theme.test.ts`、`assets.test.ts`） | **归档** | `archive/ts-legacy/gpui/**` |
| `src/platform/**`（8 文件：`clipboard`/`explorer`/`notification`/`win32`/`init`…，含 3 个测试） | **归档** | `archive/ts-legacy/platform/**`（Tauri 侧零引用） |
| `app.tsx` | **归档**（作为历史入口） | `archive/ts-legacy/entry/app.tsx`；根目录不再有它的消费者 |
| `screenshot.ts` | **归档** | `archive/ts-legacy/entry/screenshot.ts` |
| `scripts/` 中 18 个 TS 栈卫星（§5.7） | **归档**（保住考古能力） | `archive/ts-legacy/scripts/**` |
| `scripts/` 其余（如 `make-icon.tsx`） | 视依赖保留或归档 | 见 §5.7 |
| `src/shared/protocol/*.ts` | **删除** | 改由 `crates/agent-proto/client-ts/**` 生成 |
| `ts_engine/**` | **保留** | 插件运行时适配器（M6 可搬进 `crates/agent-adapter/plugin-ts`） |
| `kernel/`、`compiler/` shim（`agent_core/src/{kernel,compiler}/mod.rs`） | **删除** | 纯 `pub use ts_engine::*` 转发 |
| `tauri-ui/src/types/index.ts` | **改造** | 改为 re-export 生成类型（M3-T3） |
| `tests/`（若后续有） | — | — |

### 5.3 目标目录树

```
archive/
  README.md                     # 归档说明：冻结时间、tag、为何归档、如何临时复活（独立 worktree）
  ts-legacy/
    agent/                      # 原 src/agent/**
    ui/                         # 原 src/ui/**
    gpui/                       # 原 src/AgentWindow.tsx、icons.tsx、theme.ts 及其测试
    platform/                   # 原 src/platform/**
    entry/                      # 原 app.tsx、screenshot.ts
    scripts/                    # 原 18 个卫星脚本
    docs/                       # 归档时的 README 快照（可选）
```

`archive/README.md` 必须写清：① 这是**冻结的 TS 实现**，不作为参考设计、不被任何构建/测试引用；
② 复活办法：`git worktree add ../ts-legacy archive/ts-legacy-final` + `bun install && bun run dev`（需 gpuix 本地链接）；
③ 若为考古某段逻辑（例如 decision 引擎）而读它，**移植到 Rust 前先登记 M6 任务**。

### 5.4 前置：插件 SDK 冻结（M4-T1，必须在 `git mv` 之前）

现状：插件契约**只存在于 TS 类型里**——`src/agent/plugins/types.ts`、`src/agent/tools/builtin-plugins/types.ts`、
`src/agent/core/events.ts`（25 个钩子点位）。而 Rust 沙箱执行插件的方式是：

> `PluginSandbox::load_and_transpile` 读**单个文件** → oxc 转译 → `PureTsRuntime` 求值 → 注入 API 对象
> `{ trace, workspace, registerTool, registerSkill, registerPrompt }`（`agent_core/src/plugins/sandbox.rs:14-107`）

即：**插件是"单文件 + 注入 API"**（仓库内置插件因 `import '../../core/events'` 等深度耦合，**本来也无法被沙箱执行**，
Rust 侧对它们有原生替代）。因此归档前必须：

1. 把插件契约冻结为**语言中立文档 + 版本号**：`docs/plugin-sdk/v1.md`（导出形态、注入 API、工具/技能/提示词声明、钩子形态与超时语义、能力位）。
2. 提供 SDK 类型包 `plugin-sdk/`（纯类型，无 agent 实现依赖），第三方插件可 import。
3. 更新 `plugin.createTemplate`：生成的模板**只依赖注入 API + SDK 类型**，不再 import `src/agent/**`。
4. 归档前跑一次"工作区插件冒烟"：临时目录放一个模板插件 → Rust 沙箱 `inspect_plugin` + `call_tool` 通过。

**没做完 SDK 冻结就不许 `git mv`**——否则第三方插件作者的契约来源会随归档一起消失。

### 5.5 执行步骤（6 步，可回滚）

```bash
# 0. 前置：确认 M0–M3 完成、verify/compat 全绿、SDK 冻结完成（§5.4）
# 1. 冻结点
git tag archive/ts-legacy-final
# 2. 迁移（保留 git 历史）
mkdir -p archive/ts-legacy/{agent,ui,gpui,platform,entry,scripts}
git mv src/agent            archive/ts-legacy/agent
git mv src/ui               archive/ts-legacy/ui
git mv src/AgentWindow.tsx src/AgentWindow.test.tsx src/icons.tsx src/theme.ts src/theme.test.ts src/assets.test.ts \
        archive/ts-legacy/gpui/
git mv src/platform         archive/ts-legacy/platform
git mv app.tsx              archive/ts-legacy/entry/app.tsx
git mv screenshot.ts        archive/ts-legacy/entry/screenshot.ts
# 18 个卫星脚本逐个 git mv（清单见 §5.7）
# 3. 删除手写 TS 协议类型（已由生成物取代）
git rm -r src/shared/protocol
# 4. 删除死 shim
git rm -r agent_core/src/kernel agent_core/src/compiler
# 5. 摘除构建/测试/CI（§5.6）
# 6. 验证
cargo xtask verify-archive && cargo test --workspace && bun run typecheck:tauri
```

### 5.6 构建 / 测试 / CI / 依赖摘除清单

| 对象 | 动作 |
|---|---|
`package.json` | `dev`/`start` → 改为 Tauri 路径（或删除，开发走 `bun run tauri:dev`）；`build` → `cargo xtask ship --product ada-coding`；`test` 不再全仓扫描（归档后 TS 侧已无测试，脚本测试按需保留）；删除 `link`（不再连 `../gpuix`）、`screenshot`
`tsconfig.json` | `include` 收敛到 `tauri-ui`（其自身有 package.json/tsconfig）；`exclude: ["archive/**", "node_modules"]`（**必须**，否则 `tsc --noEmit` 会去查归档代码）
`bunfig.toml` | `preload`（`scripts/test-preload.ts`）随 TS 测试一起失效 → 删除或改为 Tauri 侧需要；确认任何 `bun test` 都不递归进 `archive/`
CI | 门二从 `bun test`（**归档后为 0 文件**）整体切到 `cargo test --workspace`；新增 `cargo xtask verify` / `verify-spec` / `verify-archive` / `compat`；前端类型检查走 `tauri-ui` 自己的 `tsc`
依赖 | 根 `dependencies`（`eventsource-parser`、`jiti`）与 devDeps（`@types/react`、`typescript`）确认在 `tauri-ui/` 与 `archive/` 之外无引用后删除；`bun run link`（连 `../gpuix`）不再是克隆后必做步骤 → 更新 `AGENTS.md` 与 `README.md`
`scripts/build.ts` | 归档；新出包走 `xtask ship`（复用其 PE 子系统补丁逻辑）
`scripts/binary-check.ts` | 归档；替代品：Rust 侧 `xtask smoke --product ada-coding`（用应用日志当证据，AGENTS.md §17 的约束继续适用）
文档 | `README.md` 重写运行段（去掉 `app.tsx`/GPUIX/Hermes）；`AGENTS.md` 更新项目速览与「开发与验证」（去掉 GPUIX/Hermes/`bun run link`，§16/§17 改为 Rust 口径）；`docs/feature-catalog.md` 的 `src/ui` 列删掉

### 5.7 卫星脚本处置表（18 个 + 其余）

| 脚本 | 处置 | 替代 |
|---|---|---|
`binary-check.ts` | 归档 | `xtask smoke`（Rust） |
`extension-check.ts`、`projects-check.ts`、`session-delete-check.ts`、`menu-check.ts`、`rows-check.ts`、`settings-check.ts`、`window-controls.ts`、`drag-probe.ts`、`launch-own.ts` | 归档（GPUIX 真窗口自动化 + 其 spawn helper） | Tauri 侧用现有 `tauri-ui` 测试手段，不在 MVP 范围内重建 |
`smoke.ts`、`startup-baseline.ts` | 归档 | `xtask smoke` |
`scan-methods.ts`（`walk('src/ui')`） | 归档 | 被 `xtask gen/verify` 取代 |
`test-mode-switch.ts`、`test-regular-conversation.ts`、`verifier.ts` | 归档 | 走协议层测试（Rust conformance + 客户端） |
`test-run.ts` | 归档 | `cargo test` |
`build.ts` | 归档 | `xtask ship` |
`test-preload.ts`（`A_DA_HOME` → 临时目录） | 归档（TS 测试树随之消失），其语义同步到 Rust 测试装备 | `agent_base::testing` 的临时 app home |
`link.ts` | 删除（只服务 `../gpuix`，归档后无人需要） | — |
`make-icon.tsx` | 视 Tauri 打包是否要图标：要则保留，不要则归档 | — |

### 5.8 `verify-archive` 的五条断言

1. `src/**`、`tauri-ui/**`、`scripts/**`、`crates/**`、`products/**`、`src-tauri/**` 中**不存在**指向 `archive/**` 的 import（`ripgrep` 路径检查；文档 `*.md` 不在此列）。
2. 仓库内**只有一个多轮循环**：`run_turn` 仅出现在 `crates/agent-base`；非归档路径不得出现 `runAgentLoop`。
3. 非归档路径不存在 TS 宿主残留：无 `A_DA_FORCE_LEGACY_HOST`、无 `createInProcessClient`、无 `hostEntryArgs`。
4. `package.json` 的 `dev`/`start`/`build`/`test` 不再指向 `app.tsx`；`tsconfig.json` 的 `include`/`exclude` 已不含 `src/agent`、`src/ui`、`src/platform`、`src/{AgentWindow,theme,icons}`。
5. `agent_core/src/{kernel,compiler}` 已删除，且 `agent_core` 不再出现"桌面模式/双目标"入口。

### 5.9 回滚方案

- 迁移**只做 `git mv`**（不改内容）→ `git revert` 单个提交即可恢复原路径。
- 归档前打 tag `archive/ts-legacy-final`；需要临时复活时用独立 worktree（§5.3），**不在主仓恢复**。
- 风险窗口：M4 之后到 M5 出包前，若发现某能力只有 TS 实现（例如 decision 引擎），**不回滚**，而是按 §1.3 的口径**删声明 + 登记 M6 移植**（保持"不许静默"）。

### 5.10 代价与验收基线切换（必须提前知道）

| 项 | 归档前 | 归档后 |
|---|---|---|
| `bun test` | 800 项 / **104 文件**（`src/agent` 69 + `src/ui` 29 + 根级/`platform` 6） | **0 个测试文件**（整棵 TS 测试树随归档移除） |
| Rust 测试 | 现有 crate 内联测试 | `cargo test --workspace`：agent-base 单测 + conformance + 产品测试（**新增主力门**） |
| 门二定义 | `bun test` | `cargo test --workspace`（`bun test` 退出门禁） |
| 插件链路验证 | TS 侧 hook-runtime 测试（599 项里的一部分） | Rust 侧：SDK 冻结的模板插件冒烟 + `PluginHost` 合规断言 |
| 开发形态 | `bun run dev`（GPUIX 热重载） | `bun run tauri:dev`（新客户端）+ `cargo run -p ada-coding -- --host` |
| 克隆后必做 | `bun install && bun run link` | `cargo build --workspace`（不再需要 gpuix） |

**这意味着 MVP 期间测试重心整体从 TS 迁到 Rust**；M0 的冻结回放夹具就是"迁移期不丢回归线"的保险。

---

### 5.11 执行记录（2026-10-08）：已完成，含三处刻意偏差

**冻结点**：`git tag archive/ts-legacy-final`（指向迁移前的 `main`）。

**实际落地形态**（与 §5.3 的草案不同）：

```
archive/
  README.md                     # 冻结说明、复活办法、禁止事项
  ts-legacy/
    src/**                      # 原 src/ 整棵树（agent、ui、platform、shared/protocol、AgentWindow.tsx、theme.ts、icons.tsx…）
    scripts/**                  # 原 scripts/ 全部 24 个文件（除被删的 link.ts）
    app.tsx                     # 原单文件双角色入口
    screenshot.ts
    assets.d.ts / env.d.ts      # 原根级类型声明（只服务归档代码）
```

**偏差 1：不切分，整棵 `src/` 平移**。§5.3 原计划拆成 `agent/`、`ui/`、`gpui/`、`platform/` 四个子目录，
但实测归档内部的相对 import 依赖原始形状：`src/ui/main.tsx → ../AgentWindow`、`src/ui/*.test.tsx → ../agent/store`、
`src/agent/…/*.test.ts → ../../../../scripts/test-preload`。只做 `git mv src` / `git mv scripts` 才能让这些路径继续成立
（也顺带说明：**归档内部结构完整，但不再保证可运行**）。
偏差 2：**`src/shared/protocol/` 一并归档而不是删除**。计划里写"删除、由生成物取代"，但生成物要到 M3-T3 才有；
现在删掉等于抽走归档代码的最后一块拼图，所以在生成物落地前先随归档保留（M3-T3 完成后再单独删）。
偏差 3：**`scripts/` 全部归档，且根 `bunfig.toml` 重新加回**（内容只剩 `[test] pathIgnorePatterns = ["archive/**"]`）。
计划只列了 18 个卫星脚本；实测另外 5 个（`icon-window.tsx`/`make-icon.tsx`/`png-pixels.ts`/`window-probe.ts`）也全依赖 GPUix 或原生窗口，属同代产物；
`link.ts` 只服务 `../gpuix`，直接删除。重加 bunfig 是为了**实测确认** `bun test` 不再扫到归档（跑出 "No tests found"，退出码 1），
避免"文档说别跑、实际一跑 599 项"。

**同时完成的收尾**：

| 项 | 结果 |
|---|---|
| 死 shim | `agent_core/src/{kernel,compiler}` 删除，`lib.rs` 去掉两行 `pub mod`；`cargo check` 两个 crate 均通过 |
| 根配置 | 删 `tsconfig.json`（归档代码的解析入口）、旧 `bunfig.toml`；`package.json` 收敛为 `typecheck`/`test`/`verify:archive`/`tauri:*`，**deps 与 devDeps 清空** |
| 锁文件 | 根 `bun.lock` 被 `bun install` 判为空并删除（无依赖）；`tauri-ui` 继续用 `package-lock.json` |
| 新门 | `tools/verify-archive.ts`（`bun run verify:archive`）实现 §5.8 的断言，**当前全绿**；M0 并入 `cargo xtask verify-archive` |
| 插件契约 | [docs/plugin-sdk/v1.md](plugin-sdk/v1.md) 冻结（形态、注入 API、工具与返回值契约、沙箱限制、v1 缺口）——先写文档后归档，顺序与 §5.4 相反但结果一致 |
| 文档 | `AGENTS.md`（速览/开发与验证/代码结构/三条规矩 + §索引归档说明）、`README.md`（顶部过期横幅）已改；README 正文重写留 M5 |

**未做（按计划留在后续里程碑）**：`src-tauri`/`tauri-ui` 的依赖改指向 `agent-runtime`（M1）、客户端类型生成（M3-T3）、
`xtask ship`/`smoke`（M5）、README 正文重写（M5）。

**验收实测**：`bun run verify:archive` 7 条断言全绿；`cargo check --all-targets`（agent_core、src-tauri）通过；
`bun run typecheck`（tauri-ui `tsc --noEmit`）通过；`bun test` = "No tests found"（不再扫归档）。

---

## 6. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| 归档砸掉 Tauri 客户端类型来源 | 前端编译失败 | 顺序强制：**M3-T3 生成客户端类型 → M4 归档**；归档前 `bun run typecheck:tauri` 必须绿 |
| 某能力只有 TS 实现（decision 引擎、GPUIX 真窗口自动化） | 功能缺口 | MVP 明确**删声明**（能力位 false），登记 M6；不许保留"恒成功"的假实现 |
| 引擎抽出改变事件时序 | 前端行为异常 | M0 冻结 golden 事件序列 + M1 兼容翻译层；逐字段比对 |
| 注册表化暴露既有行为差异（`enabled` 顺序、批量检查点） | 用户可见行为变化 | 在 M2 单独提交 + 行为变更写进 CHANGELOG；宁可分多个小提交 |
| Windows 进程树取消实现不一致 | 停止按钮不彻底 | 复用现 `cmd_tools.rs:21-27` 的 `taskkill /F /T`，加子进程组测试 |
| 生成器与手写文件共存期漂移 | 两边都改 | 生成物加 `// @generated` 头 + `verify` 在 CI 强制；提交前必须 `xtask gen` |
| 抽象过度（基座长出产品概念） | 多产品失效 | `agent-base` 关键词门（`Path`/`workspace`/`calendar`/`patch` 即红）+ §1.1 的判据 |
| 归档后"没人知道旧实现怎么做" | 重复踩坑 | `archive/README.md` + 本计划 §1.3 处置表 + 设计 v0.2 §1 的证据索引（保留结论，不留依赖） |

---

## 7. 排期（2 人并行，单位：工作日）

| 周 | 甲（内核） | 乙（协议/闸/归档） | 里程碑 |
|---|---|---|---|
| W1 | — | spec 反推、`xtask` 骨架、golden 夹具 | M0 完成 |
| W2–W3 | domain/ports/engine、组合根 | 生成器、客户端类型 | M1 |
| W3–W4 | 审批/取消/terminate/重试接线 | `validate()` + 五处名单删除 | M1 收尾、M2 起步 |
| W4–W5 | `core`/`fs`/`command` 工具包 | 投影层 + `snapshot.seq` + 能力位 | M2、M3 |
| W5 | SDK 冻结 + 插件模板冒烟 | —— | M4 前置 |
| W5–W6 | 归档迁移 + 摘除构建/测试 | `verify-archive` + CI 切换 | **M4 完成** |
| W6–W7 | conformance 全套 + `ada-coding` | `ada-skeleton` + `ship` + 文档 | **M5 = MVP 完成** |
| W7+ | 钩子全量、细粒度事件、图片、`ada-life`、decision 移植 | — | M6+ |

---

## 8. 目录结构迁移执行记录（M1 第一批，2026-10-08）

目标结构见设计 v0.2 §3 / 计划 §9。**做法：先搬"叶子"（依赖少、无产品名词的模块），
每批都用 `cargo check --workspace --all-targets` + `cargo test --workspace` 验收；
`agent_core` 保留同名 shim，调用点一行不改**——这样搬迁与行为改动彻底分离。

### 8.1 本批已落地

| 目标 | 内容 | 来源 | 依赖 |
|---|---|---|---|
| `crates/agent-base` | `model/`（`types.rs` + `think_filter.rs`）：对话消息、流式增量、用量、供应商配置、思考标签过滤 | `agent_core/src/ai/{types,think_filter}.rs` | serde / serde_json |
| `crates/agent-proto` | `lib.rs` + `dto.rs` + `errors.rs` + `methods.rs`：JSON-RPC 帧、线上 DTO、错误码、方法常量 | `agent_core/src/protocol/*` | agent-base |
| `crates/agent-adapter` | `model/stream.rs`：三家协议的 SSE 解析 + 中止（出网 IO） | `agent_core/src/ai/stream.rs` | agent-base、reqwest、tokio |
| `crates/agent-toolkit` | `lib.rs` + `fs_tools.rs` + `cmd_tools.rs` + `sandbox.rs` + `diff.rs`：文件读写、路径围栏、命令执行、文本 diff | `agent_core/src/tools/*` | anyhow、similar、tokio |
| `products/ada-coding` | 二进制入口（原 `agent_core` 的 `main.rs`，`--host/--port/--token/--parent-pid/--workspace`） | `agent_core/src/main.rs` | agent_core、clap、tracing |
| shim | `agent_core/src/{ai,protocol,tools}/mod.rs` 三处 `pub use` 转发，历史路径 `crate::ai::*`/`crate::protocol::*`/`crate::tools::*` 全部保持可用 | 新建 | 上面四个 crate |

**依赖方向**（一条都不许反向）：`agent-base` ← `agent-proto` / `agent-adapter` / `agent-toolkit` ← `agent_core` ← `products/*`。
`agent_core/src/main.rs` 搬走后，`agent_core` 变成**纯库**；`--host` 二进制现在叫 `ada-coding`。

### 8.2 为什么本批只搬这些（被依赖卡住的部分）

| 模块 | 卡在哪 |
|---|---|
| `plugins/*` → `agent-adapter/plugin` | `plugins/types.rs` 里 `PluginItem.skills: Vec<crate::skills::SkillSummary>` 与 `capabilities` 反向依赖产品侧的 skills；先把 `SkillSummary` 提到 `agent-base` 才能搬（否则成环） |
| `session/manager.rs`、`checkpoint/*` → `agent-adapter/store` | 都直接读 `get_app_home()` 与环境变量（`A_DA_HOME`/`A_DA_CONFIG`）；要先有 `Clock` + `AppHome` 端口，否则适配器里会继续写死进程环境 |
| `session/types.rs` → `agent-base/domain` | 落盘格式与领域模型混在一起（`SessionHeader`/`SessionEntry` 是**文件格式**，`AgentMessage` 是领域）；先拆再搬，否则把 IO 语义带进基座 |
| `state/`、`runner/`、`server/`、`subagents/`、`skills/`、`approval/` | 互相直接调用具体实现（engine 还没有端口）；这批就是 M1 的主工作：先定 `ports/`，再让它们实现端口 |
| `crates/agent-runtime`、`agent-host`、`agent-conformance` | 需要 `ports/` 与 `domain/` 存在才有内容；**刻意不建空壳 crate**（空目录会腐烂） |

### 8.3 本批的验收与遗留

| 检查 | 结果 |
|---|---|
| `cargo check --workspace --all-targets` | 通过（agent-base / agent-proto / agent-adapter / agent-toolkit / agent_core / ada-coding / ts_engine / src-tauri） |
| `cargo test --workspace`（并行） | **2 个既有用例互相干扰**：`tests::test_plugin_and_skill_lifecycle`（读 `~/.a-da` 配置）与 `runner::builtin_tools::tests::test_execute_ask_user_aborted`（读全局 `question_manager` 单例）。单独跑各自通过、失败集合每次还会变 |
| `cargo test --workspace -- --test-threads=1` | 44/44 通过（`bun run test` 已改为串行） |
| `bun run verify:archive` | 全绿（扫描目录已加入 `crates/`、`products/`） |

**两条要记在 M1 账上的事**：① 那两个用例是**共享全局态**的典型症状，正是 INV-8（无隐藏全局态）要治的病；
② **Rust 测试会写用户真实的 `~/.a-da`**（TS 时代靠 `scripts/test-preload.ts` 重定向，Rust 侧没有等价物）
——M1 引入 `AppHome` 端口时，测试必须注入临时 home，这条不能只靠"串行跑"掩盖。

### 8.4 第二批：domain 拆分 + 端口落地（2026-10-08）

**目标**：把"领域模型"从"落盘格式"里拆出来，并落下 8 个端口与它们的真实实现/测试替身，
让后面 `session` / `checkpoint` / `plugins` 的搬迁有接缝可用。

| 落地 | 内容 |
|---|---|
| `agent-base/domain/message.rs` | `AgentMessage` / `ToolCallBlock`（含 serde 兼容与 3 个用例）从 `agent_core::session::types` 拆出；**领域**进基座 |
| `agent_core/session/types.rs` | 只留**落盘格式**（`SessionHeader` / `SessionEntry*` / `SessionSummary` / `CURRENT_SESSION_VERSION`），并 `pub use agent_base::domain::{AgentMessage, ToolCallBlock}` 保持旧路径 |
| `agent-base/domain/tool.rs` | `ToolCall` / `ToolDescriptor` / `Access` / `PathSelector` / `ApprovalPolicy` / `RollbackPolicy` / `Execution` / `Termination` / `ToolReceipt` / `ToolStatus`。**§1/§2/§4/§5/§6/§10/§18 在这里变成类型**；`ToolReceipt::duration_ms()` 是派生值，不存在"没填"状态 |
| `agent-base/domain/event.rs` | `AgentEvent`（`seq` + `at_ms` + `thread_id` + body）与 `AgentEventBody`（12 种）、`TurnStopReason`（含 `BudgetExhausted`：有预算必须自报，**没有隐式步数上限**） |
| `agent-base/domain/error.rs` | `FailDirection`（默认 `Closed`）、`DenialKind`、`AgentError`（含 `Unsupported`：INV-2 的显式出口） |
| `agent-base/ports/*` | `Clock` / `AppHome` / `EventSink` / `CancelToken` / `Scope` / `Tool`+`ToolCatalog` / `ModelClient` / `ApprovalGate`；**全部无默认实现**（INV-2）。`ToolCatalog::validate(consumers)` 把"声明↔实现"双向核对做成返回值 |
| `agent-base/testing` | `FixedClock` / `RecordingSink` / `NeverCancel` / `TempAppHome`（测试替身进基座，适配器与产品共用同一套契约） |
| `agent-adapter/app_home.rs` | `SystemAppHome`：**全仓唯一**读 `A_DA_HOME`/`A_DA_CONFIG`/`USERPROFILE`/`HOME` 的地方（原逻辑从 `session/manager.rs` 搬来，行为逐字一致） |
| `agent-adapter/clock.rs` | `SystemClock`：**全仓唯一**直接读系统时间的地方 |

**接线（两个端口已经在生产路径上生效，不是空壳）**：

- `agent_core::session::{app_home, set_app_home, get_app_home, get_config_path}`：`get_app_home()` 签名不变，
  内部走 `AppHome` 端口；`#[cfg(test)]` 默认指向 `temp/a_da_agent_core_test_home_<pid>`。
- `agent_core::state::{clock, set_clock, now_millis}`：`now_millis()` 内部走 `Clock` 端口。
- **`AgentStore::with_home(workspace, &dyn AppHome)`**：`new()` 委托给它。这是端口的第一个真实使用点。

**这一批顺手修掉的两个既有问题**：

1. **测试写用户真实的 `~/.a-da`**：`#[cfg(test)]` 默认 home 改为临时目录，实测用户目录不再被改动
   （此前最后一次被改是 17:56，改完后再跑测试不再触碰）。
2. **跨运行状态泄漏**：临时目录名带 PID，避免"上一轮跑剩下的 `config.json` 影响下一轮"——
   这个泄漏会伪装成随机失败（第一版没带 PID，立刻在 `test_store_config_and_session_restore`
   与 `test_dispatcher_provider_management` 上炸出来）。
3. `test_store_config_and_session_restore` 原来靠 `std::env::set_var("A_DA_HOME")` 改环境变量；
   端口被 `OnceLock` 初始化后那种做法**必然失效**。改为 `AgentStore::with_home(...)` +
   `TempAppHome` 注入——同一份语义，但不再碰进程环境、可以并行。

**自检发现的一处自身设计问题（已修）**：`AppHome` 最初用 `DataKind::{Plugins, Workspace, Checkpoints…}`
枚举分区，等于把某个产品的目录表写进基座（生活类助手没有 checkpoints/插件这些概念）。
改成 `fn dir(&self, name: &str)`：**名字由产品层决定，基座只提供具名子路径**。

**已知残留（记在 M2）**：`AgentMessage::ToolResult.patch` 是编码助手味的字段名，
但它是**已落盘的兼容键**，不能直接改名；等回执模型收敛到 `ToolReceipt.data/details` 时再谈迁移。

**验收**：`cargo check --workspace --all-targets` 通过；`cargo test --workspace -- --test-threads=1`
**91 项全绿**（`agent-base` 25 项含 8 个端口/领域用例、`agent-toolkit` 6、`agent_core` 41…）；
`bun run verify:archive` 全绿。

### 8.5 第三批：AgentRuntime 引擎落地与单测验收（M1-T2 完成，2026-10-08）

**目标**：落下单一引擎 `AgentRuntime::run_turn`、相关策略与测试替身，接通审批/取消/终止语义，
单测覆盖全部合规断言（无工具自然收尾、取消即 Aborted、恰好一个收尾、审批拒绝回模型、Termination 消费、预算自报、seq 单调）。

| 落地 | 内容 | 作用 |
|---|---|---|
| `agent-base/ports/store.rs` | `SessionStore` trait | 零 IO 的会话存储接缝（`load_messages`/`append_message`） |
| `agent-base/ports/prompt.rs` | `PromptSource` trait | 零产品名词的系统提示词源 |
| `agent-base/engine/policy.rs` | `RunPolicy` | 轮内预算（`max_steps`）、并发工具数、超时 |
| `agent-base/engine/turn.rs` | `TurnRequest` / `TurnOutcome` | 单轮请求入参与结束摘要 |
| `agent-base/engine/prompt_format.rs` | `format_messages_for_model` | 领域消息与模型消息转换纯函数 |
| `agent-base/engine/runtime.rs` | `AgentRuntime::run_turn` | 多轮流式驱动单一引擎（INV-1） |
| `agent-base/testing` | 扩展测试替身全家桶 | `ManualCancel` / `InMemorySessionStore` / `FixedPrompt` / `MockScope` / `RecordingApprovalGate` / `MockTool` / `InMemoryToolCatalog` / `ScriptedModelClient` |
| `agent-base/engine/runtime.rs::tests` | 8 个核心合规单测 | 覆盖无工具结束、取消即 Aborted、模型错误单收尾、时序先后、审批 Denied 回模型、Termination::EndTurn、预算自报、seq 单调递增 |

**断言保证验证（M1-T2 / M1-T4 / M1-T5 / M1-T6）**：
1. **无工具调用即结束**：模型仅产出文本时步数为 1，事件流完整自然结束。
2. **取消贯穿与单收尾**：启动前取消、流式中取消均恰好发出一个 `TurnFinished(Aborted)`。
3. **每轮恰好一个收尾**：任何路径（错误、取消、预算耗尽、正常完成）收尾事件数恒为 1。
4. **审批接线**：受约束工具触发 `ApprovalRequested` 与 `ApprovalGate::decide`；拒绝时以 `ToolReceipt::denied` 回模型，不抛错。
5. **终止语义**：声明 `Termination::EndTurn` 的工具执行后立即收尾，不进入下一轮大模型推理。
6. **预算自报**：达到 `max_steps` 上限后自报 `BudgetExhausted`，杜绝隐式截断。
7. **事件单调有序**：事件信封 `seq` 严格单调自增（INV-6）。

**验收**：
- `cargo check --workspace --all-targets` 通过
- `cargo test --workspace -- --test-threads=1` **99 项全绿**（`agent-base` 单测扩充至 22 项）
- `bun run verify:archive` 全绿
- `bun run typecheck` 全绿

### 8.6 第四批：模型重试（M1-T7）与组合根装配（M1-T3 / M1-T8 完成，2026-10-08）

**目标**：消除 `max_retries` 幻觉配置，在 `ModelClient` 与 `stream.rs` 内实现 3 次指数退避重试；
建立 `crates/agent-runtime` 组合根，实现 `CompositeToolCatalog`（装配一次，取代每轮扫盘）与 `ProductBuilder`。

| 落地 | 内容 | 作用 |
|---|---|---|
| `agent-adapter/model/stream.rs` | `send_http_request_with_retry` | HTTP 5xx / 429 / 连接失败时 3 次指数退避重试（500ms, 1000ms, 2000ms），4xx 快速失败，支持取消中断，消费 `max_retries` 配置 |
| `agent-adapter/model/client.rs` | `NetworkModelClient` | 真实网络模型客户端，实现 `agent_base::ports::ModelClient`（三家协议 + 重试 + 中止） |
| `crates/agent-runtime` | 新 crate 组合根 | `catalog.rs`、`spec.rs`、`builder.rs`、`lib.rs`（INV-8） |
| `agent-runtime/catalog.rs` | `CompositeToolCatalog` | 工具注册表常驻内存，装配一次取代每轮扫盘（M1-T3），带 `scan_count` 计数器验证 |
| `agent-runtime/spec.rs` | `AgentSpec` | 产品声明数据结构（identity / toolkits / capabilities / policies） |
| `agent-runtime/builder.rs` | `ProductBuilder` | 产品组合根，负责声明双向校验（`validate`）与端口装配（`build` 产出 `AgentRuntime`） |
| `agent_core/src/lib.rs` | `pub use agent_runtime as runtime;` | `agent_core` 开始作为 facade 转发组合根能力 |

**断言保证验证（M1-T3 / M1-T7 / M1-T8）**：
1. **模型重试（M1-T7）**：测试验证 HTTP 500 时重试 2 次（共 3 次请求）并如实报错；HTTP 400 时立即报错（请求 1 次），不盲目重试。
2. **工具目录装配一次（M1-T3）**：测试验证初始装配后计数为 1，任意多次查询 `descriptors` 与 `resolve` 均不递增扫描计数，只有显式 `reload` 时才重载。
3. **组合根端到端构建（M1-T8）**：测试验证从 JSON 规格解析到注入各端口装配并成功执行多轮循环，全链路打通。

**验收**：
- `cargo check --workspace --all-targets` 通过（覆盖 8 个 crates 与二进制目标）
- `cargo test --workspace -- --test-threads=1` **104 项全绿**（`agent-adapter` 3 项、`agent-runtime` 2 项）
- `bun run verify:archive` 全绿
- `bun run typecheck` 全绿

---

## 9. INV → 任务映射（谁保证哪条不变量）

| INV | 落地点 | 验收 |
|---|---|---|
| INV-1 单一引擎 | M1-T1/T2、M4-T3、`verify-archive` 断言 2 | 归档后仅 `agent-base` 有 `run_turn` |
| INV-2 端口必填 | M1-T2 + conformance「无默认实现」断言 | trait 定义扫描 |
| INV-3 注册表即真源 | M2-T1 | 五处名单删除 + `validate()` |
| INV-4 失败方向在类型里 | M1-T1、M2-T1（声明） | 断言 `direction()` 与声明一致 |
| INV-5 回执必填 | 已有（Rust）→ 纳入 conformance | 回执字段非 `Option` |
| INV-6 事件有序可重放 | M1-T2、M3-T2 | `seq` 单调 + 缺口语义 |
| INV-7 领域/投影分离 | M3-T2 | `agent-base` 无 `UiSnapshot` |
| INV-8 无隐藏全局态 | M1-T8、conformance 断言 1 | 同进程两 runtime 互不干扰 |
| INV-9 兼容在适配层 | M1-T1、M3-T3 | 领域模型无 alias |
| INV-10 能力先声明 | M0-T1、M5-T3 | `ada-skeleton` 零内核改动 |
| INV-11 协议单源 | M0-T2、M3-T1 | `verify` 全绿 |
| INV-12 归档不可复活 | M4-T6 | `verify-archive` 五条断言 |

---

## 附录 A：文件级迁移映射（归档相关部分）

| 现状 | 目标 | 方式 |
|---|---|---|
`src/agent/**`（92 源 + 69 测试） | `archive/ts-legacy/agent/**` | `git mv` |
`src/ui/**`（含 29 测试） | `archive/ts-legacy/ui/**` | `git mv` |
`src/{AgentWindow.tsx,AgentWindow.test.tsx,icons.tsx,theme.ts,theme.test.ts,assets.test.ts}` | `archive/ts-legacy/gpui/**` | `git mv` |
`src/platform/**`（8 文件，含 3 测试） | `archive/ts-legacy/platform/**` | `git mv` |
`app.tsx`、`screenshot.ts` | `archive/ts-legacy/entry/**` | `git mv` |
`scripts/{binary-check,extension-check,projects-check,session-delete-check,menu-check,rows-check,settings-check,window-controls,drag-probe,launch-own,smoke,startup-baseline,scan-methods,test-mode-switch,test-regular-conversation,verifier,test-run,build,test-preload}.ts` | `archive/ts-legacy/scripts/**` | `git mv` |
`scripts/link.ts` | —（删除） | 只服务 `../gpuix` |
`src/shared/protocol/**` | —（删除） | 被 `crates/agent-proto/client-ts/**` 取代 |
`agent_core/src/{kernel,compiler}/mod.rs` | —（删除） | 直连 `ts_engine` |
`tauri-ui/src/types/index.ts` | 改为 re-export 生成类型 | 编辑 |
`ts_engine/**` | 保留（M6 可搬 `crates/agent-adapter/plugin-ts`） | 原地 |
`agent_core/**` | → `crates/agent-base` + `agent-runtime` + 适配器；`agent_core` 保留 facade 一个版本 | 拆分 |
`src-tauri/**` | 保留；`AgentStore` 依赖改指向 `agent-runtime` | 编辑 |

## 附录 B：MVP 之后的路线（M6+）

1. **钩子全量**：补齐 25 个点位（含成对约束与超时/熔断），逐个"副作用断言"；把 `PluginCapabilities` 的能力位真正消费起来。
2. **decision 引擎移植**：把 `archive/ts-legacy/agent/tools/builtin-plugins/decision/**` 的 `decide`/`check_gate` 移植为 Rust `x.coding.decide`（遵守"绝不捏造确定性"）。
3. **细粒度事件**：拆 `evt.item.*`/`evt.card.updated` 等，补 `session.resync`/`ping`/多客户端抢答。
4. **图片与多模态**：`ContentPart::Image` 打通到三家 provider。
5. **第二真实产品 `ada-life`**：`calendar`/`reminder`/`notes`/`web` 工具包 + `user-data` scope —— 验证多产品声明式开发。
6. **`ada-assistant` 通用助手**：验证"无 fs/无 workspace"的第三形态。
7. **插件 SDK v1 发布**（多语言、WASM 沙箱可选）、非 Windows 打包。
