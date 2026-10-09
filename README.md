# a_da

> **状态（2026-10-09）**：纯 Rust 微内核 + Tauri 桌面宿主，M1–M6 接线收口已完成。
>
> 本 README 描述**当前**实现。历史 TypeScript 实现已整体冻结，不再是参考设计、
> 不参与构建与测试。设计与计划的唯一口径是
> [docs/agent-base-design.md](docs/agent-base-design.md) 与
> [docs/agent-base-wiring-plan.md](docs/agent-base-wiring-plan.md)。

<img src="./assets/logo.svg" width="88" align="right" alt="a_da logo" />

一个**本地** AI 编码 Agent 的桌面程序：纯 Rust 内核 + Tauri 桌面壳（同进程起
JSON-RPC 宿主）+ React 前端。标志是一条 shell 提示符 `>_`——Agent 就是在工作区里
跑命令的那个东西，方块的落点正好是名字里那条下划线。

![a_da 主界面](./docs/app.png)

## 目录

- [架构](#架构)
- [它做什么](#它做什么)
- [一轮对话是怎么跑的](#一轮对话是怎么跑的)
- [审批与取消](#审批与取消)
- [子智能体](#子智能体)
- [检查点与回滚](#检查点与回滚)
- [运行](#运行)
- [代码结构](#代码结构)
- [门禁与测试](#门禁与测试)
- [已知限制](#已知限制)

---

## 架构

**微内核**：`crates/agent-base` 是零 IO、零产品名词的领域层——领域类型、11 个端口、
以及**唯一**的多轮循环 `AgentRuntime::run_turn`（INV-1）。它不知道文件系统、
不知道产品叫什么、不知道界面长什么样。

**依赖方向**（单向，无环）：

```
agent-base  →  agent-proto / agent-adapter / agent-toolkit / agent-runtime  →  agent-host
                                                                                  ↓
                                                            agent-core / products / src-tauri
```

| 层 | 位置 | 职责 |
|---|---|---|
| 领域与端口 | `crates/agent-base` | 消息 / 工具描述符与回执 / 事件 / 错误与失败方向；11 个端口；**单一引擎** |
| 线协议 | `crates/agent-proto` | JSON-RPC 帧、72 个方法常量、线上 DTO、错误码 |
| 适配器 | `crates/agent-adapter` | 三家协议 SSE（OpenAI Chat / Anthropic / OpenAI Responses）、会话落盘、提示词排版、作用域、时钟 |
| 工具包 | `crates/agent-toolkit` | 24 个工具描述符（**单一真源**）+ 文件/命令/决策/Git/项目工具 |
| 组合根 | `crates/agent-runtime` | `ProductBuilder`、产品声明（`AgentSpec`）、工具目录 |
| 宿主装配 | `crates/agent-host` | 按产品声明装配引擎 + 会话 + 审批；`run_from_spec` |
| 核心库 | `crates/agent-core` | JSON-RPC 分发、会话状态、插件/技能/子智能体管理 |
| 合规套件 | `crates/agent-conformance` | 端口契约 + 8 条跨端口不变量 + golden 回放夹具 |
| 产品 | `products/ada-coding`、`products/ada-skeleton` | 声明式产品（`agent.spec.json` + 极小 `main`） |
| 桌面宿主 | `src-tauri` + `tauri-ui` | Tauri 壳 + React 界面（`ws-client.ts` 是唯一协议客户端） |

**产品声明式装配**：`products/*/agent.spec.json` 声明 `toolkits` / `capabilities` /
`identity` / `policies`，`agent-host` 按声明装配。声明的每个字段都有**真实消费者**
（`cargo xtask verify-spec` 会逐个核对）：

| 声明字段 | 消费者 |
|---|---|
| `toolkits` | `tools_for_toolkits` → 工具目录（子智能体也走同一处装配） |
| `capabilities.subagents` | 是否装配 `invoke_subagent` |
| `capabilities.images` | 声明 `false` → 拒绝图片输入 |
| `identity.name` / `.persona` / `.locale` | 系统提示词（产品人格与语言） |
| `policies` | `RunPolicy`（步数、并发、工具超时） |

**协议单源**：`spec/proto/*.json` 是唯一手写源，另有手写副本
（`methods.rs` / `client-ts/methods.ts` / `dispatch.rs` 的 `match` 臂），
三对副本**双向校验**（`cargo xtask verify-wiring`、`compat` 与 `agent-proto` 的测试）。
详见 [spec/proto/README.md](spec/proto/README.md) 与 [docs/protocol/README.md](docs/protocol/README.md)。

> **不做代码生成**：评估后判定不划算（方法集合变更频率极低，而生成器自身也要被校验）。
> 决策与依据见 [docs/agent-base-design.md](docs/agent-base-design.md) §6.2。

---

## 它做什么

在输入框里用自然语言描述任务。Agent 在**当前工作区**内工作，模型可见的工具由
产品声明装配（`ada-coding` 为 **24 个**）：

| 工具 | 作用 |
|---|---|
| `list_files` | 列目录（跳过 `node_modules`、`.git`、`dist` 等） |
| `read_file` | 读文本文件，支持 `offset`/`limit` 按行段读；超限、二进制、目录都拒绝 |
| `search_files` | 正则搜索代码（条数上限）；支持纯文本、大小写敏感、上下文行、子目录限定 |
| `code_outline` | 按名字查函数/结构体等定义位置与签名 |
| `write_file` / `batch_write` | 新建或整体覆盖，返回 unified diff |
| `edit_file` / `batch_replace` | 精确替换（要求唯一匹配），可一次多组 |
| `run_command` | 工作区内执行 shell（默认超时；**超时/中止杀整棵进程树**，输出截断） |
| `run_background` / `check_task` / `kill_task` | dev server、watcher 这类长命令：后台启动返回任务 id，轮询状态，按需整树终止 |
| `todo` | 多步骤任务规划 |
| `project_inspect` / `inspect_project` | 项目结构与依赖概览 |
| `git_status` / `git_diff` / `git_log` | Git 只读查询 |
| `run_tests` | 跑项目测试 |
| `decide` / `check_gate` | 确定性决策路由与验收门禁判定 |
| `ask_user` | 向用户提问（选项 + 自定义答复） |
| `finish` | 声明任务完成（终止整轮） |
| `invoke_subagent` | 委派给隔离运行的子智能体 |

这份表**不是写死的清单**：`ToolDescriptor` 注册表是唯一真源，
`PLUGIN_BUILTIN_CATALOG` 由它派生，`cargo xtask verify-wiring` 会盯着"派生"这件事。

所有路径都会被解析回该会话所属工作区的根目录，任何指到外面的路径都被拒绝
（含 `run_command` 的 `cwd`）。字符串级检查之后还会 realpath 解析符号链接：
工作区内指向外面的 symlink（含 Windows junction）按**真实落点**拒绝。
唯一有意留下的口子：`run_command` 是真 shell，逃逸沙箱的命令它管不了。

---

## 一轮对话是怎么跑的

一切都在**一个** `AgentRuntime::run_turn` 里（`crates/agent-base/src/engine/`）：

1. 从 `SessionStore` 载入历史，把 `TurnRequest.user_prompt` 追加进去；
2. 向 `ToolCatalog` 要一次工具表（装配期定好，不在每轮扫盘）；
3. 向 `ModelClient::stream` 要增量流，边收边发 `AgentEvent`；
4. 有工具调用 → 过 `ApprovalGate` → 执行 → 回执作为**结构化工具结果**回给模型；
5. 没有工具调用 / 撞上预算 / 被取消 → 收敛并发出**恰好一个** `TurnFinished`。

关键性质（都有断言守着）：

- **审批拒绝不抛错**：拒绝理由作为**工具结果**回给模型（AGENTS.md §14），
  模型知道被拒了，不会以为调用成功了；
- **事件单调**：`seq` 单调递增，`ToolCallStarted` 必先于同 `call_id` 的
  `ToolCallFinished`（INV-6）；
- **工具结果是结构化回执**：`status` / `duration_ms` / `started_at` / `finished_at`，
  界面据此渲染耗时与状态徽章（AGENTS.md §18）；
- **工具顺序执行**：审批一次只该问一件事；`RunPolicy.max_parallel_tools` 可放开。

界面侧：`LoopEventBridge` 把领域事件投影成 `AgentLoopEvent`，
`dispatch.rs` 的执行泵写进会话态并广播快照。**`thread.start` 与"编辑重发"
共用同一条执行泵**（`spawn_thread_loop`）——事件只有一条通路。

---

## 审批与取消

**审批**：策略归插件 `approval-guard`，执行归核心（AGENTS.md §14）。线上表现为：

1. 引擎判定"要问" → 发出 `ApprovalRequested` → 工具卡片进入
   `status: "waiting_approval"`（带工具名与参数）；
2. 界面渲染批准 / 拒绝按钮 → 调 `approval.decide`；
3. 拒绝 → 拒绝理由作为工具结果回模型。

**失败方向**（INV-4）：`ApprovalGate::direction()` 决定"**拿不到判定依据**时往哪边倒"：

| `AnsweredBy` | 含义 | 处理 |
|---|---|---|
| `User` / `Policy` | 确定的答案 | **原样采信**（方向不得覆盖人的决定） |
| `Timeout` / `Aborted` | 没有答案 | 按 `direction`：`Closed`（默认）→ 拒绝；`Open` → 放行 |

超时默认 300s。`Scope` 的失败方向**刻意不做成可配置**：路径越界是**确定的判定**，
给它一个 `Open` 开关等于"拿不准时允许逃出沙箱"。

**取消**：`CancelToken` 贯穿传进 `ModelClient::stream` 与 `Tool::execute`。
`run_command` 会**真的杀掉整棵进程树**（不是"等命令自己结束"）。
INV-8 的断言是"**执行中**取消穿透到工具"——工具必须**观察到**取消，
而不只是"轮次最终 Aborted"。

---

## 子智能体

`invoke_subagent` 是一等 `Tool`（描述符在注册表里），把专项任务委派给**隔离运行**的
子智能体。隔离性由**装配**保证，而不是"另写一个循环"：

| 隔离维度 | 靠什么 |
|---|---|
| 只拿到 `profile.system_prompt`（AGENTS.md §3） | `ProfilePrompt` |
| 上下文一次性、不污染主会话 | `EphemeralSessionStore` |
| 只读档位挡写工具 | `ReadonlyEnforcingGate`（按策略**直接拒绝**，子智能体没有用户可问） |
| 只装裁切后的工具 | `filter_subagent_tools` + 描述符驱动 |
| 父会话取消真的传进来 | `WatchedCancel` → 引擎 `CancelToken` |

子智能体**不递归**（`invoke_subagent` 永远不在子智能体的工具表里），
且**没有"续跑"**：上下文刻意是临时的，所以协议里也没有 `subagent.resume`。

---

## 检查点与回滚

改动前会把涉及的文件纳入检查点（`ToolDescriptor.rollback` 声明）。
`change.revertCard` / `revertCheckpoint` / `revertFile` / `revertAll` 回滚后，
回执里带**恢复 / 删除 / 跳过 / 失效**四份清单——界面可以列出"恢复了哪些文件"。
没有对应检查点时返回 `ok: false` + 原因，**不会谎报"已回滚"**。

---

## 运行

```bash
# 桌面客户端（前端热重载 + 宿主）
bun run tauri:dev

# 构建
cargo build --workspace

# 打包独立二进制
cargo xtask ship --product ada-coding
```

**单 exe 多角色**：

```bash
a-da.exe                                  # 默认：Tauri GUI 桌面端
a-da.exe --workspace "E:/codes/proj"      # 指定初始工作区启动 GUI
a-da.exe --headless                       # 纯后台守护进程（自动分配空闲端口）
a-da.exe daemon --port 52353 --token "…"  # 固定端口与令牌

# 单次任务：装配引擎 → 跑一轮 → 会话落盘
a-da.exe run --workspace "." "审查所有 Rust 文件并指出潜在错误"
a-da.exe run --workspace "." --dry-run "冒烟"   # 不联网，只验证装配与落盘
# 退出码：0 = 完成；1 = 执行失败（含未配置模型供应商）；2 = 用法错误
```

**数据目录**：配置、会话流水、插件、调试日志都在 `~/.a-da` 下；
`A_DA_HOME` 可以换掉这个目录。`A_DA_CONFIG` 可以换掉配置文件路径。

**模型配置**：界面设置里的「供应商」一栏（预设：OpenAI / DeepSeek / 阿里云百炼 /
Moonshot / Ollama / 自定义）。环境变量（`A_DA_API_KEY` / `A_DA_MODEL` / `A_DA_BASE_URL`，
以及 `OPENAI_*`）优先于配置文件。网络走代理就设 `HTTPS_PROXY` / `HTTP_PROXY`。

### 构建注意

本机 `cargo` 默认 target 目录编译 `ring` 会报 MSVC `D8050`；
加 `CARGO_TARGET_DIR=../cargo_target_ada` 复用已有缓存即可（与代码无关）。

---

## 代码结构

```
crates/
  agent-base/          领域类型 + 11 个端口 + 单一引擎（零 IO、零产品名词）
    domain/            消息、工具描述符与回执、事件、错误、失败方向
    ports/             AppHome ApprovalGate CancelToken Clock EventSink
                       ModelClient PromptSource Scope SessionStore Tool ToolCatalog
    engine/            run_turn（唯一多轮循环）
    testing/           测试替身（FixedClock / RecordingSink / ScriptedModelClient …）
  agent-proto/         线协议：帧、方法常量、DTO、错误码、client-ts
  agent-adapter/       模型 SSE（三家协议）、会话落盘、提示词、作用域、时钟
  agent-toolkit/       ToolDescriptor 注册表（24 个）+ 各工具包工厂
  agent-runtime/       ProductBuilder、AgentSpec、工具目录
  agent-host/          按产品声明装配引擎（组合根）
  agent-core/          JSON-RPC 分发、会话状态、插件/技能/子智能体
  agent-conformance/   端口契约 + 不变量 + golden 夹具
  ts-engine/           插件运行时（Boa + oxc）
products/
  ada-coding/          产品声明 + 极小 main
  ada-skeleton/        极简骨架（用于验证"产品只是一份声明"）
src-tauri/             Tauri 宿主 + CLI（`run` 在 cli_run.rs）
tauri-ui/              React 界面（client/ws-client.ts 是唯一协议客户端）
spec/proto/            协议登记表（唯一手写源）
tools/xtask/           门禁与打包
```

**唯一真源清单**（改一处要同步的地方都在这）：

| 事实 | 唯一真源 | 谁在守 |
|---|---|---|
| 工具元数据（读写性、审批、回滚、终止） | `agent-toolkit/src/registry.rs` | `verify-wiring` check B/C |
| 协议方法 | `spec/proto/*.json` | `verify-wiring` A、`compat`、`agent-proto` 测试 |
| 端口清单 | `agent-base/src/ports/` 的 `pub trait` | `verify-wiring` check D（派生） |
| 工具包名 | `agent-toolkit/src/toolkits.rs` | `verify-spec` |
| 产品能力声明 | `products/*/agent.spec.json` | `verify-spec`（逐字段找消费者） |

---

## 门禁与测试

**六条命令，全绿才算完**：

```bash
cargo test --workspace -- --test-threads=1   # 门二：全量测试（必须串行）
bun run typecheck                            # 门一：前端类型检查
cargo xtask verify                           # 主门禁：cargo test + 归档校验
cargo xtask verify-wiring                    # 接线结构审计（协议覆盖、注册表派生、端口实现、失败方向、前端重连）
cargo xtask verify-spec                      # 产品声明：工具包存在 + 每个声明字段有生产消费者
cargo xtask compat                           # 跨产品 base 协议一致
cargo xtask verify-archive                   # 归档门：主干不得引用归档、不得有第二份引擎
```

**为什么必须串行**：个别用例共享进程级状态（`AppHome` 单例、全局问题管理器）。
测试默认 home 已指向临时目录，但仍有个别用例共享状态。

**为什么归档门里有一条"不得有第二份引擎"**：仓库曾经有两套多轮循环
（主循环 + 子智能体循环），取消与工具判定各写一遍。现在只有
`agent-base` 一份，`verify-archive` 会盯着这一点。

**TS 测试已整体冻结**：`bunfig.toml` 已把旧实现排除在测试发现之外，
跑 TS 测试只会得到 "No tests found"。门就是上面这六条。

---

## 已知限制

完整的**功能缺口清单**在 [docs/unfinished-features.md](docs/unfinished-features.md)
（含证据与最小实现路径）。接线类问题的唯一口径在
[docs/agent-base-wiring-plan.md](docs/agent-base-wiring-plan.md)。

几个值得先知道的：

- **MCP client 未实现**（待拍板：是否接受外部进程注册工具）；
- **审批没有持久化 allowlist**：`ApprovalGuardConfig.auto_approve` 存在但**没有消费者**；
- **图片输入从未送进模型**：`TurnRequest::with_images` 没有调用点
  （当前产品都声明 `images=false`，所以门禁是通的，但链路没接）；
- **钩子点位 = 0**：插件契约承诺了机制与点位，实际一个都没接；
- **无 i18n**：界面文案硬编码中文（产品 `identity.locale` 只影响系统提示词）；
- **文件树 / 编辑器视图 / 会话全文搜索 / 成本计价**：均未排期。

---

## 相关文档

| 文档 | 用途 |
|---|---|
| [docs/agent-base-design.md](docs/agent-base-design.md) | 设计口径（分层、端口、协议单源、装配、交付） |
| [docs/agent-base-wiring-plan.md](docs/agent-base-wiring-plan.md) | **接线收口计划与执行记录**：缺口台账、任务表、断言变更 |
| [docs/unfinished-features.md](docs/unfinished-features.md) | 功能缺口清单（证据 + 最小路径 + 状态） |
| [docs/feature-catalog.md](docs/feature-catalog.md) | 已实现功能清单 |
| [docs/protocol/README.md](docs/protocol/README.md) | 线协议总览（握手、帧、错误码、审批通道） |
| [spec/proto/README.md](spec/proto/README.md) | 协议登记表真源与命名规则 |
| [AGENTS.md](AGENTS.md) | 协作者规范与 § 索引 |
