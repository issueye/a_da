# a_da 已实现功能清单

> **更新**：2026-10-25（按代码逐项核对；新增网关内置 SSR 网页界面）。
> **定位**：本文写"**现在真能做什么**"，每条都给出代码落点，便于复核。
> **配对阅读**：[unfinished-features.md](unfinished-features.md)（还缺什么）、
> [../plans/wiring.md](../plans/wiring.md)（"装上了没接线"的处置记录）、
> [../README.md](../README.md)（按功能域找文档）。

系统形态：**Electron 桌面壳 / 浏览器（网关内置 SSR）→ 网关（`ada-gateway`）→ 按工作区拉起产品实例（`ada-coding`）**；
界面与宿主之间是 **JSON-RPC 2.0 over WebSocket**，另有一层 HTTP unary API（`/api/*`）承载一问一答——
两者的端点、鉴权、帧与错误码统一在[接口文档](../protocol/README.md)；事件仍走 WS。

---

## 一、内核与引擎

| 能力 | 落点 |
|---|---|
| **单一多轮循环** `run_turn`（唯一的 agent 循环，INV-1） | `crates/agent-base/src/engine/runtime/mod.rs` |
| 零 IO / 零产品名词的领域层 | `crates/agent-base/src/domain/`（消息、工具描述符与回执、事件、错误与失败方向、诊断） |
| **13 个端口**（无默认实现，缺实现就编译不过） | `crates/agent-base/src/ports/`：`AppHome` `ApprovalGate` `CancelToken` `Clock` `EventSink` `HookRunner` `ModelClient` `PromptSource` `Scope` `ScriptHost` `SessionStore` `Tool` `ToolCatalog` |
| 一等事件流（13 个变体）+ 单调 `seq` | `crates/agent-base/src/domain/event.rs` |
| **失败方向**（`Closed` 默认拒绝 / `Open` 放行） | `ApprovalGate::direction()`（INV-4） |
| 取消贯穿到工具执行内部 | `CancelToken` → `ModelClient::stream` / `Tool::execute`（INV-8） |
| 轮次预算（步数 / 并发 / 工具超时） | `RunPolicy` ← 产品声明 `policies` |
| **诊断遥测**（17 个点位，只登记真有发射方的） | `crates/agent-base/src/domain/diag.rs` 的 `DIAG_POINTS` |

---

## 二、工具与工作区沙箱

**20 个内置工具描述符**，元数据（读写性、审批、回滚、终止）单一真源在
`crates/agent-toolkit/src/registry.rs`；工具包名单单一真源在 `toolkits.rs`
（`core` / `fs` / `command` / `git` / `project` / `forge`）。

| 工具包 | 工具 | 备注 |
|---|---|---|
| `core` | `ask_user`、`todo`、`finish` | 提问、多步规划、终止整轮 |
| `fs` | `read_file`、`write_file`、`edit_file`、`list_files`、`search_files`、`batch_write`、`batch_replace` | 编辑按文本匹配，**匹配按 LF 归一化、写回按文件原换行还原**（`agent-toolkit/src/newline.rs`） |
| `command` | `run_command`、`run_background`、`check_task`、`kill_task` | **超时/中止杀整棵进程树**；子进程 stdin 一律空设备（否则一条等输入的命令会卡满超时） |
| `git` | `git`（单工具，15 个 `action`） | 只读动作（status/diff/log/show/blame）不问、危险动作（reset/checkout/restore/merge/push、stash drop、force、hard）走 `DangerScan` 必审批；参数经 argv 直传，不经 shell |
| `project` | `project_inspect`、`code_outline`、`run_tests` | `run_tests` 会执行命令，按**写工具**处理 |
| `forge` | `create_plugin` | 生成可加载的插件包骨架；`scope` **必填**（workspace/global），缺了会让模型先 `ask_user` 问用户 |
| 能力位装配 | `invoke_subagent` | 由 `capabilities.subagents` 决定是否装配 |

- **路径沙箱**：所有路径解析回会话工作区根；越界拒绝（含 `run_command` 的 `cwd`）；
  字符串检查之后还 realpath 解析符号链接（Windows junction 也算），按**真实落点**拒绝。
- **档位与路径同源**：fs 工具走 `PathGate::Scoped`（引擎注入的 `Scope`），
  `Full`（完全访问）下工作区外的路径可读写；`~/.a-da` 里只有 `agent-coding/extensions/`
  （插件）与 `agent-coding/skills/`（技能）放行，配置与密钥仍拒；
  插件工具与内部调用走 `PathGate::Workspace`（只认工作区，行为不变）。
- **长时工具豁免**：等用户、等子智能体、自带超时的命令不套策略超时
  （`LONG_RUNNING_TOOLS` 单一真源 + 契约测试）。
- **失败安全**：认不出的工具名一律按**写操作**处理（该审批就审批）。

---

## 三、会话与状态帧

| 能力 | 落点 |
|---|---|
| 会话落盘（JSONL）与摘要列表 | `crates/agent-adapter/src/store/`、`crates/agent-node/src/session/` |
| 全量快照 `evt.state.snapshot`（首帧 / 自愈 / 重连） | `crates/agent-rpc/src/server/emitter.rs` |
| **增量帧** `evt.state.patch`（指纹比对，只推变过的 item） | 同上；契约见 [../protocol/README.md](../protocol/README.md) §4 |
| 流式增量 `evt.message.delta`、单卡更新 `evt.card.updated` | 同上 |
| **卡片顺序**由 `at` 表达且严格递增（内存侧与磁盘回填侧两处都守） | `agent_rpc::state::store::next_item_at` + `SessionManager::CardStamps` |
| item 分页 | `thread.items.page` |
| 客户端重连退避（500ms 起 / 30s 封顶 / 抖动 + 代次保护） | `desktop-ui/src/client/reconnect-policy.ts`（`verify-wiring` 有结构断言） |

---

## 四、审批与提问

- 引擎判定"要问"→ 发 `ApprovalRequested`（**带工具名与参数原文**）→ 卡片进入
  `waiting_approval`；界面在**输入框上方浮窗**批准 / 拒绝（`ApprovalFloatingPanel.tsx`，
  不随对话流滚动），走 `approval.decide`。
- **拒绝不抛错**：拒绝理由作为**工具结果**回模型（模型知道自己被拒了）。
- 三档审批档位 + `Full`（完全访问，放开工作区边界与插件网络能力）。
- 超时默认 300s，按失败方向处理并记 `by: Timeout`。
- 模型主动提问走同一形态（`ask_user` / `question.answer`，`AskUserFloatingPanel.tsx`）。

---

## 五、子智能体与委派

| 能力 | 落点 |
|---|---|
| 一等工具 `invoke_subagent`（描述符在注册表里） | `crates/agent-toolkit/src/registry.rs` + `crates/agent-node/src/subagents/` |
| 只拿 `profile.system_prompt`；上下文一次性（不污染主会话） | `ProfilePrompt` + `EphemeralSessionStore` |
| 只读档位挡写工具（子智能体没有用户可问 → 直接拒） | `ReadonlyEnforcingGate` |
| 工具表按描述符裁切 | `filter_subagent_tools` |
| 父会话取消穿透 | `WatchedCancel` → 引擎 `CancelToken` |
| 跨进程委派与**多轮续跑** | `AgentBus` 端口（本地实现如实拒绝续跑；网关实现 `GatewayAgentBus` 支持） |

子智能体**不递归**，且没有 `subagent.resume`（上下文刻意是临时的）。

---

## 六、检查点与回滚

- 改动前把涉及的文件纳入检查点：**由描述符的 `RollbackPolicy` 声明**（不再有手写名单）。
- `change.list` / `change.count` / `change.revertCard` / `revertCheckpoint` / `revertFile` / `revertAll`。
- 回滚回执带**恢复 / 删除 / 跳过 / 失效**四份清单；没有检查点时 `ok: false` + 原因，
  **不谎报"已回滚"**。
- 界面侧：改动面板 `ChangesPanel.tsx`、`utils/derive-changes.ts`。

---

## 七、插件与钩子（扩展点）

| 能力 | 落点 |
|---|---|
| 包发现（工作区 / 全局，包根 `index.ts|js`，标识 `scope:<id>`） | `crates/agent-node/src/plugins/manager.rs` |
| **四类贡献**：工具 / 系统提示词 / 技能 / 钩子（装配期结算） | `plugins/sandbox.rs`（脚本侧）+ `plugins/view.rs`（活体视图） |
| **钩子 33 个点位**，成对硬约束，缺席报 `no_subscriber` | `crates/agent-base/src/ports/hooks.rs` |
| 动作分级（数据面 / 控制面 / 只读面；`abort` 只给内置；`replace` 仅两处） | 同上 `HookLevel` / `allows` / `REPLACE_SUPPORTED` |
| 用户能力开关（6 位 + 超时，每位都有生产消费者） | `plugins/types.rs` 的 `PluginCapabilities`，门禁 `verify-wiring` check L2 |
| 沙箱两档（进程内 Boa / 子进程） | `crates/ts-engine` + `--script-sandbox subprocess` |
| 能力声明 `// @ada-needs`（`fs` / `fs=read` / `fs=write` / `net` / `none`） | `crates/agent-node/src/scripts/needs.rs` |
| 管理面：`plugin.list/load/validate/setEnabled/reload/delete/builtinCatalog/createTemplate/capabilities.set/config.set/secret.set` | `spec/proto/ada-coding.ext.json` |
| 内置插件 **10 个**：`git-tools`、`code-outline`、`project-inspector`、`test-runner`、`batch-ops`、`plugin-forge`、`todo-guard`、`approval-guard`、`ask-user`、`ponytail` | `crates/agent-node/src/plugins/builtins.rs` |

细节与排错见 [../plugins/README.md](../plugins/README.md)。

## 八、技能与提示词

- **技能三来源**：用户目录 / 插件声明 / 内置；插件技能带 `plugin:<id>` 来源，界面分开显示。
- frontmatter 有**认不出的键**会记 `skill.invalid` 诊断（不再静默忽略）。
- 提示词组装：人格（`identity`）+ 工作区 + 编码指引 + 插件片段（带 `plugin:<id>` 归因）+
  技能正文；`stats.promptChars` 返回**实测**字符数。
- 诊断点位：`prompt.composed`、`skill.loaded` / `skill.skipped` / `skill.invalid`。

## 九、模型接入

| 能力 | 落点 |
|---|---|
| 三家协议 SSE（OpenAI Chat / Anthropic / OpenAI Responses） | `crates/agent-adapter/src/model/` |
| **流式只设空闲超时**（`A_DA_STREAM_IDLE_SECS`，默认 90s）+ 建连超时 10s | 同上（曾有"总时限 120s 截断长回答"的事故） |
| **EOF 不等于完成**：必须见过终止信号（`[DONE]` / `finish_reason` / `message_stop` / `response.completed`） | 同上 |
| 请求重试（设置项 `retryCount`，默认 5，范围 0..=20；429 认 `Retry-After`，上限 30s） | `config.setRetryCount` → `ModelChatOptions.max_retries` |
| 重试耗尽 → 落 `Notice` 卡片（**只给人看，永不进模型上下文**，可重试） | `persist_failure_notice` + `NoticeRow`（`Transcript.tsx`） |
| 图片输入（data URI → 三家协议的内容块） | `dispatch::normalize_images`（**全仓唯一图片 IO 边界**）→ `TurnRequest::with_images` |
| 思考内容与正文分离 | 适配器流式解析 |

## 十、网关

- **三合一**：AGENT 管理平台（注册表 / 监督 / 实例回收）+ 交互平台（委派 / 续跑 / 取消）+ 桥接平台（路由透传）。
- **一工作区一进程**：身份键 `workspace_key()`（统一分隔符、折叠 `.`/`..`、Windows 折叠大小写）；
  空闲 15 分钟回收、同时在跑上限 6（LRU）、每分钟扫一次；**有客户端连着不回收**；
  工作区目录不存在则拒绝拉起。
- **WEB 接入面**：token / 作用域 / Origin / 配对码；非回环 `--host` 必须配 token 且显式允许明文，
  否则**拒绝启动**。
- 工作区门户：`gateway.fs.*`、`gateway.workspaces.*`（[设计意图](../gateway/README.md)）。
- **网关 ↔ agent 的链路**：回环 TCP（默认，`ws://127.0.0.1:<port>/rpc`）或本地管道
  （`A_DA_GATEWAY_AGENT_LINK=pipe`：Windows 命名管道 / Unix socket，**端口 0、不监听 TCP**）；
  两种形态同一套帧与 token 握手，端点解析单点在 `agent-gateway/src/link.rs`。
- **内置 SSR 网页界面**（2026-10-25，`crates/agent-web`）：网关默认 `--web-ui ssr`，一个进程一个端口
  托管页面 / SSE / `/static/*` / `/ext/*` / `/api/*` / `/rpc`；页面通过 `GatewayBackend` 进程内调用
  agent（不再 HTTP 反代），事件订阅持有 `ConnectionGuard` 防回收；`/__auth` 接受网关 token / 配对码并
  换发 HttpOnly 会话 cookie。`--web-ui desktop` 回退到 `desktop-ui/dist` 静态托管。

## 十一、桌面端界面（默认 SSR；`desktop-ui/` 回退）

- **Electron 默认加载 SSR**：等网关就绪后 `loadURL(http://127.0.0.1:<port>/)`，带 token 时用
  `?token=` 自动登录；窗口保持无边框，preload 在 SSR 顶栏注入最小化/最大化/关闭。`--ui=desktop`
  （`bun run electron:dev:desktop`）回退到下面的 React 界面。
- **React 回退布局**：标题栏（连接态 / 模式徽标 / 窗口控制，`z-[100]`）+ 侧栏（工作区与会话）+ 主区
  （`TabStrip` + `Transcript` + `Composer`）+ 右侧面板（`AgentContextPanel`）。
- **浮层**：审批（`ApprovalFloatingPanel`）、提问（`AskUserFloatingPanel`）、待办（`TodoFloatingPanel`）、
  排队消息（`QueuedMessagesFloatingPanel`）、命令面板、斜杠命令、`@` 引用菜单、
  流程查看（`ProcessModal`）、设置（`SettingsModal`，供应商 / 模型 / 审批档位 / 重试次数）、
  插件管理（`PluginsModal` + `PluginDiagnostics`）、调试面板。
- **工作区与文件**：Web 原生目录选择（`FilePicker.tsx`）走 `fs.roots` / `fs.list` / `fs.mkdir`，
  **不使用操作系统原生弹窗**；截断会如实提示（"已省略 N 项"）。
- **渲染**：Markdown（`MarkdownRenderer.tsx`）、改动 diff、工具卡片（状态 / 耗时徽章）、
  上下文用量（`ContextUsagePopover` + `utils/context-breakdown.ts`）。
- **模式**：coding / pm 由 `mode` 表达（`agent_id` 恒为 `ada-coding`），PM 是**模式预设**不是第二个产品。

## 十二、协议面（77 个方法）

| 域 | 代表方法 |
|---|---|
| 会话与轮次 | `session.initialize` / `session.snapshot`、`thread.create` / `send` / `retry` / `abort` / `compact` / `editAndResend` / `items.page` |
| 审批与提问 | `approval.decide`、`question.answer` |
| 队列 | `queue.promote` / `remove` / `clear` |
| 界面态 | `ui.activeThread` / `activeProject` / `openTab` / `closeTab` / `setShell` |
| 配置与供应商 | `config.get` / `setProvider` / `checkProvider` / `setApproval` / `setEffort` / `setRetryCount`、`provider.*` |
| 插件 / 技能 / 提示词 / 子智能体画像 | `plugin.*`、`skill.*`、`prompt.*`、`subagentProfile.*` |
| 改动与回滚 | `change.*` |
| 文件与工作区 | `fs.roots` / `list` / `mkdir` / `read_base64`、`workspace.add` / `remove` / `entries` / `openPublic` |
| 调试 | `debug.hostInfo` / `trace` / `log.clear`、`stats.promptChars` |

登记表真源：[`spec/proto/`](../../spec/proto/README.md)（base 23 + 产品扩展 54）。

**HTTP unary API（Phase 0/1/2）**：`crates/agent-gateway/src/http_api.rs` 把 60 个
unary 方法映射为 `/api/*`（18 条查询 + 42 条写），另有 11 条网关管理面；Bearer/Origin/
作用域鉴权、`/api/snapshot` 的 ETag、JSON-RPC 错误码到 HTTP 状态的映射均已接线。
事件与流式输出仍由 `/rpc` 的 WS 推送，不被 HTTP 改动。端点表见
[protocol/README.md](../protocol/README.md) §3.3。

## 十三、门禁与实机验收

静态门：`cargo xtask verify-spec` / `verify-wiring` / `compat` / `verify-docs`（`verify --fast` 一次跑完）；
测试门：`cargo test --workspace -- --test-threads=1`（**必须串行**）与 `bun run typecheck`。
实机脚本：Electron 默认 SSR（`verify-electron-ssr.mjs`）、React 桌面壳（`--ui=desktop`；锁屏时自动走 CDP 无头版）、重试与断线、卡片顺序、插件平台、网关进程模型、网关内置 SSR
（`tools/verify-gateway-web.mjs`）、SSR 流式 / 审批中止 / 工作区（`tools/verify-ssr-*.mjs`，均为单进程），
见 [README.md](../../README.md#门禁与测试)。

---

## 能力位（产品声明的现状）

`products/ada-coding/agent.spec.json`：`images` / `streaming` / `rollback` / `subagents` /
`plugins` / `hooks` **全部为 true**；`policies`：`parallelTools: 4`、`toolTimeoutSec: 120`。
> ⚠️ `parallelTools` 目前只是声明：`RunPolicy.max_parallel_tools` 尚未被引擎读取，工具实际始终串行。
`capabilities.images` 是**产品级**开关，单个模型认不认图另由 `ModelEntry.supportsImages` 表达
（**两者不是一回事**）。
