# AGENTS.md

给在本仓库工作的 AI 智能体与协作者的注意事项。**本文件会被自动注入系统提示词**，因此只留
「接手就必须知道的事」；深度约定全文已迁至 **[docs/agent-conventions.md](docs/agent-conventions.md)**。

项目速览：**纯 Rust 微内核**（`agent_core/`：主循环、工具执行、插件沙箱、会话与检查点持久化）+
**Tauri 桌面宿主**（`src-tauri/`，同进程起 `WsHostServer`）+ **React 前端**（`tauri-ui/`）；
`ts_engine/` 是独立的 TS 执行引擎，只作插件运行时。
**TypeScript 时代的实现（Bun + GPUIX 客户端 + TS 侧 agent/宿主）已整体归档到
[`archive/ts-legacy/`](archive/ts-legacy)**：它不再是参考设计、不参与构建与测试。
设计与计划的唯一口径是 [docs/agent-base-design.md](docs/agent-base-design.md) 与
[docs/agent-base-plan.md](docs/agent-base-plan.md)（含"看起来装上了其实没接线"的逐条处置表）。
**功能缺口清单在 [docs/unfinished-features.md](docs/unfinished-features.md)**：接活前先看一眼，
别把"已知未做"当成 bug 去修；做完一项顺手划掉。

## 开发与验证

```bash
cargo build --workspace     # 编译基座四 crate + agent_core + ada-coding + ts_engine + tauri 宿主
cargo test --workspace -- --test-threads=1   # 门二（必须串行：有 2 个用例共享全局态）
bun run typecheck           # 门一：tauri-ui 的 tsc --noEmit
bun run verify:archive      # 归档门：主干不得引用 archive/、不得有第二份引擎
bun run tauri:dev           # 桌面客户端开发（前端热重载 + 宿主）
```

- **`typecheck` 与 `cargo test` 是两个独立的门，两个都要过**，别只跑一个。
- **Rust 测试必须串行跑**（`--test-threads=1`，`bun run test` 已带）：`test_plugin_and_skill_lifecycle`
  与 `test_execute_ask_user_aborted` 分别读 `~/.a-da` 配置和全局 `question_manager` 单例，并行会互相污染
  （单独跑各自通过）。根因是共享全局态（INV-8），M1 用依赖注入根治后再改回并行。
- 本机 `cargo` 默认 target 目录编译 `ring` 会报 MSVC `D8050`；加上
  `CARGO_TARGET_DIR=../cargo_target_ada` 复用已有缓存即可（与代码无关）。
- **Rust 测试目前会写用户真实的 `~/.a-da`**（TS 时代的 `scripts/test-preload.ts` 重定向已随归档失效）：
  M1 引入 `AppHome` 端口后改为注入临时 home。
- **别跑 `bun test`**：TS 测试已随归档冻结，`bunfig.toml` 已把 `archive/**` 排除在 test 发现之外（跑只会得到 "No tests found"）；门是 `cargo test`。

## 代码结构

**目录结构按设计落地中**（`docs/agent-base-design.md` v0.2 §3；已搬批次见 `docs/agent-base-plan.md` §8）：

- `crates/agent-base` 基座内核（零 IO、零产品名词）：已落地 `model/`；`domain/`、`ports/`、`engine/`、`policy/` 待搬
- `crates/agent-proto` 线协议：JSON-RPC 帧、方法常量、线上 DTO、错误码
- `crates/agent-adapter` 适配器：已落地 `model/`（三家协议 SSE + 中止）；`store/`、`plugin/`、`scope/` 待搬
- `crates/agent-toolkit` 工具包：文件读写、路径沙箱、命令执行、文本 diff
- `products/ada-coding` 产品二进制（`--host/--port/--token/--parent-pid/--workspace`）；`agent_core` 已变纯库
- `agent_core/src/{ai,protocol,tools}/mod.rs` 是**兼容 shim**（`pub use` 转发到上面四个 crate），调用点不动
- `agent_core/src/{runner,server,state,session,plugins,subagents,skills,approval,checkpoint}` 仍是权威实现，M1 收敛
- `agent_core/src/server` —— `dispatch.rs`（JSON-RPC 分发，最大文件）、`ws.rs`（宿主）、`emitter.rs`（快照合帧）
- `src-tauri` Tauri 宿主（同进程起核心服务）；`tauri-ui` React 前端（`src/client/ws-client.ts` 是协议客户端）
- `ts_engine` 插件运行时（Boa + oxc）；插件契约见 [docs/plugin-sdk/v1.md](docs/plugin-sdk/v1.md)
- `archive/ts-legacy` **只读归档**（TS 时代的 src + scripts + app.tsx）

## 三条会立刻绊倒你的规矩

- **不许引用归档**：主干任何代码/配置都不得 import 或指向 `archive/ts-legacy/**`——那会把"两份实现"的漂移重新引进来；`bun run verify:archive` 会红。
- **改 Rust 核心前先看 [docs/agent-base-plan.md](docs/agent-base-plan.md) §1.3**：审批闸门、取消贯穿、`terminate`、插件 `enabled`、空壳工具等**目前多数是"看起来装上了、其实没接线"**，那里逐条写明了处置口径（补实现 / 删声明）。
- **工具元数据现在仍是五处名单**（`crates/agent-toolkit/src/lib.rs` 的读写判定、`approval/types.rs` 的命令工具、`executor.rs` 的检查点、子智能体白名单、插件自述 `is_write`）：改一个工具的语义要五处同步；M2 会收敛成 `ToolDescriptor`，在那之前别只改一处。

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
