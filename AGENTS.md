# AGENTS.md

给在本仓库工作的 AI 智能体与协作者的注意事项。**本文件会被自动注入系统提示词**，因此只留
「接手就必须知道的事」；深度约定全文已迁至 **[docs/agent-conventions.md](docs/agent-conventions.md)**。

项目速览：**纯 Rust 微内核**（`crates/agent-base/`：零 IO 的领域层 + 11 个端口 + **唯一**多轮引擎 `run_turn`）
+ **两层拆包**（S4）：`crates/agent-node/`（节点：会话 / 审批 / 检查点 / 委派 / 插件 / 技能）
与 `crates/agent-rpc/`（桥接面：JSON-RPC 分发 + WS 宿主 + UI 投影）
+ **网关**（S5/S6）：`crates/agent-gateway/`（`ada-gateway`：AGENT 管理平台 + 交互平台 + 桥接平台）
+ **Tauri 桌面宿主**（`src-tauri/`）+ **React 前端**（`tauri-ui/`）；
`crates/ts-engine/` 是独立的 TS 执行引擎，只作插件运行时。
**TypeScript 时代的实现（Bun + GPUIX 客户端 + TS 侧 agent/宿主）已整体归档到
[`archive/ts-legacy/`](archive/ts-legacy)**：它不再是参考设计、不参与构建与测试。
设计与计划的唯一口径是 [docs/agent-base-design.md](docs/agent-base-design.md) 与
[docs/agent-base-plan.md](docs/agent-base-plan.md)（含"看起来装上了其实没接线"的逐条处置表）。
**功能缺口清单在 [docs/unfinished-features.md](docs/unfinished-features.md)**：接活前先看一眼，
别把"已知未做"当成 bug 去修；做完一项顺手划掉。
**分层与重构的执行记录在 [docs/agent-base-wiring-plan.md](docs/agent-base-wiring-plan.md)**（§13 逐条）。

## 开发与验证

```bash
cargo build --workspace     # 基座四 crate + agent-node + agent-rpc + 产品 + tauri 宿主
cargo test --workspace -- --test-threads=1   # 门二（必须串行：有共享全局态的用例）
bun run typecheck           # 门一：tauri-ui 的 tsc --noEmit
bun run verify:archive      # 归档门：主干不得引用 archive/、不得有第二份引擎
bun run tauri:dev           # 桌面客户端开发（前端热重载 + 宿主）
```

- **`typecheck` 与 `cargo test` 是两个独立的门，两个都要过**，别只跑一个。
- **Rust 测试必须串行跑**（`--test-threads=1`，`bun run test` 已带）：`test_plugin_and_skill_lifecycle`
  读 `~/.a-da` 配置，而提问链路仍走全局单例 `approval::question_manager::GLOBAL_QUESTION_MANAGER`
  （`OnceLock`），并行会互相污染（单独跑各自通过）。根因是共享全局态（INV-8）；
  提问端口化（S1a）会去掉这个单例，届时再改回并行。
- 🔴 **本机 `C:\pagefile.sys` 上限只有 2 GB**：默认并行度下编译/链接大 crate 会
  `failed to mmap rmeta ... os error 1455（页面文件太小）` 或 `link.exe 1102`。
  用 `CARGO_BUILD_JOBS=2`（或 `-j 2`，**注意 `-j` 要放在 `--` 之前**）即可。
  **这是环境限制，不是代码缺陷**——单独编译失败的 crate 能过、重跑也能过。
- 本机 `cargo` 默认 target 目录编译 `ring` 会报 MSVC `D8050`；加上
  `CARGO_TARGET_DIR=../cargo_target_ada` 复用已有缓存即可（与代码无关）。
- **Rust 测试不再写用户真实的 `~/.a-da`**：`agent_node::session::app_home()` 在 `cfg(test)` 下指向
  `temp/a_da_agent_core_test_home_<pid>`（端口化后的默认值）。要自己控制目录就用
  `AgentStore::with_home(workspace, &TempAppHome::at(dir))` 注入，**不要用 `std::env::set_var`**
  （端口是 `OnceLock`，初始化后改环境变量无效——已有用例栽在这上面）。
- **别跑 `bun test`**：TS 测试已随归档冻结，`bunfig.toml` 已把 `archive/**` 排除在 test 发现之外（跑只会得到 "No tests found"）；门是 `cargo test`。

## 代码结构

**目录结构按设计落地中**（`docs/agent-base-design.md` v0.2 §3；已搬批次见 `docs/agent-base-plan.md` §8）：

- `crates/agent-base` 基座内核（零 IO、零产品名词）：`model/`（模型面类型）、`domain/`（消息·工具描述符与回执·事件·错误与失败方向）、`ports/`（11 个端口，**无默认实现**）、`engine/`（**唯一**多轮循环 `run_turn`）、`testing/`（FixedClock/RecordingSink/TempAppHome）
- `crates/agent-proto` 线协议：JSON-RPC 帧、方法常量、线上 DTO、错误码
- `crates/agent-adapter` 适配器：`model/`（三家协议 SSE + 中止）、`app_home.rs`（**全仓唯一**读 `A_DA_HOME`/`USERPROFILE` 的地方）、`clock.rs`（**全仓唯一**读系统时间的地方）
- `crates/agent-toolkit` 工具包：文件读写、路径沙箱、命令执行、文本 diff、决策与门禁判定；`registry.rs` 是**工具元数据单一真源**
- **`crates/agent-node`（S4 拆出，5.9k 行 / 36 文件）** —— 节点：`session` / `approval` / `checkpoint` / `subagents`（含 `agent_bus.rs` 委派总线端口与 `local_bus.rs` 本地实现；端口支持**多轮续跑** `thread_id`，本地总线对它是**如实拒绝**——一次性子智能体没有可续线程）/ `plugins` / `skills` / `node_config.rs`（节点配置端口）/ `delegation_depth.rs`（委派深度端口：被派活的节点靠它知道自己「在第几层」）。**不含协议管道**（不依赖 `tokio-tungstenite`）、**不含 UI 投影**（不依赖 `AgentStore`）。`AgentBus` 的网关实现（`GatewayAgentBus`）在 **`agent-rpc`**——跨进程委派需要 WS 客户端，那属于桥接面，节点只保留端口
- **`crates/agent-rpc`（S4 拆出，5.3k 行 / 18 文件）** —— 桥接面：`server/`（`dispatch.rs` 72 个方法的 JSON-RPC 分发、`ws.rs` 宿主、`emitter.rs` 快照合帧、`fs_service.rs`）、`state/`（`AgentStore`，**给界面看的投影**）、`runner/`（引擎调用 + `AgentEvent` → `AgentLoopEvent` 投影）。**依赖方向单向：`agent-rpc → agent-node`**
- **`crates/agent-gateway`（S5 新建）** —— a-da 网关，三合一：**AGENT 管理平台**（`registry.rs` 实例注册表 / `supervisor.rs` 生命周期）+ **交互平台**（S6：`delegate.rs` 派活/**多轮续跑**/取消跨网关）+ **桥接平台**（`relay.rs` 路由与透传）+ **WEB 接入面**（S7：`auth.rs` token/作用域/Origin/**配对码**）。二进制 `ada-gateway`（打印 `A_DA_GATEWAY_READY {port}`）。**只依赖线协议 `agent-proto`**——不含引擎（INV-1）、不缓存会话状态（INV-8），由 `verify-wiring` check J 守着。🔴 **非回环 `--host` 必须配 `--token`，且必须显式 `--allow-plaintext`**（网关只提供明文 `ws://`，明文上的 token 可嗅探；反代终止 TLS 时由人确认）。两条都**拒绝启动**而不是警告——失败安全
- `crates/ts-engine` 插件运行时（Boa + oxc）；插件契约见 [docs/plugin-sdk/v1.md](docs/plugin-sdk/v1.md)
- `products/ada-coding` 产品二进制（`--host/--port/--token/--parent-pid/--workspace`）
- **`products/ada-pm`（S6 新建）** —— 项目管理助手（PM agent）：**经网关**把目标委派给 coding agent。它的能力几乎全在声明里（`capabilities.delegation = "gateway"` + `gateway.endpoint`）；**不声明 `fs` 写工具包**——角色边界（PM 的价值是拆解与分派，不是改代码）
- 所有内部调用点直连基座（`agent-proto` / `agent-toolkit` / `agent-base` / `agent-adapter`），历史 shim 与 facade 已全量下线
- 端口已接线处：`session::{app_home,set_app_home,get_app_home,get_config_path}`、`state::{clock,set_clock,now_millis}`、`AgentStore::with_home`（单元测试默认 home 在临时目录，不再碰用户真实 `~/.a-da`）
- `src-tauri` Tauri 宿主（同进程起核心服务）；`tauri-ui` React 前端（`src/client/ws-client.ts` 是协议客户端）
- `archive/ts-legacy` **只读归档**（TS 时代的 src + scripts + app.tsx）

## 三条会立刻绊倒你的规矩

- **不许引用归档**：主干任何代码/配置都不得 import 或指向 `archive/ts-legacy/**`——那会把"两份实现"的漂移重新引进来；`bun run verify:archive` 会红。
- **改 Rust 核心前先看 [docs/agent-base-plan.md](docs/agent-base-plan.md) §1.3**：审批闸门、取消贯穿、`terminate`、插件 `enabled`、空壳工具等**目前多数是"看起来装上了、其实没接线"**，那里逐条写明了处置口径（补实现 / 删声明）。
- **工具元数据已收敛为 `ToolDescriptor` 单一真源**（W2-T2 完成）：读写性（`is_write`）、审批要求、
  检查点回滚策略（`rollback`）、终止语义都在 `crates/agent-toolkit/src/registry.rs`；
  `is_write_tool` / `is_readonly_tool` 由它派生，`PLUGIN_BUILTIN_CATALOG` 也由它派生
  （`cargo xtask verify-wiring` 的 check B/C 盯着"派生"这件事）。
  历史上那五处手写名单（`lib.rs` 读写判定、`approval/types.rs` 命令工具、`executor.rs` 检查点、
  子智能体白名单、插件自述 `is_write`）**只剩插件自述一处**，且已改为按描述符校验。

## § 索引（`AGENTS.md §N` 一律指下表第 N 条，正文见 docs/agent-conventions.md）

> ⚠️ **归档说明（2026-10-08）**：§1–§15 的正文描述的是**已归档的 TS 实现**（`archive/ts-legacy/`）里的机制，
> 它们在 Rust 权威路径上多数**尚未落地**——逐条处置（补实现 / 删声明）见
> [docs/agent-base-plan.md](docs/agent-base-plan.md) §1.3。§16（协议方法多端同步）与 §18（工具结构化回执）
> 按原样继续生效；§17 的"用应用日志当证据"仍然有效，但那条 `console.log` 劫持链路本身已随 `src/platform/` 归档。

| § | 一句话警告 |
|---|---|
| 1 | 子智能体 `allowedTools` 是写死白名单、与插件表**脱钩**：新工具不同步就白装 |
| 2 | `isWriteTool` **失败安全**：只读工具漏进 `READ_ONLY` 就被当写工具，plan/只读子体拿不到；非内置插件借走只读内置名也一律算写 |
| 3 | 子智能体只拿到 `profile.systemPrompt`，**没有** AGENTS.md 与主线程系统提示词 |
| 4 | 新写工具须进 `CHECKPOINT_TOOLS` 与 `checkpointPathsOf`（批量要逐文件进快照） |
| 5 | 工具结果的 `terminate: true` 结束的是**整轮**，不是"这批" |
| 6 | 全局 sequential，但整批都声明 `executionMode: 'parallel'` 时会重叠执行 |
| 7 | 子智能体收尾的 `wakeParent()` 必须在 `runningThreadIds.delete()` 之后；离线兜底单独补 |
| 8 | 别轮询 `check_subagent`，用 `await_subagents` 挂起等待 |
| 9 | 决策插件**绝不捏造确定性**：拿不到就失败，`calibrated` 仅 Jev 引擎为 true |
| 10 | 插件生效需**多处同时登记**（READ_ONLY／目录／allowedTools），各有守门测试 |
| 11 | `LoadedPlugin` 是唯一产物；第三方插件 id 由加载器定为「scope:文件名」 |
| 12 | 钩子**成对**是硬约束，工具集**只能收窄**；两处刻意偏差别当 bug 修 |
| 13 | 审批／压缩的效力**刻意不对称**；文末另有 UI 单窗口约束 |
| 14 | 审批：**策略**归插件 `approval-guard`、**执行**归核心 `askUser`；点位顺序不能反 |
| 15 | 陷阱：钩子"声明了却没人调用"是静默失效——**断言副作用**，别断言钩子被调用 |
| 16 | 界面只认 `ui/client` 四条通道；**加协议方法要同步三处**（有守门测试，改错方向会红） |
| 17 | 打包后的 exe **收不到自动化通道**（`console.log` 被劫持进日志）——二进制检查要用应用日志当证据 |
| 18 | 工具结果是一份**结构化回执**（`status`／`duration_ms`／`started_at`／`finished_at`，读写两侧都认 snake/camel）：塞裸文本不报错，只是界面耗时与状态徽章**静默变空** |

> 兼容说明：历史上写作「`AGENTS.md` §9」「`AGENTS.md` 第 1 条」的引用，按上表第 N 条理解。
> 编号在迁出后**原样保留**，故 `docs/` 与源码注释里的既有引用无需改动。
