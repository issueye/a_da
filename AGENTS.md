# AGENTS.md

给在本仓库工作的 AI 智能体与协作者的注意事项。**本文件会被自动注入系统提示词**，因此只留
「接手就必须知道的事」；深度约定全文已迁至 **[docs/agent-conventions.md](docs/agent-conventions.md)**。

项目速览：**纯 Rust 微内核**（`crates/agent-base/`：零 IO 的领域层 + 11 个端口 + **唯一**多轮引擎 `run_turn`）
+ **两层拆包**（S4）：`crates/agent-node/`（节点：会话 / 审批 / 检查点 / 委派 / 插件 / 技能）
与 `crates/agent-rpc/`（桥接面：JSON-RPC 分发 + WS 宿主 + UI 投影）
+ **网关**（S5/S6）：`crates/agent-gateway/`（`ada-gateway`：AGENT 管理平台 + 交互平台 + 桥接平台）
+ **桌面客户端宿主**：**Electron 桌面壳**（`electron/`：彻底根除 Windows WebView2 权限污染与目录锁死问题，支持 `bun run electron:dev` 与 `bun run electron:pack`）+ **React 前端**（`desktop-ui/`：通过 `src/client/desktop-bridge.ts` 与 Electron 宿主通信）；
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
cargo build --workspace                       # 基座四 crate + agent-node + agent-rpc + 产品
cargo test --workspace -- --test-threads=1     # 门二（必须串行：有共享全局态的用例）
bun run typecheck                             # 门一：desktop-ui 的 tsc --noEmit
bun run verify:archive                        # 归档门：主干不得引用 archive/、不得有第二份引擎
cargo xtask verify-wiring                     # 接线门：方法覆盖 / 描述符单源 / 端口实现 / 依赖方向
cargo xtask verify-spec                       # 声明门：产品/预设 spec 校验
bun run electron:dev                          # Electron 桌面端开发（前端热重载 + 网关监督）
bun run electron:pack                         # 打包 Electron 免安装版 (dist/electron/win-unpacked)
pwsh -NoProfile -File tools/verify-desktop-shell.ps1                    # 桌面壳验收（Electron 交付形态）
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
- **工具链环境**：现行版本为 **Rust 1.99.0 / Cargo 1.99.0 / Rustup 1.29.1**（MSVC）。
- **应用目录端口在 `cfg(test)` 下指向临时目录**（`agent_node::session::app_home()` →
  `temp/a_da_agent_core_test_home_<pid>`），**但那只覆盖 crate 自身的单测**：
  集成测试（`crates/*/tests/`）里依赖是按**非 test** 编译的，端口仍指真实 `~/.a-da`。
  所以集成测试必须显式注入（见下一条三条铁律），**不要用 `std::env::set_var`**。
- **别跑 `bun test`**：TS 测试已随归档冻结，`bunfig.toml` 已把 `archive/**` 排除在 test 发现之外（跑只会得到 "No tests found"）；门是 `cargo test`。
- 🔴 **测试绝不能写用户真实的 `~/.a-da`**。2026-10-12 真事故：跑一次 `cargo test -p agent-rpc`，
  用户配置里的激活供应商被改成了测试数据（`anthropic-claude`）、顶层 baseUrl/apiKey/model 一起被改。
  铁律三条：
  ① 应用目录端口是 `OnceLock`——**只要有一个用例先构造过 `AgentStore::new`，同进程后续的
  `std::env::set_var("A_DA_HOME")` 就完全无效**。集成测试一律用
  `AgentStore::with_home(ws, &TempAppHome::at(dir))`（见 `tests/dispatch_integration.rs::store_for_test`）。
  ② `PluginManager` / `SkillManager` 会写 `config.json` / `skills_state.json`，测试里必须
  `PluginManager::with_home(dir)` / `SkillManager::with_home(dir)`（生产路径 `new()` 不变）。
  ③ `save_app_config` 取 **store 自己的 `config_path`**，不再问全局 `get_config_path()`。
  跑完门二顺手比对 `~/.a-da/config.json` 的哈希没变。
- **网关的实例模型是"一工作区一进程"**（`AgentRegistry::id_for_workspace`）：同一工作区的
  **多个会话/多个客户端共用同一个 `ada-coding`**，这是"进程数可控"的前提。
  三条硬约束：
  ① 身份键走 `workspace_key()`：统一分隔符、折叠 `.`/`..`、Windows 下折叠大小写——
     `E:\x` / `E:/x/` / `e:/X` 是同一个目录，**写法差异不许起出第二个进程**；
  ② **实例回收**（`supervisor::ReapPolicy` + `Gateway::spawn_reaper`）：空闲 15 分钟回收、
     同时在跑上限 6（LRU 驱逐）、每分钟扫一次，环境变量
     `A_DA_GATEWAY_IDLE_REAP_SECS` / `A_DA_GATEWAY_MAX_LIVE_AGENTS` /
     `A_DA_GATEWAY_REAP_INTERVAL_SECS` 可调；没有它，访问过 N 个工作区就常驻 N 个进程；
  ③ **有客户端连着就不回收**（`AgentRegistry::connections` + `relay` 的 ConnGuard）：
     用户可能正对着一条长回答发呆十几分钟，掐掉就是"莫名其妙掉线"。
  另外：工作区目录不存在时 `ensure_agent` **拒绝拉起**（为一个空气目录起进程只会白占一个）。
- **工具超时是真的会生效的**（`RunPolicy::tool_timeout` ← 产品声明 `policies.toolTimeoutSec`）：
  引擎对**非** `Tool::long_running()` 的工具加超时，超时给一条 `ToolStatus::Timeout` 回执
  （回执里说清是哪个工具、让模型换路子），整轮**继续**而不是永远挂住。
  哪些工具豁免由 `agent_toolkit::registry::LONG_RUNNING_TOOLS` 单一真源声明
  （等审批 / 等提问 / 派活 / 命令执行 / 决策插件），契约测试
  `test_long_running_set_matches_tool_declarations` 守着名单与实现一致。
  🔴 **子进程的 stdin 一律 `Stdio::null()`**（`cmd_tools::run_command` 与 `background`）：
  继承 stdin 时，一条等输入的命令（npm 确认、git 凭据、忘了 `--yes` 的安装器）会**卡满整个超时**，
  界面上的表现正是"工具卡片卡住 → 智能体停止 → 过一会儿才继续"。给空设备它会立刻 EOF 退出。
- **别让 `Gateway::new` 有落盘副作用**：它以前在构造时就往工作区存储里记默认工作区，
  而 `WorkspacesStore::new(None)` 指用户真实的 `~/.a-da/gateway_workspaces.json`——
  于是**任何构造 Gateway 的测试都会写用户的真实工作区列表**（历史事故：用户列表里出现
  一堆 `a_da_gw_e2e_*`）。现在构造期不落盘，生产在组合根显式记一次，测试注入
  `Gateway::with_workspaces(临时存储)`。
- **审批事件必须带 `args`**（`AgentEventBody::ApprovalRequested { call_id, tool, args }`）：
  审批卡要回答的是"我到底在批准什么"——危险命令原文、要覆写的文件路径。
  引擎在建卡之前就发这个事件（顺序是 `ApprovalRequested` → 等用户 → `ToolCallStarted`），
  所以审批卡**拿不到** `ToolCallStarted` 的 args，只能自己带；`store.set_tool_waiting_approval`
  会把参数落进卡片（`args`/`raw_args`/`details.approval.args`），界面据此渲染待批准内容。
  只给工具名 = 让用户闭眼点"批准"，那道二次确认闸门形同虚设。
- **审批的操作面只有一处：输入框上方的浮动审批窗口**
  （`desktop-ui/src/components/ApprovalFloatingPanel.tsx`，挂在 `Composer` 的列里，因此**不随对话流滚动**；
  数据源是会话 items 里 `status == "waiting_approval"` 的卡片，回执走 `approval.decide`）。
  对话流里那张卡只留**记录**：待批准内容的预览 + "去浮窗处理"。
  同一件事摆两套按钮，用户会怀疑"点哪个才算数"——按钮只在浮窗一处；
  预览文案两处共用 `desktop-ui/src/utils/pending-approvals.ts` 的 `approvalPreviewOf`，别各写一份。
- **模型请求重试次数**（设置项，默认 5，范围 `0..=20`）这条链要一起改才生效：
  `ConfigSnapshot.retryCount`（wire/`client-ts`）↔ `~/.a-da/config.json` 的 `retryCount` ↔
  `config.setRetryCount`（`spec/proto/ada-coding.ext.json` 也要登记）↔ 每轮
  `TurnRequest::with_max_retries` → `ModelChatOptions.max_retries` → 适配器指数退避
  （429 认 `Retry-After`，上限 30s）。**重试耗尽即断开本轮对话**：不再消费排队指令，
  失败文本由 `persist_failure_message` **落盘**——只放内存里会被 1.5s 后的
  `sync_threads_from_disk` 整份覆盖，界面上的错误卡片会"闪一下就没"。
  实机回归：`pwsh -NoProfile -File tools/verify-retry-and-disconnect.ps1`
  （起真产品、走真 JSON-RPC、改用户配置前先备份后还原）。
- 🔴 **Electron 宿主里 `__dirname` 不可信**：`bun run electron:build:main`（`bun build --target node
  --format cjs`）会把 `__dirname` **硬编码成构建机的源码目录**。用
  `path.join(__dirname, 'preload.cjs')` 当 preload 路径 → preload 静默加载失败 →
  `window.electronAPI` undefined → **界面拿不到网关端口（显示"未连接"）、窗口按钮全部空操作**，
  这就是"Electron 版功能缺失"的根因形态。资源路径一律走 `resolveAppPath()`
  （`app.getAppPath()` 优先 + 候选清单 + 真实存在性检查），并且失败要**大声报**
  （`dialog.showErrorBox` + launcher.log），不要静默降级。
- 🔴 **无边框窗口的标题栏必须压在所有应用模态层之上**：应用内弹窗统一是 `fixed inset-0 z-50`，
  标题栏若没有更高层级（现为 `relative z-[100]`），任何弹窗打开时最小化/最大化/关闭**点不动**、
  窗口也拖不动——第二个"无法最小化/关闭窗口"的形态。`tools/verify-desktop-shell.ps1`
  专门守着这条（弹窗开着时点最大化仍须生效）。
- **渲染进程日志要能进 launcher.log**：Electron ≥32 的 `console-message` 签名是
  `(event, details)`（旧的是 `(event, level, message, line, sourceId)`）。按旧签名写就只剩
  undefined，界面里的错再也查不到；`preload-error` / `render-process-gone` / `did-fail-load`
  一并转发。打包后的 exe 没有控制台，**应用日志是唯一证据**（§17）。
- **桌面壳的行为要按"点一下"验收**：`pwsh -NoProfile -File tools/verify-desktop-shell.ps1`
  （Electron 交付形态）。真鼠标点窗口按钮 + 读 `launcher.log`
  断言"拉起网关 → 界面接入 → 窗口控制 → 状态同步 → 退出清理"。脚本注释里记了必踩的坑：
  窗口句柄要按标题找（`MainWindowHandle` 会挑到隐藏/辅助窗口）、按钮定位要按「与背景色的差异」
  而不是亮度（暗色主题下整条标题栏都算"暗"）、每次点击前重新量窗口矩形（最大化会改坐标）、
  截图前必须把窗口顶到前台（否则截到/点到的是盖住它的窗口）。
  另：脚本会清除 `ELECTRON_RUN_AS_NODE`——被污染时 Electron 会以"纯 Node"跑 main.cjs 当场崩掉。

## 代码结构

**目录结构按设计落地中**（`docs/agent-base-design.md` v0.2 §3；已搬批次见 `docs/agent-base-plan.md` §8）：

- `crates/agent-base` 基座内核（零 IO、零产品名词）：`model/`（模型面类型）、`domain/`（消息·工具描述符与回执·事件·错误与失败方向）、`ports/`（11 个端口，**无默认实现**）、`engine/`（**唯一**多轮循环 `run_turn`）、`testing/`（FixedClock/RecordingSink/TempAppHome）
- `crates/agent-proto` 线协议：JSON-RPC 帧、方法常量、线上 DTO、错误码
- `crates/agent-adapter` 适配器：`model/`（三家协议 SSE + 中止）、`app_home.rs`（**全仓唯一**读 `A_DA_HOME`/`USERPROFILE` 的地方）、`clock.rs`（**全仓唯一**读系统时间的地方）
- `crates/agent-toolkit` 工具包：文件读写、路径沙箱、命令执行、文本 diff、决策与门禁判定；`registry.rs` 是**工具元数据与描述符单一真源**
- **`crates/agent-node`（S4 拆出，5.9k 行 / 34 文件）** —— 节点：`session` / `approval` / `checkpoint` / `subagents`（含 `agent_bus.rs` 委派总线端口与 `local_bus.rs` 本地实现；端口支持**多轮续跑** `thread_id`，本地总线对它是**如实拒绝**——一次性子智能体没有可续线程）/ `plugins` / `skills` / `node_config.rs`（节点配置端口）/ `delegation_depth.rs`（委派深度端口：被派活的节点靠它知道自己「在第几层」）。**不含协议管道**（不依赖 `tokio-tungstenite`）、**不含 UI 投影**（不依赖 `AgentStore`）。`AgentBus` 的网关实现（`GatewayAgentBus`）在 **`agent-rpc`**——跨进程委派需要 WS 客户端，那属于桥接面，节点只保留端口
- **`crates/agent-rpc`（S4 拆出，5.3k 行 / 18 文件）** —— 桥接面：`server/`（`dispatch.rs` 72 个方法的 JSON-RPC 分发、`ws.rs` 宿主、`emitter.rs` 快照合帧、`fs_service.rs`）、`state/`（`AgentStore`，**给界面看的投影**）、`runner/`（引擎调用 + 基座一等事件 `AgentEventBody` 直连投影至 UI store，废除中间桥接 `AgentLoopEvent`）。**依赖方向单向：`agent-rpc → agent-node`**
- **`crates/agent-gateway`（S5 新建）** —— a-da 网关，三合一：**AGENT 管理平台**（`registry.rs` 实例注册表 / `supervisor.rs` 生命周期）+ **交互平台**（S6：`delegate.rs` 派活/**多轮续跑**/取消跨网关）+ **桥接平台**（`relay.rs` 路由与透传）+ **WEB 接入面**（S7：`auth.rs` token/作用域/Origin/**配对码**）。二进制 `ada-gateway`（打印 `A_DA_GATEWAY_READY {port}`）。**只依赖线协议 `agent-proto`**——不含引擎（INV-1）、不缓存会话状态（INV-8），由 `verify-wiring` check J 守着。🔴 **非回环 `--host` 必须配 `--token`，且必须显式 `--allow-plaintext`**（网关只提供明文 `ws://`，明文上的 token 可嗅探；反代终止 TLS 时由人确认）。两条都**拒绝启动**而不是警告——失败安全
- `crates/ts-engine` 插件运行时（Boa + oxc）；插件契约见 [docs/plugin-sdk/v1.md](docs/plugin-sdk/v1.md)
- `products/ada-coding` 产品二进制（`--host/--port/--token/--parent-pid/--workspace`）
- **PM 不是第二个产品**（2026-10-11 降级）：它是 `products/ada-coding` 的**模式预设**
  （`products/ada-coding/modes/pm.agent.spec.json`）——同一个二进制、同一处装配函数
  （`build_injection`），只是喂了另一份声明：persona（拆解/分派/汇总）+ 不含 `fs`/`command`
  的工具包（角色边界：PM 不亲自改代码）+ 委派配置（有网关走网关，否则进程内本地委派）。
  会话是 coding 还是 pm 由 **`mode`** 表达，`agent_id` 恒为 `ada-coding`；老盘上
  `agentId=ada-pm`/`pm-assistant` 的会话在**读盘时自动归一并写回**（幂等）。
  模式预设同样受 `verify-spec` 门禁约束（`products/<产品>/modes/*.agent.spec.json` 也在扫描范围内）。
- 所有内部调用点直连基座（`agent-proto` / `agent-toolkit` / `agent-base` / `agent-adapter`），历史 shim 与 facade 已全量下线
- `electron/` **Electron 桌面宿主**：主进程监督 `ada-gateway` 生命周期、单实例锁、无边框原生控制与 IPC 桥接；`dist-electron/` 为构建中间产物
- `desktop-ui/` **React 桌面前端**：通过 `src/client/desktop-bridge.ts` 与 Electron 宿主通信（`ws-client.ts` 是唯一协议客户端）
- `dist/` **发布交付套件**：包含 `ada-gateway.exe`、`ada-coding.exe` 以及 `dist/electron/win-unpacked/`（Electron 完整绿色便携包，内置 Chromium 运行时与 `resources/bin/` 关联二进制，彻底规避 WebView2 权限污染）
- `archive/ts-legacy` **只读归档**（TS 时代的 src + scripts + app.tsx）

## 核心红线与防踩坑规矩

- **不许引用归档**：主干任何代码/配置都不得 import 或指向 `archive/ts-legacy/**`——那会把"两份实现"的漂移重新引进来；`bun run verify:archive` 会红。
- **改 Rust 核心前先看 [docs/agent-base-plan.md](docs/agent-base-plan.md) §1.3**：审批闸门、取消贯穿、`terminate`、插件 `enabled`、空壳工具等**目前多数是"看起来装上了、其实没接线"**，那里逐条写明了处置口径（补实现 / 删声明）。
- **工具描述符与元数据单一真源**（W2-T2 + 单源化收敛）：读写性（`is_write`）、审批要求、
  检查点回滚策略（`rollback`）、终止语义，以及全部 24 个内置工具的标准描述符与参数 Schema，
  均收敛于 `crates/agent-toolkit/src/registry.rs`；具体实现统一调用 `descriptor()` 构造，严禁内联手写；
  `test_builtins_descriptor_equality` 契约测试（INV-3）机械守门。
  历史上那五处手写名单（`lib.rs` 读写判定、`approval/types.rs` 命令工具、`executor.rs` 检查点、
  子智能体白名单、插件自述 `is_write`）**只剩插件自述一处**，且已改为按描述符校验。
- **事件流单一真源**：引擎多轮循环向外发射基座一等事件 `AgentEventBody`（12 个变体），
  `agent-rpc` 桥接层直接订阅并投影到 UI，严禁在桥接层私造二等中间事件流（历史 `AgentLoopEvent` 已废除）。

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
