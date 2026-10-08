# AGENT BASE：Rust-only Agent 基座与产品线规范（设计 v0.2）

> 状态：**设计草案 v0.2，只定设计与规范，不动代码**。
> **v0.2 的定位变更（相对 v0.1）**：① **只考虑 Rust**——TS 那套 agent 实现（`src/agent/**` 的循环/宿主/工具）
> 作为**归档版本**，不再是参考设计、不再是等价性基准；② 目标从"把 a_da 抽象出来"改成
> **"用基座快速产出一个协议与规范都一致的 AGENT CORE"**，且要能覆盖多种产品：
> **通用 AI 助手 / 生活类助手 / CODING 助手**（以及后续任何同类）。
> 依据：本仓当前实现的实测（证据见附录 A）。**没有实测依据的结论不写。**
>
> 归档的含义要说清楚：TS 的**客户端类型**（`tauri-ui` 需要的 `ClientSnapshot`/`Item` 类型）仍由生成器产出——
> 归档的是 **agent 实现**，不是"UI 不写 TS"。TS 也仍可作为**插件代码的运行时**（`ts_engine` 沙箱），
> 那是一个可选适配器，不是设计基准。

---

## 0. 一页结论

**基座 = 一个 Rust 内核（引擎 + 端口 + 协议）+ 一套产品声明 + 一道生成/校验闸。**
产品之间只允许在**声明**里不同；一旦某个差异需要改内核，那就是内核缺能力位，而不是"开个分支"。

```
                     ┌──────────────────────────────────────────────┐
  agent-base  ──────►│ 领域类型 · 端口 trait · 引擎 · 策略 · 事件流   │  零 IO / 零产品名词 / 零传输
                     └──────────────────────────────────────────────┘
                                     ▲ 被所有产品依赖（版本化、只由基座维护）
  agent-proto ──────►  线协议规范（base 协议 + 能力位 + 产品扩展命名空间）
                                     │
  agent-runtime ────►  组合根：按「产品声明」装配端口、校验、启动
                                     │
  products/<id> ────►  agent.spec.json（人格/工具包/能力/策略/传输）+ 约 30 行 main.rs
                                     ▼
             gen（生成协议目录·客户端类型·文档） → verify（一致性闸） → test（合规套件） → ship（单 exe）
```

**"协议和规范都一致"的机械化定义**（CI 对每个产品都跑，见 §8.3）：

| 门 | 断言 |
|---|---|
| `verify-proto` | base 协议在所有产品间**逐字节一致**；每个产品的目录 = base ∪ 自己的扩展；生成物（Rust 常量 / dispatch 臂 / 文档表格 / 客户端类型）**集合相等** |
| `verify-spec` | 声明里出现的工具/能力位/方法**都能被装配**（没有"声明了没有实现"），且没有"实现了却没声明" |
| `conformance` | 端口契约 + 跨端口不变量全绿（审批必被调用、取消贯穿、回执完整、失败方向一致…） |
| `verify-archive` | 仓库里**不再存在**第二份引擎实现（防止 TS 那套以任何形式复活） |

新增一个 agent core 的路径（**7 步，其中 5 步是生成/校验**）：

```
cargo xtask new-agent --id ada-life --archetype assistant   # 1 脚手架
$EDITOR products/ada-life/agent.spec.json                   # 2 只改声明
cargo xtask gen                                             # 3 生成协议目录/客户端类型/文档
cargo xtask verify                                          # 4 一致性闸（红则不许合并）
cargo test -p ada-life                                      # 5 合规套件
cargo xtask ship --product ada-life                         # 6 单 exe
cargo xtask verify-archive                                  # 7 没有第二份引擎
```

---

## 1. 为什么是 Rust-only，以及要付的代价

### 1.1 事实：多实现已经在漂移（TS 归档的动机）

| 契约 | 份数 | 实测 |
|---|---|---|
| 主循环 | 2 | Rust `runner/agent_loop.rs:39-48`；TS `core/agent-loop.ts:209`（由 `src/agent/store.ts:3804`、`:2494`、`:2992`、`subagents/runner.ts:128` 驱动） |
| 协议目录 | 2 | Rust `protocol/methods.rs` **76 个方法常量** vs TS `ProtocolCommands` **90 个键**（其中 21 `evt.*` + 3 `req.*`）→ **Rust 有、TS 无 10 个** |
| 主机分发 | 2 | Rust `server/dispatch.rs` 常量臂 vs TS `host/dispatch.ts` 66 个 case |
| 目录 ↔ 实现 | — | `workspace.set`、`config.update` 声明了却**没有 dispatch 臂**，落到 `_ => method_not_found`（`dispatch.rs:2250`） |
| 线 DTO | 3 | `protocol/dto.rs` / `src/shared/protocol/dto.ts` / `tauri-ui/src/types/index.ts` |

**门禁判据**：只要有两份引擎，就必须有"等价性"测试才能证明没漂移；而这个测试本身要维护两套脚手架。
→ **删掉第二份实现，等价性问题消失**，剩下的风险变成"产品之间漂移"，由 §8 的生成+合规闸管住（成本低得多）。

### 1.2 事实：权威路径上有一批"看起来装上了"的部件（新基座必须补上）

| 部件 | 证据 | 后果 |
|---|---|---|
| 插件钩子（25 个点位） | 只在 TS 侧定义并调用（`src/agent/core/events.ts:579-605`）；Rust 全仓只有能力位解析（`plugins/manager.rs:101-110`） | Rust 引擎**整套钩子缺失**；`hook_timeout_ms`/`allow_third_party_hooks` 解析后无处使用 |
| 审批闸门 | `should_ask_approval` 只在 `approval/mod.rs:23-96` 的测试里调用；`ApprovalManager::register_waiter` 唯一调用点在 `#[cfg(test)]` 的 `dispatch.rs:2356`；`config.approval` 只写不读（`dispatch.rs:1238-1240`、`state/store.rs:117`） | ask/readonly 档位**零效力**，审批 UI 建在空接线之上 |
| 模型重试 | `max_retries` 只在 `ai/types.rs:159` 定义、`agent_loop.rs:98` 与 `subagents/runner.rs:153` 赋值，**全仓无读取点** | "重试 3 次"是幻觉 |
| 取消贯穿 | `run_command(..., None)`（`executor.rs:117`）；子智能体 `abort_rx: None`（`executor.rs:157`） | 用户点停止，**子进程与子智能体收不到信号** |
| `terminate` | 插件沙箱写入（`plugins/sandbox.rs:254`）后被 executor 丢弃（`executor.rs:236-249`），模型里没有该字段（`session/types.rs:71-96`） | §5 的语义在 Rust 上不可达 |
| 禁用内置插件 | `execute_builtin_plugin_tool`（`executor.rs:177`）在 `enabled` 检查（`:184-196`）**之前** | 禁用只影响工具声明，不影响执行 |
| 附件图片 | `ChatCompletionMessage.content` 只有 `Option<String>`（`ai/types.rs:146`），`format_messages_for_model` 丢 `images`（`runner/prompt.rs:222-229`） | 界面能预览，**模型看不到图** |
| 工具 schema | 内置插件工具参数全为空对象（`plugins/builtins.rs:104`、`:114`），含 `ask_user`（`:77`），而 handler 在读 `question/choices/allow_text`（`builtin_tools.rs:17-67`） | 模型只能瞎猜参数 |
| 空壳工具 | `check_gate`/`evaluate_diff`/`manage_ponytail` 恒回成功文案（`builtin_tools.rs:241-246`） | 直接违反「绝不捏造确定性」（AGENTS.md §9） |

**门禁判据**：这些不是"编码助手特有问题"，而是**基座缺机制**。新设计必须让它们成为**类型或合规断言**，
否则换个产品照样复发（§3 的 INV 表就是干这个的）。

### 1.3 事实：产品假设写死在"通用"代码里（多产品能不能成立的关键）

- 人格与语言写死：`runner/prompt.rs:169-179`（"你是 a-da…AI 编程智能体…中文"）。
- 工具集写死：`prompt.rs:7-128` 的 7 个编码工具 schema + `executor.rs:48-207` 的 8 个名字分支。
- 载体写死：`workspace: &Path` 同时是沙箱基准、会话分组键、检查点目录键、插件扫描根（`agent_loop.rs:40`、`session/manager.rs:73-80`、`checkpoint/manager.rs:39`、`prompt.rs:135`）。
- 默认 provider 兜底写死 `gemini-2.5-flash`（`executor.rs:138-148`）。
- 一份"工具元数据"散在五处：`tools/mod.rs:13-36`（读写名单）、`approval/types.rs:15-18`（命令工具/危险子串）、`executor.rs:59-66,88-95`（检查点手写分支）、`subagents/types.rs:44`（白名单）、`plugins/types.rs:44`（插件自述 `is_write`）。

**门禁判据**：生活类助手不需要 `fs`/`command`/`git`，也不需要"统一 diff / 写前快照"这套
**补丁语义**（那是 coding 专有概念）。所以这些必须从内核降级为**可选工具包 + 可选能力位**。

### 1.4 TS 归档的具体处置（迁移期动作）

| 对象 | 处置 | 理由 |
|---|---|---|
| `src/agent/**`（循环、宿主、工具、store） | **移入 `archive/ts-agent/`（只读保留一个 tag），并从构建与测试中摘除** | 不再是参考设计；留着就会被误用（§1.1 的等价性负担） |
| `app.tsx` 的 `--host` 分支、`src/ui/client/in-process.ts`、`host-bootstrap.ts` 的兜底与 `A_DA_FORCE_LEGACY_HOST` | 删除 | TS 宿主消失后这些是死路；**GPUIX 客户端改为必须搭档 `agent_core.exe`**（现状默认路径本来就是它，`host-bootstrap.ts:94-116`） |
| `ts_engine` + `plugins/sandbox.rs` | **保留为可选适配器**（插件代码运行时），但插件**契约**由 Rust 侧的 `PluginHost` 端口定义 | 插件生态是资产；契约必须单源 |
| `src/shared/protocol/*.ts` | 改为**生成物**（客户端类型），不再手写 | 治 §1.1 的第三份 DTO |
| `kernel/`、`compiler/` shim（`kernel/mod.rs:1-6`、`compiler/mod.rs:1-2`） | 删除（`ts_engine` 直接依赖） | 纯转发，无消费者 |
| `docs/jsonrpc-protocol.md` 中描述 TS 形态的段落（§0.1 客户端是 GPUIX、§16.2 jiti） | 重写为 Rust-only 口径 | 文档与实现必须同代 |

---

## 2. 目标形态与不变量

### 2.1 三条目标

- **G1 快**：`new-agent` 到"能跑、协议一致、合规全绿" ≤ 半天；产品 crate 只写声明。
- **G2 一致**：base 协议与规范在所有产品间**同一份**；产品差异全部显式声明且可机械校验。
- **G3 不可静默失效**：任何"声明了没实现 / 实现了没声明 / 方向不对"都由 `verify` 或 `conformance` 拦下。

### 2.2 不变量（INV，规范正文）

| # | 不变量 | 对治的现状 |
|---|---|---|
| INV-1 | **单一引擎**：仓库内只有 `agent-base` 一份多轮循环；产品不得自带循环 | 双引擎漂移（§1.1） |
| INV-2 | **端口必填**：契约性端口方法禁止默认实现；不支持 → 显式 `Unsupported` | 钩子整套缺失（§1.2） |
| INV-3 | **注册表即真源**：工具的只读性/审批/回滚/执行模式/终止语义全部来自 `ToolDescriptor`；任何按名字的旁路名单都是缺陷 | 五处名单（§1.3） |
| INV-4 | **失败方向在类型里**：`FailDirection`，默认 `Closed` | §2/§9/§12 只在文档里 |
| INV-5 | **回执结构化且必填**：`status`/`started_at`/`finished_at` 非 `Option`，`duration` 派生 | AGENTS.md §18 的双写兜底 |
| INV-6 | **事件有序可重放**：事件带 `(seq, thread, turn)`；快照带 `seq`；缺口可重同步；未实现的能力必须在握手里如实降级 | 快照无 seq、无 resync |
| INV-7 | **领域与投影分离**：`Item`/`ClientSnapshot` 是投影；领域里不出现 `settings_open` 这类界面状态 | DTO 就是 UI 形状 |
| INV-8 | **无隐藏全局态**：端口由组合根注入；单例/`get_app_home()`/`PluginManager::new()` 只许存在于适配器内部 | `global_question_manager()`、每轮重扫盘 |
| INV-9 | **兼容在适配层**：旧字段名/旧会话版本只在 serde 适配与投影层处理，不进领域模型 | `session/types.rs:56-95` 的 alias |
| INV-10 | **能力先声明**：新产品要的新能力 = 内核加能力位 + 适配器实现 + 合规断言；**禁止 fork 引擎** | 防产品分叉 |
| INV-11 | **协议单源**：base 协议一份，产品扩展在 `x.<product>.*` 命名空间内声明；四份产物（Rust 常量/dispatch 臂/客户端类型/文档）由生成器产出并校验 | §1.1 的目录漂移 |
| INV-12 | **归档不可复活**：CI 断言仓库内不存在第二份引擎/宿主实现 | TS 那套的回归 |

---

## 3. 基座内核 `agent-base`

零 IO、零产品名词、零传输。**它只认识四件事：会话状态、模型流、工具调用、策略与事件。**

> **落地进度（2026-10-08）**：`crates/agent-base` 已建，`model/`（模型面类型）、`domain/`（消息 / 工具描述符与回执 /
> 事件 / 错误与失败方向）、`ports/`（Clock / AppHome / EventSink / CancelToken / Scope / Tool(Catalog) /
> ModelClient / ApprovalGate / SessionStore / PromptSource，全部无默认实现）、`engine/`（`AgentRuntime::run_turn` /
> `RunPolicy` / `TurnRequest` / `TurnOutcome` / 纯函数消息格式化）、`testing/`（FixedClock / RecordingSink /
> NeverCancel / ManualCancel / TempAppHome / InMemorySessionStore / FixedPrompt / MockScope / RecordingApprovalGate /
> MockTool / InMemoryToolCatalog / ScriptedModelClient）已全量落地并完成 8 组核心合规单测。
> `AppHome` 与 `Clock` 已在生产路径接线；详细批次见计划 §8。

### 3.1 领域类型（不含产品词汇）

```rust
pub struct ThreadId(String); pub struct TurnId(u64); pub struct CallId(String);
pub struct Timestamp(i64);                    // 由 Clock 产出

pub struct Turn { id: TurnId, started_at: Timestamp, ended_at: Option<Timestamp>,
                  stop: Option<StopReason>, steps: Vec<Step> }

pub enum StopReason {                         // INV: 有预算必须自报，不许悄悄截断
    Completed, Aborted, ModelError { message: String },
    BudgetExhausted { limit: Budget }, Denied { by: DeniedBy },
}

pub struct Message { id: MessageId, role: Role, at: Timestamp,
                     content: Vec<ContentPart>, usage: Option<Usage> }

pub enum ContentPart {                        // 附件/思考/工具都是 part，不再是 Option<String>
    Text(String), Thinking(String), Image(ImageRef),
    ToolCall(ToolCall), ToolResult(ToolReceipt),
}

pub struct ToolDescriptor {                   // INV-3：一个结构取代五处名单
    name: ToolName, summary: String, schema: Json,
    access: Access, approval: ApprovalPolicy,
    rollback: RollbackPolicy, execution: Execution, termination: Termination,
}
pub enum Access { ReadOnly, Mutates { paths: PathSelector }, Executes { command_arg: &'static str } }
pub enum ApprovalPolicy { Never, Always, DangerScan { patterns: Vec<String> }, Named(&'static str) }
pub enum RollbackPolicy { None, SingleTarget, PerTargetInBatch }   // coding 的"检查点"是它的一个实现
pub enum Execution { Sequential, ParallelSafe }
pub enum Termination { ContinueTurn, EndTurn }

pub struct ToolReceipt {                      // INV-5
    status: ToolStatus, output: String, data: Option<Json>, details: Option<Json>,
    started_at: Timestamp, finished_at: Timestamp,
}
impl ToolReceipt { pub fn duration(&self) -> Duration }
```

注意 `RollbackPolicy` 取代了 v0.1 的 `CheckpointPolicy`：**"写前快照 + revert"是 coding 的实现选择，
不是内核概念**；生活类助手的"操作可撤销"由同一个端口的不同适配器实现（§5）。

### 3.2 端口（INV-2：无默认实现）

```rust
#[async_trait] pub trait ModelClient: Send + Sync {
    fn capabilities(&self) -> ModelCapabilities;
    async fn stream(&self, req: CompletionRequest, cancel: CancelToken) -> Result<DeltaStream, ModelError>;
}
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> &ToolDescriptor;
    fn execute<'a>(&'a self, call: &'a ToolCall, ctx: &'a ToolContext) -> BoxFuture<'a, ToolReceipt>;
}
pub trait ToolCatalog: Send + Sync {           // 取代 get_all_tools_for_workspace 每次扫盘
    fn descriptors(&self) -> Vec<ToolDescriptor>;
    fn resolve(&self, name: &ToolName) -> Result<Arc<dyn Tool>, ToolError>;   // 未知 => Err
    fn validate(&self, consumers: &[Consumer]) -> Vec<ContractViolation>;
}
#[async_trait] pub trait ApprovalGate: Send + Sync {
    fn policy(&self) -> &ApprovalPolicySet;
    fn direction(&self) -> FailDirection;
    async fn decide(&self, req: ApprovalRequest, cancel: CancelToken) -> ApprovalOutcome;
}
#[async_trait] pub trait SessionStore: Send + Sync { load / append / list }
#[async_trait] pub trait RollbackStore: Send + Sync { capture / revert / list }   // 可选能力
pub trait PromptSource: Send + Sync { fn system_prompt(&self, mode: Mode) -> String; }
pub trait ScopePolicy: Send + Sync { fn resolve(&self, raw: &str) -> Result<ScopedRef, DenialKind>; }
pub trait PluginHost: Send + Sync {            // 钩子 + 外部工具（TS 沙箱是它的一个实现）
    fn hooks(&self) -> Vec<HookPoint>;
    async fn run_hook(&self, point: HookPoint, input: HookInput, timeout: Duration) -> HookOutcome;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
}
pub trait EventSink: Send + Sync { fn emit(&self, ev: AgentEvent); }
pub trait Clock: Send + Sync { fn now(&self) -> Timestamp; }
pub trait CancelToken: Clone + Send + Sync { fn is_cancelled(&self) -> bool; }
```

`ScopePolicy` 取代 `workspace: &Path`：**"作用域"由产品定义**——coding 是工作区目录，
生活类是"用户数据根 + 白名单资源（日历/联系人）"，通用助手可能只是"会话沙箱"。

### 3.3 引擎契约（一份实现，产品共用）

```rust
pub struct AgentRuntime { model, tools, approval, store, rollback: Option<..>,
                          prompt, scope, plugins: Option<..>, clock, policy: RunPolicy }

impl AgentRuntime {
    pub async fn run_turn(&self, req: TurnRequest, sink: &dyn EventSink, cancel: CancelToken)
        -> Result<TurnOutcome, AgentError>;
}
pub struct RunPolicy { max_steps: Option<u32>, max_parallel_tools: usize, tool_timeout: Duration }
```

引擎必须保证（= 合规套件断言项）：

1. `ApprovalGate::decide` 在每次受策略约束的调用前**被调用一次**；`Denied` 以**工具结果**回模型，不抛错。
2. `ToolCallStarted` 必先于同 `CallId` 的 `ToolCallFinished`；每轮**恰好一个** `TurnFinished`（现状 `had_error` 分支不发收尾，`agent_loop.rs:154-156`）。
3. `CancelToken` 必须传进 `ModelClient::stream` **与** `Tool::execute`（治 §1.2 的取消断链）。
4. 无工具调用即结束，**无隐式步数上限**；有预算必自报。
5. 每条 `ToolReceipt` 落库并带起止时间（INV-5）；`RollbackPolicy != None` 时先拍快照再执行。
6. `PluginHost` 存在的实现，每个已声明钩子点位都必须在固定位置被调用（"声明了没调用"是合规红灯）。
7. 引擎内禁止 `std::fs`/`reqwest`/`std::env`/`Instant::now`（关键词门 + `cargo tree`）。

---

## 4. 产品规格 `agent.spec.json`（"快速开发"的核心）

**产品的全部差异都住在这一个文件里**，其余是生成的。

```jsonc
{
  "id": "ada-life",
  "archetype": "assistant",              // assistant | life | coding | custom
  "identity": {
    "name": "小日子",
    "persona": "prompts/system.md",      // 人格包是文件，不是代码
    "locale": "zh-CN"
  },
  "toolkits": ["core", "web", "calendar", "reminder", "notes"],
  "capabilities": {                      // 声明即契约：未列出的能力位在握手里回答 false
    "images": true, "streaming": true, "rollback": true,
    "subagents": true, "plugins": ["rss"], "hooks": ["beforeTurn","afterToolCall","beforeApproval"]
  },
  "policies": {
    "approval": { "default": "ask", "autoApprove": ["web_search","calendar_list"],
                  "dangerScan": ["delete","transfer"] },
    "budget": { "maxSteps": null, "toolTimeoutSec": 120, "parallelTools": 2 },
    "failDirection": { "approval": "closed", "gate": "open" }   // INV-4：方向写在声明里
  },
  "scope": { "kind": "user-data", "roots": ["{appData}"], "deny": ["{appData}/secrets"] },
  "store": { "kind": "fs", "layout": "{appHome}/sessions" },
  "rollback": { "kind": "snapshot" },
  "transport": ["inproc", "ws", "cli"],
  "protocol": { "base": "1.0", "ext": ["proto/ext.json"] }   // 产品扩展方法
}
```

**三个产品的差异化矩阵（引擎零改动）：**

| 维度 | 通用 AI 助手 | 生活类助手 | CODING 助手 |
|---|---|---|---|
| 人格包 | `prompts/assistant.md` | `prompts/life.md` | `prompts/coding.md` |
| 工具包 | `core`(todo/ask_user) + `web` | `core` + `web` + `calendar` + `reminder` + `notes` | `core` + `fs` + `command` + `git` + `patch` |
| 能力位 | images, subagents | images, subagents, rollback(生活操作可撤销) | images, subagents, rollback(写前快照), plugins |
| 作用域 | 会话沙箱 | 用户数据根 + 资源白名单 | 工作区目录 |
| 审批默认 | ask | ask（转账/删除二次确认） | readonly 档位 + 危险命令扫描 |
| 协议扩展 | `x.assistant.chat.*` | `x.life.calendar.*`、`x.life.reminder.*` | `x.coding.change.*`（现有 `change.*` 改造） |
| 传输 | inproc + ws | inproc + ws（手机端后续） | inproc + ws + cli |
| **改动内核？** | 否 | 否 | 否 |

**声明即契约的两条硬规则**（`verify-spec`）：
① 声明里的每个工具/能力位/方法都必须能被装配（**不许"声明了没有实现"**）；
② 装配出的每个端口实现都必须在声明里出现（**不许"实现了却没声明"**）。
这两条正是把现状「`workspace.set` 声明了却无实现」「能力位解析了却不用」变成红灯。

---

## 5. 工具包 `agent-toolkit`

工具不是自由函数，而是**可声明、可校验、可复用的包**：

```rust
pub trait Toolkit: Send + Sync {
    fn id(&self) -> ToolkitId;                     // "fs" / "web" / "calendar"
    fn tools(&self, spec: &AgentSpec) -> Vec<Arc<dyn Tool>>;
    fn required_capabilities(&self) -> CapabilitySet;   // 例如 fs 需要 scope=workspace
    fn validate(&self, spec: &AgentSpec) -> Vec<ContractViolation>;
}
```

| 包 | 工具 | 需要的端口/能力 | 备注 |
|---|---|---|---|
| `core` | `todo`、`ask_user`、`finish` | 无 | 所有产品必带；`ask_user` 的 schema 必须与 handler 对齐（现状 `plugins/builtins.rs:104` 是空对象） |
| `fs` | `read_file`/`write_file`/`edit_file`/`list_files`/`search_files` | `ScopePolicy=workspace`、`RollbackPolicy∈{SingleTarget,PerTargetInBatch}` | 现 `tools/fs_tools.rs` 整体搬迁 |
| `command` | `run_command` | `ScopePolicy`、审批 `DangerScan`、**必须接 `CancelToken`** | 现 `cmd_tools.rs`；治 §1.2 取消断链 |
| `git` | `git_status/diff/log`（+ 可选写操作） | `command` | 现内置插件 `plugins/builtins.rs:19-21` |
| `patch` | `apply_patch`/`batch_write`/`batch_replace` | `fs` + `RollbackStore` | 现 `builtin_tools.rs:201-240`（当前**不拍快照**，迁移必修） |
| `web` | `web_search`/`fetch_url` | 出网策略 | 通用/生活类主力 |
| `calendar`/`reminder`/`notes` | 各自的读写工具 | `ScopePolicy=user-data` | 生活类主力；与 coding 完全无关，验证内核中性 |

**搬迁纪律**：`Toolkit` 只依赖「端口 + 声明」，**不许**出现 `if workspace…`、不许读环境变量、不许自己 new 管理器。
现状里 `executor.rs:180-181`（每次未知工具重扫磁盘插件）与 `prompt.rs:134-135`（每轮重扫）在基座里**不可能存在**——
因为工具表来自 `ToolCatalog::descriptors()`，而 catalog 由组合根装配一次、在 scope/插件事件时刷新。

---

## 6. 协议规范 `agent-proto` 与生成闸

### 6.1 三层协议

| 层 | 内容 | 谁定义 |
|---|---|---|
L0 **base 协议** | `session.*`、`thread.*`（create/send/abort/retry/editAndResend/compact）、`queue.*`、`approval.*`、`question.*`、`subagent.*`、`evt.turn/tool/*`、能力协商、错误码 | 基座，版本化，**所有产品逐字节一致** |
| L1 **能力位** | `images`、`rollback`、`plugins`、`hooks`、`events.granularity`、`resync` … | 基座登记，产品声明启用 |
| L2 **产品扩展** | `x.<product>.<domain>.*`（如 `x.life.calendar.list`） | 产品在 `proto/ext.json` 声明，生成器并入该产品的目录 |

命名判据：**方法里的名词属于"agent 领域"还是"产品世界"**——`thread.send` 属 L0，`calendar.list` 属 L2；
现有 `fs.*`/`change.*`/`plugin.*`/`provider.*`/`debug.*` 在 ada-coding 里属 L2（它们不是 agent 通用面）。

### 6.2 单源与产物（INV-11）

```
spec/proto/base.json      ← 唯一手写（L0 + L1 登记表）
spec/proto/<product>.ext.json
        │  cargo xtask gen
        ├─► agent-proto/src/methods.rs         （Rust 常量 + 参数/结果类型）
        ├─► crates/agent-proto/client-ts/*.ts  （客户端类型，供 tauri-ui 用）
        ├─► products/<id>/src/dispatch_ext.rs  （只含产品扩展臂；L0 臂由基座提供）
        ├─► docs/protocol/<product>.md         （文档表格块）
        └─► spec/__generated__/catalog.json    （供校验）
        │  cargo xtask verify
        └─► 断言：base 各产品一致 · 目录 = base ∪ ext · 四份产物集合相等 · 无孤儿臂/无无臂常量
```

`verify-proto` 会立刻报出今日的漂移（附录 A：10 个 Rust-only 方法、2 个无臂常量、TS 多出的 24 个主题），
**这正是它存在的意义**。

### 6.3 校验机制落在生成的类型上

方法表带 `kind`（`command`/`event`/`server_request`）与 `since`（协议版本），生成器据此产出：
`enum` 形式的方法常量（不再散装 `&str`）、参数/结果的 strong type、客户端联合类型、文档表。
dispatch 侧用 `match` 覆盖枚举，**穷尽性由编译器保证**——"声明了没实现"在 Rust 里直接编译失败，
不再需要 §1.1 那种人工对账。

---

## 7. 运行时装配、宿主与交付

```rust
// products/ada-life/src/main.rs —— 全部内容
#[tokio::main] async fn main() -> anyhow::Result<()> {
    agent_host::run_from_spec(include_str!("../agent.spec.json")).await
}
```

```rust
// agent-runtime：组合根（唯一允许读声明/环境/路径的地方，INV-8）
pub struct ProductBuilder { spec: AgentSpec }
impl ProductBuilder {
    pub fn build(&self, model: Arc<dyn ModelClient>, ...) -> Result<AgentRuntime, SpecError>;
    pub fn validate(&self) -> Result<(), Vec<ContractViolation>>;   // verify-spec 的实现
}
```

| 关注点 | 设计 |
|---|---|
| 传输 | `agent-host` 提供 `inproc`（同进程库调用）、`ws`（现协议文档 §1.1 的唯一远程传输，含 token 握手）、`cli`（headless，验证"零 UI"）三种宿主，产品在 `transport` 里声明 |
| 进程角色 | 保留"单 exe 双角色"（`--host`）形态：`tauri-ui` 与 GPUIX 客户端既可直接 lib 调用，也可 spawn 自己当主机 |
| 启动期自检 | `validate()` + `conformance` 的快速子集在启动时跑；能力位与声明不一致 → 直接失败（不许"静默降级"） |
| 交付 | 每个产品一个单文件 exe；`cargo xtask ship --product <id>` 统一产出（沿用现 `scripts/build.ts` 的 PE 补丁等步骤） |
| 客户端类型 | `crates/agent-proto/client-ts/` 生成物；`tauri-ui` 与 GPUIX 客户端 import 它，不再各写一份 DTO |

---

## 8. 合规套件 `agent-conformance`（产品线的闸门）

### 8.1 端口契约（每个适配器必跑）

| 端口 | 断言要点 |
|---|---|
| `ModelClient` | 流可拼接、`stop_reason` 合法、取消后停止产出、**能力声明与实际行为一致**（声明不支持图片就不得收到图片 part） |
| `Tool`/`ToolCatalog` | `resolve` 未知名返回 `Err`；schema 非空；`validate` 对五类消费者全绿；回执字段齐全 |
| `ApprovalGate` | 每次受约束调用前被调一次；`Denied` 以工具结果回模型；`answeredBy` 如实（user/policy/timeout/aborted）；无依据时按 `direction()` |
| `SessionStore` | append→load 往返等价；旧格式只在 compat 层；并发 append 不丢 |
| `RollbackStore` | capture→revert 往返等价；`PerTargetInBatch` 必须逐目标；`snapshot_incomplete` 必须被如实上报（现 `checkpoint/manager.rs:163` 的 >5MiB 降级） |
| `PluginHost` | 每个声明点位都有"副作用断言"用例；超时/失败按 `FailDirection`；**"声明了没调用"是红灯** |
| `ScopePolicy` | 越界一律拒绝（现 `tools/sandbox.rs:73-121` 的判定用例直接搬） |
| `EventSink` | 事件带 `seq`；started/finished 配对；取消时恰好一个 `TurnFinished(Aborted)` |

### 8.2 跨端口不变量（每产品必跑）

① 同进程可跑两个 runtime 且互不干扰（无隐藏全局态）② 注册表完整性（声明↔实现双向）③ 事件 `seq` 单调
④ 取消贯穿到工具与子进程 ⑤ 失败方向与声明一致 ⑥ 回执完整性 ⑦ 钩子点位全覆盖 ⑧ 归档断言（无第二份引擎）。

### 8.3 产品级验收（"协议和规范都一致"的定义）

```bash
cargo xtask verify            # base 一致 + 声明↔实现双向 + 无孤儿臂
cargo test -p <product>       # 8.1 + 8.2
cargo xtask compat --all      # 所有产品的 base 协议哈希相等；L1/L2 并集无冲突
```

CI 对**每个产品**跑同一套；新产品的合并条件是这套全绿。**一致性从此不是"靠人记住"，
而是"合并的必要条件"。**

---

## 9. 脚手架与产出物

```
crates/
  agent-base/          内核（domain/ports/engine/policy/events）— 唯一引擎
  agent-proto/         协议 + 生成器 + 客户端类型
  agent-runtime/       组合根（ProductBuilder / validate / 装配）
  agent-host/          传输宿主（inproc/ws/cli，单 exe 双角色）
  agent-toolkit/       core/fs/command/git/patch/web/calendar/reminder/notes…
  agent-adapter/       model-openai/anthropic、store-fs/sqlite、plugin-ts/plugin-rust、scope-*
  agent-conformance/   端口契约 + 跨端口不变量
products/
  ada-coding/          agent.spec.json + prompts/ + proto/ext.json + src/main.rs(~30 行) + tests/conformance.rs(1 行)
  ada-assistant/
  ada-life/
spec/proto/{base.json,<product>.ext.json}
tools/xtask/           new-agent | gen | verify | verify-spec | verify-archive | ship | compat
```

`cargo xtask new-agent --archetype life` 产出一个**编译即可通过 verify 与 conformance** 的骨架，
之后开发者只编辑 `agent.spec.json` 与 `prompts/`。**"写一个 agent core"= 写声明；"写新能力"= 写适配器 + 能力位。**

---

## 10. 迁移路径（Rust-only，含 TS 归档）

| 阶段 | 动作 | 验收 | 风险 |
|---|---|---|---|
| **P0 规范与闸先行**（不加引擎代码） | 写 `spec/proto/base.json` + `xtask gen/verify`；**先把今日 Rust 实现反推成 spec 并冻结**；`verify-archive` 就位 | `gen` 产出的 Rust 常量与 `protocol/methods.rs` 等价；`verify` 报出 F2 的 10+2 处漂移（预期红） | 无 |
| **P1 内核抽出** | 建 `agent-base`（3.1–3.3）；`agent_core::runner` 改为"基座引擎 + ada 适配器"；事件名字与负载**保持兼容**（前端不动） | 现 `bun test` 800 项全绿；`cargo check --all-targets` 绿；新增**冻结回放测试**：脚本化模型 → 事件序列与 P0 冻结的产物逐字段相等（不再依赖 TS 对照） | 中：abort 时序、usage 累加 |
| **P2 注册表化 + 补空接线** | 7 个内置工具与 12 个内置插件工具改 `Toolkit`+`ToolDescriptor`；删 5 处名单；接上审批/取消/`terminate`/图片/回执；空壳工具要么实现要么删声明 | §1.2 每条都有对应合规断言；`run_command` 真正接取消；`ask_user` schema 与 handler 对齐 | 中高（行为面广，正是目的） |
| **P3 协议单源 + 投影** | 方法表生成物替换手写；`snapshot` 加 `seq`；`UiSnapshot` 出领域；`fs.*`/`change.*`/`plugin.*` 归入 `x.coding.*` | `verify`/`compat` 全绿；前端通过生成类型编译 | 中 |
| **P4 TS 归档** | `src/agent/**` → `archive/ts-agent`；删 `app.tsx --host`、`in-process.ts`、legacy 兜底、`kernel`/`compiler` shim；`ts_engine` 收敛为插件适配器 | `verify-archive` 绿；GPUIX 与 Tauri 客户端都走 `agent_core.exe`；插件启停/工具调用测试仍绿 | 中：插件链路回归 |
| **P5 第二个产品** | `ada-life`（或 `ada-assistant`）**只写声明**接入；用它暴露基座缺的能力位 | 第二产品零内核改动即可通过 verify + conformance；差异 < 200 行声明 | 低（这一步是设计的验收） |

**存量缺陷处理原则**：P2 只"登记"§1.2 里的每一条，然后逐条决定**补实现**或**删声明**；
**禁止**把它们静默带进新基座——那正是本次重构的目的。

---

## 11. 危险区（抽象别把静默坑合法化）

| 反模式 | 为什么它把坑合法化 | 判据（出现即回退） |
|---|---|---|
| 端口给默认空实现 | "声明了没调用"变成"允许不实现" | 契约端口出现 `default` 实现即红灯（读 trait 定义的测试） |
| 契约字段用 `Option` 兜 | 回执缺字段 → 界面静默变空（§18） | `status`/`started_at`/`finished_at` 非 `Option` |
| 保留按名字的工具名单 | "多处登记"换个位置继续存在 | 基座/适配器出现 `matches!(name, …)` 名单 → 关键词门 |
| 用 `Option<Port>` 表达"这个产品不需要" | "没接"与"不支持"无法区分 | 用能力位 + `Unsupported`，不用 `None` 静默跳过 |
| 产品为了差异 fork 引擎/复制 dispatch | 漂移从"语言之间"变成"产品之间" | `verify-archive` + 依赖规则（产品只能依赖 runtime/proto/base） |
| 能力位解析了没消费者 | 现在的 `hook_timeout_ms`/`allow_*` 就是这样被扔掉的 | 每个能力位必须有一个"被读取"的测试，否则删除 |
| 把产品概念塞进内核（`workspace`、`patch`、`calendar`） | 内核变成某个产品的形状，其他产品只能将就 | `agent-base` 关键词门：出现产品名词即失败 |
| "先粗后细"当挡箭牌 | 协议文档里 21 个未实现 topic 已挂三个版本 | 未实现的能力必须在握手里回答 false，且文档块由生成器产出 |

---

## 12. 开放决策（已收敛，待你确认）

| # | 决策 | 建议 | 影响 |
|---|---|---|---|
| 1 | **TS 归档形态** | 移入 `archive/ts-agent/` 并摘出构建（不是立刻删除） | 保留考古能力，同时 `verify-archive` 保证不被引用 |
| 2 | **插件运行时** | 保留 `ts_engine` 作可选 `PluginHost` 实现；契约由 Rust 定义；产品在 `capabilities.plugins` 里声明 | 插件生态不动，内核不沾 TS |
| 3 | **GPUIX 客户端** | 冻结在"必须搭档 `agent_core.exe`"（现状默认路径）；新功能只投 `tauri-ui` | 删掉 legacy 兜底，减少一条并行路径 |
| 4 | **首个第二产品** | `ada-life`（与 coding 差异最大，最能暴露内核里的产品假设） | 这是设计的真正验收 |
| 5 | **协议扩展命名** | 统一 `x.<product>.*`，现有 `fs/change/plugin/provider/debug` 归入 `x.coding.*`（保留旧名做别名一个版本） | 老客户端平滑 |
| 6 | **文档处置** | `docs/jsonrpc-protocol.md` 重写为 Rust-only 口径，表格块改为生成物；`docs/agent-base-design.md`（本文）作为设计源 | 文档与实现同代 |

---

## 附录 A：现状结论的证据索引

| 结论 | 证据 |
|---|---|
| Rust 是默认权威宿主 | `src/ui/client/host-bootstrap.ts:79-116`；`src-tauri/src/lib.rs:84-118`；`scripts/build.ts:70-107` |
| TS 循环仍可达（故需归档而非忽视） | `src/agent/store.ts:3804`、`:2494`、`:2992`；`src/agent/subagents/runner.ts:128`；`src/agent/core/agent-loop.ts:209` |
| 协议目录漂移 | `protocol/methods.rs`（76 方法常量）vs `src/shared/protocol/methods.ts`（90 键，含 24 evt/req）；`workspace.set`/`config.update` 无 dispatch 臂 → `server/dispatch.rs:2250` |
| TS 分发规模 | `src/agent/host/dispatch.ts` 66 个 case |
| 钩子只在 TS 且 Rust 零调用 | `src/agent/core/events.ts:579-605`(:614/:637)；`grep hook` 仅 `plugins/manager.rs:101-110`、`lib.rs:315-321`；`plugins/sandbox.rs:24,140` 只有 inspect/call |
| 审批空接线 | `approval/mod.rs:23-96`（仅测试）、`manager.rs:18`、`dispatch.rs:2356`（`#[cfg(test)]`）、`dispatch.rs:1238-1240`、`state/store.rs:117` |
| `max_retries` 无读取点 | `ai/types.rs:159`、`agent_loop.rs:98`、`subagents/runner.rs:153` |
| 取消断链 | `executor.rs:117`（run_command None）、`executor.rs:157`（subagent abort None） |
| `terminate` 被丢弃 | `plugins/sandbox.rs:254` → `executor.rs:236-249`；`session/types.rs:71-96` 无该字段 |
| 禁用插件仍可执行 | `executor.rs:175-178` vs `:184-196` |
| 内置插件 schema 为空 | `plugins/builtins.rs:104`、`:114`（含 `ask_user` :77） |
| 图片被丢弃 | `ai/types.rs:146`；`runner/prompt.rs:222-229` |
| 空壳工具恒成功 | `runner/builtin_tools.rs:241-246` |
| 每轮/每次重扫插件 | `prompt.rs:134-135`、`:181-182`；`executor.rs:180-181` |
| 五处工具名单 | `tools/mod.rs:13-36`、`approval/types.rs:15-18`、`executor.rs:59-66,88-95`、`subagents/types.rs:44`、`plugins/types.rs:44` |
| 检查点只在两处且不解批量 | `executor.rs:59-66,88-95`；`builtin_tools.rs:156`（`_checkpoint_mgr` 未使用） |
| 产品假设写死 | `runner/prompt.rs:169-202`；`prompt.rs:7-128`；`executor.rs:138-148`；`agent_loop.rs:40` |
| 组合根写死 | `server/ws.rs:49-70`；`src-tauri/src/lib.rs:92-96` |
| 事件被两份手写 match 消费 | `server/dispatch.rs:542-600`、`:1570-1600` |
| 合帧与 seq | `server/emitter.rs:33-72`；每命令后广播 `ws.rs:226-228`；种子快照 `seq:0` `ws.rs:146-161` |
| 死 shim | `kernel/mod.rs:1-6`、`compiler/mod.rs:1-2`、`agent_core/Cargo.toml:28` |
| 协议文档自述的未实现项 | `docs/jsonrpc-protocol.md:106-117`、`:139-142`、`:148-149`、`:547-560`、`:603-615` |

## 附录 B：`spec/proto/base.json` 草案（片段）

```jsonc
{
  "version": "1.0",
  "capabilities": {
    "streaming": true,
    "images": { "since": "1.0", "default": false },
    "rollback": { "since": "1.0", "default": false },
    "plugins": { "since": "1.0", "default": false },
    "hooks": { "since": "1.0", "points": ["beforeTurn","afterTurn","beforeToolCall","afterToolCall",
                                          "beforeApproval","afterApproval","beforeSubagentStart",
                                          "afterSubagentEnd","beforeCompaction","beforePersist"] },
    "events": { "granularity": ["snapshot","delta"], "resync": false }
  },
  "methods": {
    "thread.create":   { "kind": "command", "params": { "scope": "ScopeRef", "mode?": "Mode" },
                         "result": { "threadId": "ThreadId" } },
    "thread.send":     { "kind": "command", "params": { "threadId": "ThreadId", "text": "String",
                                                       "images?": "[ImageRef]" },
                         "result": { "accepted": "bool", "queued?": "bool" } },
    "thread.abort":    { "kind": "command", "params": { "threadId": "ThreadId" }, "result": { "aborted": "bool" } },
    "thread.retry":    { "kind": "command", "params": { "threadId?": "ThreadId" },
                         "result": { "accepted": "bool", "reason?": "String" } },
    "approval.decide": { "kind": "command", "params": { "callId": "CallId", "approved": "bool" },
                         "result": { "callId": "CallId", "approved": "bool",
                                     "answeredBy": "user|policy|timeout|aborted" } },
    "evt.turn.started":  { "kind": "event", "params": { "seq": "u64", "threadId": "ThreadId", "turnId": "TurnId" } },
    "evt.tool.finished": { "kind": "event", "params": { "seq": "u64", "callId": "CallId",
                                                        "receipt": "ToolReceipt" } },
    "evt.turn.finished": { "kind": "event", "params": { "seq": "u64", "turnId": "TurnId",
                                                        "stop": "StopReason" } }
  },
  "errors": { "-32000": "ProtocolVersionMismatch", "-32001": "Unauthorized",
              "-32004": "Denied", "-32005": "Timeout", "-32010": "NeedResync",
              "-32020": "Unsupported" }
}
```

`-32020 Unsupported` 是新增的：INV-2 要求"不支持"有明确的线上表达，而不是静默无响应或假装成功。
