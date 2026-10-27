# 统一事件总线与插件系统设计（基座 + TS 引擎 + 插件系统）

> 状态：**E1 基本落地（2026-10-27）**：`PluginEvent` / `EventOutcome` /
> `EventDispatcher` 契约已加入 `agent-base`；`PluginEventRuntime` 提供有界 lane、
> 异步 worker 与 generation；`HookEventDispatcher` 作为内层适配器折回现有
> `HookRuntime`。引擎 before / after 点位均已走 dispatcher；`ada.on("event_*")`
> 在注册期折回旧点位。纯通知事件投影、EventSpec 静态目录与 TS 类型仍待补。
> 范围：`agent-base` 事件契约、TS 插件事件 API、插件系统的事件分发与生命周期
> 相关：[PLAN-pluginization.md](PLAN-pluginization.md)、[kernel-design.md](kernel-design.md)、
> [../plugins/README.md](../plugins/README.md)、[../plugins/extension-factory.md](../plugins/extension-factory.md)
>
> 本文取代“钩子 + 事件”两套插件 API 的目标形态；`PLAN-pluginization.md` 仍负责
> 插件化路线与安全下线的整体排期。两者冲突时，**事件/钩子模型以本文为准**。

---

## 0. 一页结论

目标分层：

```text
agent-base（零 IO：AGENT 循环 / 领域事件 / EventDispatcher 端口 / 事件词汇）
        ↑
agent-plugin-system（插件发现 / 清单 / 装配 / 订阅 / 生命周期）
        ↑                         ↓
agent-rpc / agent-host      ts-engine（纯 TS 执行原语）
```

- 插件侧只保留一种 API：`ada.on("event_*", handler)`，不再暴露“钩子”这个第二概念。
- 事件统一为 `event_` 前缀的 snake_case 名称；`event_before_*` / `event_after_*`
  表示决策与观察，生命周期与增量事件也用同一命名空间。
- 事件分三种效果：`Intercept`（异步执行，引擎等待结果）、`Decorate`（异步执行，
  引擎短等待以应用旁注）、`Notify`（fire-and-forget）。
- “异步投递”不等于“发完不管”：`event_before_*` 必须是带 deadline 的屏障，
  否则后续插件无法做安全/策略管控。
- 安全与审批暂不执行；`EventOutcome`、超时、失败方向等类型保留，后续由受信插件接入。
- 核心 `AgentEventBody` 与线协议 `evt.*` 主题先不重命名；插件侧通过映射表使用
  `event_*` 名称，避免一次性打断网关、SSR、旧客户端与测试。

---

## 1. 现状与问题

### 1.1 当前两条通道

| 通道 | 落点 | 语义 |
|---|---|---|
| 领域事件流 | `agent-base/src/domain/event.rs` 的 `AgentEventBody`、`ports/event.rs` 的 `EventSink` | 引擎 → 宿主/UI 的**单向通知**，带 `seq` / `thread_id` |
| 钩子点位 | `agent-base/src/ports/hooks.rs` 的 `points` / `HookRunner`；`agent-node/src/hooks` 的 `HookRuntime` | 引擎在决策点**同步/等待**插件返回 `pass/deny/replace/note` |

这套分层本身没有错，但插件作者面对的是两套概念：

- 想观察工具调用，要订阅事件；
- 想拦截工具调用，要注册钩子；
- 两者名称、排序、超时、失败策略、热重载行为各有一套；
- 文档需要同时解释“事件不是钩子”“钩子返回什么”“事件能不能改”等问题。

### 1.2 现有事件与钩子的重叠

- `ToolCallStarted` / `ToolCallFinished` 是领域事件；
- `beforeToolCall` / `afterToolCall` 是决策/旁注钩子；
- `ApprovalRequested` 是领域事件，`beforeApproval` / `afterApproval` 又是钩子；
- 子智能体、上下文压缩、提示词、供应商标头等也有成对的领域事件或钩子。

合并后必须回答：**同一次工具调用，插件应该收到几个事件、按什么顺序、哪个能改结果。**

### 1.3 现有点位并不完全成对

当前 `points::IMPLEMENTED` 有 29 个点位（2026-10-27 已移除 `beforeTodoUpdate` /
`afterTodoUpdate` / `beforeApproval` / `afterApproval`），仍存在不对称：

- `beforePersist` 没有 `afterPersist`；
- `afterCheckpoint` 没有 `beforeCheckpoint`；
- `beforeProviderRequest` / `beforeProviderHeaders` / `afterProviderResponse` 不是对称三元组；
- `beforeAgentStart` 的配对是 `afterAgentEnd`，不是 `afterAgentStart`。

因此“全部改成 `event_before_xxx` / `event_after_xxx`”不能靠机械改名完成；必须先审计每个点位的真实语义，允许**非对称事件名**。

---

## 2. 目标与非目标

### 2.1 目标

- **G1 一个插件 API**：插件只注册 `event_*`，不再区分“事件订阅”和“钩子注册”。
- **G2 一个分发器**：统一排序、超时、失败方向、热重载与收据。
- **G3 能力不丢**：`before` 仍可 deny / replace / note；`after` 仍可 note（需要时）；
  领域事件的 `seq` / 顺序 / 单一真源不破坏。
- **G4 分层不破**：`agent-base` 不依赖 TS/插件发现；`ts-engine` 不依赖 agent 领域模型；
  插件系统只依赖两者提供的契约。
- **G5 安全可后补**：当前不执行审批/沙箱策略，但事件结果类型与失败方向保留，
  后续受信插件可以用 `event_before_*` 接管策略。

### 2.2 非目标

- 不在本次重命名线协议 `evt.*` 主题或 UI 事件；
- 不实现审批弹窗、路径沙箱、插件能力门的恢复；
- 不实现完整 Node EventEmitter 语义；
- 不把 TS 引擎并进 `agent-base`；
- 不做全局事件总线（必须是每运行时实例）。

---

## 3. 分层与依赖

### 3.1 crate 关系

```text
agent-base
  ├─ domain/event.rs        AgentEvent / AgentEventBody（领域事件，保持稳定）
  ├─ ports/event.rs         EventSink（引擎 → 宿主的唯一出口）
  ├─ ports/events.rs        EventDispatcher（插件事件分发端口，新增）
  └─ ports/hooks.rs         事件词汇 / 效果 / 结果类型（由 hooks.rs 演进）

agent-plugin-system（可先放在 agent-node 内，按目录拆分）
  ├─ manifest / discovery
  ├─ event runtime（队列、排序、超时、热重载）
  ├─ hook/event 兼容层
  └─ TS sandbox adapter

ts-engine
  └─ 纯 TS 执行 + ada.on 事件注册（不认识 agent 循环与插件发现）
```

### 3.2 依赖规则

1. `agent-base` 不依赖 `ts-engine`、不依赖插件目录扫描、不读文件系统。
2. `ts-engine` 不依赖 `agent-base`；插件系统负责把两边类型映射成 JSON/事件 DTO。
3. 插件系统实现 `EventDispatcher`；引擎只拿端口，不认识具体插件。
4. `EventSink` 仍是领域事件唯一出口；插件事件总线是它的消费者和拦截层，不是第二事件源。
5. 第一方 Rust 插件与第三方 TS 插件共用**同一套事件名称与效果声明**，差异只在信任级别与可提供的贡献类型。

---

## 4. 事件模型

### 4.1 命名规则

- 统一 snake_case，全部以 `event_` 开头。
- 决策/观察点使用 `event_before_*` / `event_after_*`；不对称点位保留语义名，不硬造配对。
- 生命周期事件使用 `event_turn_started`、`event_turn_finished`、`event_text_delta` 等。
- 同一语义只保留一个名字；旧钩子名做别名映射，作为兼容层输入，不再出现在新文档的主路径。

### 4.2 三类效果

| 效果 | 名称形态 | 投递 | 返回值 | 失败/超时 |
|---|---|---|---|---|
| `Intercept` | `event_before_*` | 异步执行处理器，引擎等待 join | `Pass / Deny / Replace / Note` | 按事件声明：当前 `Pass`，后续可 fail-closed |
| `Decorate` | `event_after_*`（需要旁注时） | 异步执行，引擎短等待 | `Note` 或空 | 超时视为无旁注 |
| `Notify` | `event_*`（生命周期/诊断） | 有界异步队列，fire-and-forget | 忽略 | 丢弃并记诊断，不阻塞引擎 |
| `Stream` | `event_text_delta` / `event_thinking_delta` | 独立合并/丢弃 lane | 忽略 | 可合并、可丢弃 |

关键约束：

- `Intercept` 不能是纯通知，否则无法 deny / replace。
- `Decorate` 若保留“追加旁注”的能力，就必须在最终领域事件发出前完成；否则只能降级为 `Notify`。
- `Stream` 不能和 `Intercept` 共用阻塞队列，否则高频增量会拖慢工具决策。

### 4.3 事件信封

```rust
pub struct PluginEvent {
    pub name: String,          // event_before_tool_call
    pub seq: u64,              // 与领域事件同源；插件事件不另造序号
    pub at_ms: i64,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub call_id: Option<String>,
    pub trace: TraceCtx,
    pub source: EventSource,   // Engine / Plugin / Host
    pub payload: serde_json::Value,
}
```

`payload` 是投影后的 DTO，不是内部结构体的直接序列化；审批参数、命令原文等敏感字段
按事件可见性过滤后才进入插件总线。

### 4.4 事件声明

```rust
pub struct EventSpec {
    pub name: &'static str,
    pub effect: EventEffect,          // Intercept / Decorate / Notify / Stream
    pub allowed_actions: &'static [EventAction],
    pub timeout_ms: u64,
    pub failure: EventFailure,        // Pass / Deny；当前默认 Pass
    pub visibility: EventVisibility,  // Public / TrustedOnly
    pub lane: EventLane,              // Critical / Decorate / Notify / Stream
}
```

事件名、效果、允许动作、超时、失败方向是**单一真源**；插件清单只能引用，不能自造。

### 4.5 结果

```rust
pub enum EventOutcome {
    Pass,
    Deny { reason: String },
    Replace { payload: serde_json::Value },
    Note { text: String },
    AbortTurn { reason: String }, // 保留给受信插件；当前不开放给第三方
}
```

链式语义：

1. 多个 `Intercept` 处理器按“内置优先 → order 升序 → plugin id”执行；
2. `Note` 累积，继续；
3. `Deny` / `AbortTurn` 终止；
4. `Replace` 后对替换载荷重新做 schema/路径/风险校验，再继续剩余处理器；
5. 超时或异常按 `EventSpec.failure` 处理，当前默认 `Pass`。

### 4.6 现有钩子点位的映射

> 2026-10-27：`beforeTodoUpdate` / `afterTodoUpdate` / `beforeApproval` / `afterApproval`
> 已先从核心移除（见 §11.0）。下面保留这四行作为迁移历史；未来统一事件系统
> 需要它们时，用同一组 `event_*` 名称重新引入，而不是恢复旧 hook 常量。

| 现点位 | 新事件名 | 默认效果 | 备注 |
|---|---|---|---|
| `beforeToolCall` | `event_before_tool_call` | Intercept | 可 deny / replace / note |
| `afterToolCall` | `event_after_tool_call` | Decorate | 保留 note 能力时必须早于最终工具结果 |
| `beforeApproval` | `event_before_approval` | Intercept | 后续安全插件入口 |
| `afterApproval` | `event_after_approval` | Notify | 审批已发生，不改事实 |
| `beforePluginToggle` | `event_before_plugin_toggle` | Intercept | 控制面；当前不限制来源 |
| `afterPluginToggle` | `event_after_plugin_toggle` | Notify | |
| `beforeThreadCreate` | `event_before_thread_create` | Intercept | |
| `afterThreadCreate` | `event_after_thread_create` | Notify | |
| `beforeTurn` | `event_before_turn` | Intercept | |
| `afterTurn` | `event_after_turn` | Notify / Decorate | 视是否需要 note |
| `beforeTodoUpdate` | `event_before_todo_update` | Intercept | 可 replace 清单 |
| `afterTodoUpdate` | `event_after_todo_update` | Notify | |
| `beforeAgentStart` | `event_before_agent_start` | Intercept | |
| `afterAgentEnd` | `event_after_agent_end` | Notify | 非 `afterAgentStart`，保留不对称 |
| `beforeCompaction` | `event_before_compaction` | Intercept | |
| `afterCompaction` | `event_after_compaction` | Notify | |
| `beforeLlmRequest` | `event_before_llm_request` | Intercept | |
| `afterLlmResponse` | `event_after_llm_response` | Notify | |
| `beforeThreadDelete` | `event_before_thread_delete` | Intercept | |
| `afterThreadDelete` | `event_after_thread_delete` | Notify | |
| `beforeSubagentStart` | `event_before_subagent_start` | Intercept | |
| `afterSubagentEnd` | `event_after_subagent_end` | Notify | 非 `afterSubagentStart` |
| `beforeSystemPrompt` | `event_before_system_prompt` | Intercept | |
| `afterSystemPrompt` | `event_after_system_prompt` | Notify | |
| `beforePersist` | `event_before_persist` | Notify | 无对应 after |
| `afterCheckpoint` | `event_after_checkpoint` | Notify | 无对应 before |
| `beforeSkillLoad` | `event_before_skill_load` | Notify | |
| `afterSkillLoad` | `event_after_skill_load` | Notify | |
| `beforeProviderRequest` | `event_before_provider_request` | Intercept | |
| `beforeProviderHeaders` | `event_before_provider_headers` | Intercept | 与上者不是同一事件 |
| `afterProviderResponse` | `event_after_provider_response` | Notify | |
| `beforeContext` | `event_before_context` | Intercept | |
| `afterContext` | `event_after_context` | Notify | |

> ⚠️ 表中“默认效果”是设计起点。正式实现前应逐个点位确认载荷能否重新校验；
> 不能重新校验的点位不允许 `Intercept/Replace`，只能 `Notify`。

### 4.7 领域事件的插件侧投影

内部 `AgentEventBody` 与线协议 `evt.*` 名称保持不变；插件侧映射为：

| 内部事件 | 插件事件 | 效果 |
|---|---|---|
| `TurnStarted` | `event_turn_started` | Notify |
| `ThinkingDelta` | `event_thinking_delta` | Stream |
| `TextDelta` | `event_text_delta` | Stream |
| `ToolCallStarted` | `event_tool_call_started` | Notify |
| `ToolCallFinished` | `event_tool_call_finished` | Notify |
| `ApprovalRequested` | `event_approval_requested` | Notify |
| `QuestionAsked` | `event_question_asked` | Notify |
| `SubagentStarted` | `event_subagent_started` | Notify |
| `SubagentFinished` | `event_subagent_finished` | Notify |
| `UsageReported` | `event_usage_reported` | Notify |
| `TurnFinished` | `event_turn_finished` | Notify |
| `Failed` | `event_failed` | Notify |
| `Diagnostic` | `event_diagnostic` | Notify |

注意 `event_before_tool_call` 与 `event_tool_call_started` 是两个不同事件：
前者在工具执行前拦截，后者是引擎已经决定执行后的通知。不要合并成一个名字。

---

## 5. 投递与运行时

### 5.1 每运行时实例

`EventDispatcher` 由插件系统为每个 agent runtime 创建一个实例，禁止：

- `OnceLock` / `static mut` 全局总线；
- 跨工作区共享订阅表；
- 让插件事件总线反向持有引擎。

### 5.2 四条 lane

| lane | 内容 | 队列 | 满/慢时 |
|---|---|---|---|
| Critical | `event_before_*` | 有界，按 thread 保序 | 等待 deadline；超时按 failure |
| Decorate | 需要 note 的 `event_after_*` | 有界 | 短等待；超时丢弃旁注 |
| Notify | 生命周期/诊断 | 有界 ring | 丢弃并记 `event_dropped` 诊断 |
| Stream | 文本/思考增量 | 可合并/丢弃 | 合并或丢弃，不阻塞 |

`event_before_*` 不能因为某个插件卡住而无限等待；`event_after_*` 不能反向阻塞
已经完成的事实。

**当前实现**：`agent-node::hooks::PluginEventRuntime`。四条 lane 各一个有界
`tokio::mpsc` + 单 worker；Critical/Decorate 用 `oneshot` 回传结果，observer
经 `unbounded_channel` 送回调用方（因此诊断仍由引擎发）；Notify/Stream 用
`try_send`，满时 `dropped_notifications` 计数；`bump_generation` 让旧排队事件
在 worker 取到时作废。没有 Tokio 运行时（纯同步测试）时直接回落 `inner`。

### 5.3 顺序

- 同一 `thread_id` 内，事件按 `seq` 递增；
- 一次工具调用的顺序：`event_before_tool_call` → 执行 → `event_after_tool_call`
  （可 note）→ `event_tool_call_finished`；
- `event_after_tool_call` 若不能保证在结果定稿前完成，就必须降级为 Notify，
  不能一边说异步一边声称 note 生效。

### 5.4 热重载

- 每次插件装载分配 `generation`；
- 停用/重载时先摘除订阅，再递增 generation；
- 已入队但未派发的事件，旧 generation 的处理器不再执行；
- 停用收据列出被移除的事件订阅，便于界面显示“这次关掉了什么”。

### 5.5 失败与诊断

- `Intercept`：当前默认 `Pass`；后续安全插件可声明 `Deny`，由 `EventSpec.failure` 决定；
- `Decorate`：超时或异常视为无旁注；
- `Notify`：异常只记诊断，不影响引擎；
- 未知事件名、非法动作、越权订阅在装配期拒绝并出诊断，不允许静默忽略；
- 丢弃/合并的 `Notify/Stream` 事件必须留计数，便于排查“插件为什么没收到”。

---

## 6. TS 引擎侧契约

### 6.1 API

```ts
// @ada-needs none
export default function (ada) {
  ada.on("event_before_tool_call", async (event, ctx) => {
    if (event.tool === "run_command" && dangerous(event.args)) {
      return { action: "deny", reason: "命令被插件策略拒绝" };
    }
    return { action: "pass" };
  });

  ada.on("event_after_tool_call", async (event) => {
    return { action: "note", note: "已由 lint 插件检查" };
  });

  ada.on("event_text_delta", (event) => {
    // 纯观察，返回值忽略
  });
}
```

- `ada.on` 是唯一注册入口；`registerHook` / `beforeToolCall` 等旧名只做别名兼容。
- 处理器返回 `EventOutcome`；`Notify/Stream` 的返回值忽略。
- 未声明的事件名在装配期报错；事件效果由宿主给出，插件不能自行把 `Notify` 改成 `Intercept`。
- TS 引擎只负责执行处理器，不负责排序、队列、超时和热重载；这些在插件系统。
- `ada.emit` 不作为领域事件发布入口；插件如需内部通知，只能使用自己的命名空间，不能伪造引擎事件。

### 6.2 能力边界

- `event_*` 注入与 `@ada-needs` 的能力门无关；事件总线不是 `fetch`/`fs`/`exec` 能力。
- 插件系统在装配期把允许的事件表传给沙箱；未登记的名称直接拒绝。
- 高频 `Stream` 事件默认不订阅；订阅需要在清单中显式声明，避免每个插件都收到全部增量。

---

## 7. 插件系统侧契约

### 7.1 清单

插件声明订阅时只写事件名与可选过滤条件：

```json
{
  "events": {
    "event_before_tool_call": { "order": 10 },
    "event_after_tool_call": { "order": 10 },
    "event_text_delta": { "enabled": false }
  }
}
```

事件效果、允许动作、超时由宿主的 `EventSpec` 决定，清单不能覆盖。

### 7.2 装配

1. 读取插件声明；
2. 查事件目录，未知名称/非法效果直接拒绝；
3. 按“内置优先 → order → plugin id”排序；
4. 注册到对应 lane；
5. 产出装配收据（订阅了什么、被拒绝了什么）。

### 7.3 启停与收据

`plugin.setEnabled` 的收据必须包含：

- 移除/新增的事件订阅；
- 取消的 in-flight intercept（如有）；
- 因插件停用而丢弃的 notify 计数。

### 7.4 后续安全插件

- 安全/审批插件通过 `event_before_tool_call` / `event_before_approval` 等入口提供策略；
- 安全插件必须是受信层级，不能与普通第三方同权；
- 其 `Intercept` 失败方向由宿主事件表控制，最终拒绝权仍在引擎/宿主，不由插件自行“放行”；
- 审批 UI、路径沙箱、命令扫描的**执行语义**仍应留在核心，插件只提供策略，避免“谁能拦谁就能放行”的后门。

---

## 8. 迁移计划

### E0：契约与原型（已部分落地，2026-10-27）

- [x] `PluginEvent` / `EventOutcome` / `EventEffect` / `EventSpec` 类型加入
  `agent-base/src/domain/plugin_event.rs`；
- [x] `EventDispatcher` 端口加入 `agent-base/src/ports/events.rs`；
- [x] `ada.on("event_*")` 在注册期折回旧点位（`agent-node/src/hooks/event_names.rs`），
  引擎仍按旧点位调用；
- [ ] 3 个纵向切片（before_tool_call / after_tool_call / turn_finished）的运行时总线；
- [ ] 排序、超时、热重载与收据的端到端用例。

### E1：新增 dispatcher 端口

- [x] `agent-base` 增加 `PluginEvent` / `EventDispatcher` / `EventOutcome` / `EventSpec`；
- [x] `agent-node` 增加 `HookEventDispatcher` 迁移适配器（统一事件 → `HookRuntime`）；
- [x] `agent-node` 实现 `PluginEventRuntime`（有界 Critical/Decorate/Notify/Stream lane、异步 worker、generation）；
- [x] `ts-engine` 的 `ada.on` 已能接受 `event_*` 名称（宿主注册期折回旧点位）；
- [x] 引擎 **before / after 点位**由 dispatcher 产生；`EventSink` 仍是领域事件唯一出口；
- [x] `LivePluginReassembler` 在重装配后 `bump_generation`，旧排队事件作废。

### E2：逐点位迁移

- [x] `run_before_hook` / `run_after_hook` 统一走 `EventDispatcher`（适配器折回 `HookRunner`）；
- [x] after 多条 note/observe 通过 `dispatch_many` 汇总；
- [ ] 每个点位补测试：允许、拒绝、超时、替换后重新校验、旁注时序；
- 保留旧钩子名别名，直到插件生态迁移完成。

### E3：移除旧 API

- 插件文档与 `extension-api.d.ts` 移除钩子术语；
- `HookRunner` 可更名为 `EventDispatcher`，但**不能**降级成只有 `emit` 的 `EventSink`；
- 线协议 `evt.*` 与内部 `AgentEventBody` 暂不改名；若未来要改，走协议 v2 并提供别名。

### E4：安全插件

- 安全/审批插件按 §7.4 接入；
- 恢复 `EventFailure::Deny`、路径/命令策略与审批通道；
- 这一步独立于本次事件合并，不在 E1–E3 混做。

---

## 9. 验收与门禁

建议新增：

| 门 | 断言 |
|---|---|
| `verify-event-catalog` | 事件名唯一；`event_before_*` 只能是 Intercept/Decorate；每个事件有合法效果、超时、失败方向 |
| `verify-event-bus` | 每运行时实例；无全局单例；`EventSink` 仍是领域事件唯一出口 |
| `verify-event-aliases` | 旧钩子名只能映射到已登记事件；未知别名在装配期诊断 |
| `verify-event-receipt` | 插件启停收据与实际订阅增删一致 |

测试至少覆盖：

- before 拦截的允许/拒绝/替换/超时；
- after 旁注在最终工具结果前生效（或明确降级为 Notify）；
- notify 队列满时丢弃并留诊断；
- stream 合并/丢弃不阻塞；
- 热重载期间旧 generation 不再收到事件；
- 旧钩子名兼容；
- TS 引擎不依赖 `agent-base`，`agent-base` 不依赖 TS/插件发现。

---

## 10. 风险与开放问题

| 风险 | 处理 |
|---|---|
| `before` 被做成纯通知，后续无法做安全插件 | 本文强制 Intercept 为异步屏障；类型与超时保留 |
| `after` 旁注时序不清 | 明确 `event_after_tool_call` 在最终结果前执行；做不到就降级 Notify |
| 高频增量拖慢决策 | Stream 独立 lane，默认不订阅，可合并/丢弃 |
| 全局单例/跨工作区污染 | 每运行时实例 + generation，禁止 OnceLock |
| 插件事件泄露审批/命令原文 | 事件 payload 过滤；敏感事件标 `TrustedOnly` |
| 旧钩子名一次性消失导致插件全坏 | 别名映射 + 迁移窗口 |
| 核心事件与插件事件双写 | `EventSink` 单一出口；插件事件只做投影和拦截 |
| 安全恢复时重新设计总线 | `EventOutcome` / `failure` 已预留；安全插件走 Intercept |

开放问题（实现前必须定）：

1. `event_after_tool_call` 的 note 是否必须保留？如果保留，引擎必须等待该事件；
2. `event_before_persist` / `event_after_checkpoint` 等不对称点位的最终名称；
3. `event_before_provider_request` / `event_before_provider_headers` 是否合并为一个可替换载荷；
4. 旧钩子别名的保留周期；
5. 第三方插件能否订阅 `TrustedOnly` 事件，还是只能观察投影后的公开字段；
6. `event_text_delta` 是否需要按插件配置合并后再投递。

---

## 11. 审批与安全相关代码移除

> 决策：**物理删除**，不再维持 `SECURITY_ENFORCED` 这类行为开关。
> 本节是代码移除的执行边界；与 [PLAN-pluginization.md](PLAN-pluginization.md) §4.6
> 的 P3 清单对齐，且取代“改常量恢复”的过渡方案。
>
> 这条线**不与事件合并同批做**：事件合并先保证插件 API 稳定，审批/安全删除单独分支、
> 单独 PR、每步一个 commit。

### 11.0 执行结果（2026-10-27 完成）

先移除了四个点位（`beforeTodoUpdate` / `afterTodoUpdate` / `beforeApproval` / `afterApproval`），
随后按 §11.1–§11.7 完成了**物理删除**：

- `agent-base`：删除 `ports/approval.rs`、`ApprovalGate`、`ApprovalOutcome` / `ApprovalRequest` /
  `AnsweredBy` / `FailDirection`、`ApprovalRequested` 事件、`ApprovalPolicy` 与
  `ToolDescriptor.approval`、审批诊断族与点位、引擎审批等待与 `apply_fail_direction`。
  `Scope::resolve_path` 改为只解析、返回 `PathBuf`，不再做边界拒绝。
- `agent-adapter`：删除 `approval/` 与 `ApprovalAwareScope`；`WorkspaceScope` 换成 `PathResolver`。
- `agent-node`：删除 `approval/`（闸门 / waiter 表 / 策略 shim）；`QuestionManager` 迁到
  `agent-node/src/questions.rs`（提问是用户交互，不是审批）；`ReadonlyEnforcingGate` 删除，
  只读子智能体仍靠装配期按 `Access::ReadOnly` 裁切工具。
- `agent-rpc`：删除 `approval.decide`、`config.setApproval`、审批 waiter 表注入、审批卡投影与
  `set_tool_waiting_approval`；`NodeConfigSource` 只保留 provider / contextWindow。
- `agent-web` / `plugins`：删除审批档位选择器、审批托盘、审批浮窗、`thread.approval` 插槽、
  `/approval/*` 与 `/approvals/*` 路由、`plugins/approval`；历史 `waiting_approval` 卡片按已结束记录渲染。
- `agent-toolkit`：删除 `security.rs`、`check_workspace_sandbox`、`PathGate`、`sandbox.rs` 的边界判定、
  `cmd_tools` / `background` 的 cwd 边界校验；路径只解析。
- `ts-engine`：删除 `check_workspace_sandbox` 与“沙箱拦截”错误；`@ada-needs` 只保留
  **模块注入清单**语义（声明什么就注入什么，不再作为权限门）。
- 协议/门禁：`spec/proto`、`client-ts`、`methods.rs` 删除 `approval.decide` / `config.setApproval` /
  `ApprovalMode`；`verify-wiring` 删除 `FailDirection` 必须被消费的检查。
- 历史数据：`~/.a-da/config.json` 里的 `approval` 字段不再读取；旧会话里的 `waiting_approval`
  不再有操作按钮，按普通历史记录展示。

### 11.1 移除范围（已全部执行）

| 层 | 删除内容 |
|---|---|
| `agent-base` | `ports/approval.rs`、`ApprovalGate` 端口与 `AgentRuntime` 的 approval 字段/构造参数；`ApprovalRequested` 事件；`ApprovalMode`；`ToolDescriptor.approval` 与 `ApprovalPolicy`；`ApprovalOutcome` / `ApprovalRequest` 等审批专用类型；引擎里的 `needs_approval_for` / 审批等待与 `ApprovalRequested` 发射点 |
| `agent-adapter` | `approval/` 整个策略模块（档位、危险命令扫描、免问白名单）；`ApprovalAwareScope`；`approval_mode` 配置读取 |
| `agent-node` | `approval/`（`ApprovalManager`、`HostApprovalGate`、waiter 表）；`beforeApproval` / `afterApproval` 事件点位；审批相关诊断 |
| `agent-rpc` | `approval.decide` 分发；快照/配置里的 `approval` 字段；审批卡投影与等待状态 |
| `agent-web` | 审批档位选择器、审批浮窗、审批托盘、`thread.approval` 插槽、审批路由 `/approvals/*`、`plugins/approval` |
| `agent-toolkit` | `security.rs` 总开关；`sandbox.rs` 的路径判定；`PathGate` 的 Scoped/Workspace 安全分支；`cmd_tools` / `background` 的 cwd 校验；危险命令扫描 |
| `ts-engine` | `security::deferred` 对应的能力授权分支；`ScriptNeeds` 的“允许/拒绝”语义。**注意**：`Needs` 还承担原生模块的注入清单，是否连注入开关一起删见 §11.4 |
| 协议/文档 | `ApprovalMode`、`approval.decide`、审批事件、审批 UI 截图与示例；`verify-wiring` 中审批相关检查 |

### 11.2 明确保留（不是审批/安全）

- `Access`（`ReadOnly` / `Mutates` / `Executes` / `External`）：plan 模式、只读子智能体、
  工具过滤仍需要它，不是审批闸门。
- `Scope` 的**路径解析能力**：删除 `ApprovalAwareScope` 与沙箱判定后，保留一个
  无边界检查的 `PathResolver`（工作区相对路径解析 + `cwd` 解析），否则所有工具都要
  自己拼路径。它不再负责“允许/拒绝”。
- 事件 `seq` / 顺序 / `EventSink` 单一出口 / 取消贯穿 / 结构化回执：这些是契约，不是安全。
- `EventOutcome` / `EventFailure` / `event_before_*`：保留给后续插件式策略；
  删掉的是核心审批执行，不是“以后不能再用事件做管控”。

### 11.3 移除顺序

1. **外部面先删**：`plugins/approval`、`thread.approval` 插槽、SSR 审批托盘/浮窗、
   `/approvals/*`、`approval.decide`、`ApprovalMode` UI 与配置文件字段。
   先让界面和协议不再依赖审批，再动引擎。
2. **引擎面**：移除 `ApprovalGate` 端口、`ApprovalRequested` 事件、`ToolDescriptor.approval`
   与 `needs_approval_for` 分支；`run_turn` 不再等待审批。
3. **路径面**：`ApprovalAwareScope` → `PathResolver`；`PathGate` 去掉安全分支；
   `cmd_tools` / `background` 不再做 cwd 边界判断；`sandbox.rs` 只保留路径规范化。
4. **能力面**：删除 `security.rs`；`to_engine_needs` / `needs_json` 不再按声明授权；
   若确认“所有能力始终注入”，再删除 `@ada-needs` 与 `ScriptNeeds`（见 §11.4）。
5. **收尾**：删除审批测试、e2e（`verify-ssr-abort.mjs` 改为只验证 abort），
   更新协议 `spec/proto`、`client-ts`、文档与 `verify-wiring`。

### 11.4 `@ada-needs` 的处理

`@ada-needs` 当前承担两件事：

- **注入清单**：决定 `fs` / `net` / `sqlite` / `child_process` 是否在沙箱里存在；
- **授权语义**：声明没写的能力不授予。

本次删除的是后者。建议分两步：

1. 先让 `to_engine_needs` / `needs_json` 不再因“未声明”拒绝，保留 `Needs` 作为
   **模块注入清单**（插件仍可声明自己需要什么，但系统不把它当权限门）；
2. 如果确认不再需要按插件裁剪 API，再把 `Needs` / `ScriptNeeds` / `@ada-needs` 一并删除，
   TS 引擎恒定注入全部能力。

如果第 2 步没做，文档里不能声称“`@ada-needs` 已完全移除”，只能写“授权语义已移除”。

**执行结果**：采用第 1 步。`to_engine_needs` / `needs_json` 仍按声明返回注入清单，
但不再有“未声明就拒绝”的授权分支；`Needs` / `ScriptNeeds` / `@ada-needs` 保留。

### 11.5 兼容与迁移

- **协议**：`approval.decide` 从 `spec/proto` 与 `ALL_METHODS` 删除，属于破坏性协议变更；
  旧客户端调用会得到方法不存在。必须与产品版本号/发布说明同步。
- **配置**：`~/.a-da/config.json` 的 `approval` 字段读取方删除；旧字段保留不读，避免
  破坏用户配置；下一次保存时自然淘汰。
- **会话数据**：历史 `waiting_approval` 卡片不再有审批动作。UI 应把它们当普通工具卡/
  历史记录渲染，不能留下“点不动的批准按钮”。
- **插件**：完整删除时移除 `plugins/approval` 与 `thread.approval` 插槽；
  本次已先把它改成**纯前端**插件（只读 `props.data.approvals`），后端 hooks 已删除。
  第三方若仍声明已删除的 hook 点位，加载期会收到 `unknown_point`/`point_not_implemented`。
- **测试**：删除审批相关单测；`verify-ssr-abort.mjs` 拆成 abort-only 或删除；
  全量测试不再构造 `RecordingApprovalGate`。

### 11.6 验收

- `rg -n "Approval(Gate|Mode|Policy|Requested|Outcome)|approval\\.decide|SECURITY_ENFORCED|check_workspace_sandbox|PathGate"` 在**生产代码**中无命中；
  测试/文档中的历史说明除外。
- `cargo build --workspace`、`cargo test --workspace -- --test-threads=1`、
  `cargo xtask verify --fast`、`bun run typecheck` 全过。
- plan 模式 / 只读子智能体仍按 `Access::ReadOnly` 过滤工具（这是功能，不是审批）。
- SSR 不再渲染审批托盘/浮窗；发送、中止、提问、工具卡正常。
- 没有 `approval.decide` 的协议残留；`spec/proto` 与 `client-ts` 同步。

### 11.7 风险与回滚

- 这是**跨 9 个 crate + 协议 + UI + 测试**的破坏性改动，必须单独分支、单独 PR，
  且不要与事件合并、插件化迁移同时进行。
- 保留 `EventOutcome` / `event_before_*`，后续若要恢复“插件式审批/策略”，
  走受信插件 + Intercept，不复活旧的 `ApprovalGate` 代码。
- 回滚方式只能是 revert 本系列 commit；不要依赖已经删除的 `SECURITY_ENFORCED` 常量。
- 删除后仍要保证 `Access` 只读分类、路径解析、取消贯穿、事件顺序和结构化回执不被破坏。

---

## 12. 结论

**可以按“基座 + TS 引擎 + 插件系统 + 统一 event_* 事件”推进，但必须满足：**

1. `event_before_*` 是异步执行、引擎等待结果的 Intercept，不是 fire-and-forget；
2. `event_after_*` 若保留 note，必须是 Decorate，时序在最终结果之前；
3. 高频增量走独立 Stream lane，不与决策 lane 混用；
4. `EventSink` 仍是领域事件唯一出口，插件总线是消费者/拦截层；
5. `agent-base` 不依赖 TS 引擎与插件发现；`ts-engine` 不依赖 agent 领域模型；
6. 安全暂不执行，但 `EventOutcome`、超时、失败方向保留，后续由受信插件接入。

满足这六条，合并事件与钩子会真正简化插件模型；否则只是把“钩子”改名为“事件”，却丢掉后续做策略管控的能力。
