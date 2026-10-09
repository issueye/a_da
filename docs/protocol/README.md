# 线协议总览

a_da 的客户端与宿主之间只有**一套**线协议：JSON-RPC 2.0 over WebSocket。
本目录是它的**人类可读说明**；机器可读的登记表在 [`spec/proto/`](../../spec/proto/README.md)。

> 分工：`spec/proto/*.json` 是**真源**（方法名、`kind`、`since`、错误码）；
> 本目录解释"怎么用"。**两处不一致时以 `spec/proto/` 为准**——且这种不一致会被
> `cargo xtask verify-wiring` / `cargo xtask compat` / `cargo test -p agent-proto` 抓住。

## 1. 传输与握手

| 项 | 值 |
|---|---|
| 传输 | WebSocket（`tokio-tungstenite`） |
| 承载 | JSON-RPC 2.0（请求/响应/通知三种帧） |
| 鉴权 | **握手期 query 参数**：`ws://<host>:<port>/?token=<token>` |
| 未通过鉴权 | 握手直接返回 **HTTP 401 Unauthorized**，不建立连接 |
| 帧结构 | `{"jsonrpc":"2.0","id":<n>,"method":"...","params":{...}}` |
| 通知 | 无 `id`（服务端推送事件用 `method: "evt.*"`） |
| 协议版本 | `PROTOCOL_VERSION = "1.0"`（`agent-proto/src/methods.rs`） |

实现位置：`crates/agent-core/src/server/ws.rs`（`bind` / `bind_with_engine`，握手回调校验 token）。

> **注意**：`?token=` 是**握手期**校验，不是每条消息校验。因此 token 一旦泄漏，
> 攻击者可直接建连；宿主侧只在启动时打印/写入一次 token，不随消息轮换。

## 2. 三层协议

| 层 | 内容 | 登记处 |
|---|---|---|
| **L0** | 核心通信与调度：`thread.*`、`approval.*`、`question.*`、`initialize`、`evt.*` | `spec/proto/base.json` |
| **L1** | 系统能力位与标准错误码 | `spec/proto/base.json`（`capabilities` / `errorCodes`） |
| **L2** | 产品扩展：工作区、配置、供应商、插件、技能、子智能体、Git/FS、检查点 | `spec/proto/<product>.ext.json` |

**L2 不得重新定义 L0/L1 的方法**——`cargo xtask compat` 会断言这一点，
并要求同名方法的 `kind` 一致。

## 3. 帧类型

### 3.1 请求 / 响应

```json
// → 客户端
{ "jsonrpc": "2.0", "id": 7, "method": "thread.start", "params": { "threadId": "t1", "prompt": "..." } }

// ← 宿主
{ "jsonrpc": "2.0", "id": 7, "result": { "threadId": "t1" } }

// ← 宿主（错误）
{ "jsonrpc": "2.0", "id": 7, "error": { "code": -32601, "message": "Method not found (code: -32601)" } }
```

### 3.2 通知（服务端 → 客户端）

```json
{ "jsonrpc": "2.0", "method": "evt.state.snapshot", "params": { "seq": 42, "...": "..." } }
```

事件主题（**与方法名分开命名空间**）：`evt.state.snapshot`、`evt.message.delta`、`evt.card.updated`。
`seq` 单调递增；界面按 `seq` 去重与断线重连对账。

## 4. 错误码

### 4.1 JSON-RPC 标准段

| 码 | 名称 | 含义 |
|---|---|---|
| `-32700` | ParseError | 帧不是合法 JSON |
| `-32600` | InvalidRequest | 结构不合法（缺 `method` 等） |
| `-32601` | MethodNotFound | 方法未注册（**或未实现**——见下） |
| `-32602` | InvalidParams | 参数校验失败 |
| `-32603` | InternalError | 宿主内部错误 |

### 4.2 应用段（`-32000..-32099`）

| 码 | 名称 | 含义 |
|---|---|---|
| `-32000` | ProtocolVersionMismatch | 客户端与宿主协议版本不一致 |
| `-32001` | Unauthorized | 未授权 |
| `-32002` | NotFound | 目标不存在 |
| `-32003` | NotReady | 宿主未就绪 |
| `-32004` | Denied | 被策略拒绝（**审批拒绝走这里**，见 §5） |
| `-32005` | Timeout | 超时 |
| `-32006` | AlreadyAnswered | 该问题/审批已被回答 |
| `-32007` | WorkspaceDenied | 工作区路径被沙箱拒绝 |
| `-32008` | TooLarge | 负载过大 |
| `-32009` | Cancelled | 被取消 |
| `-32010` | NeedResync | 需要重新同步（事件断档） |
| `-32011` | Busy | 忙 |
| `-32012` | ConfigInvalid | 配置不合法 |

实现位置：`crates/agent-proto/src/errors.rs`。

> **`MethodNotFound` 的两种来路要分清**：一是客户端拼错了方法名；
> 二是方法**在 `ALL_METHODS` 里但没有 dispatch 臂**（孤儿方法）。
> 后者曾被 `verify-wiring` 当成静态缺陷来防（W5-T2 已清零），
> 因此运行期几乎只可能是第一种。

## 5. 审批与提问（**唯一需要"人"的通道**）

审批的**策略**归插件 `approval-guard`、**执行**归核心 `askUser`（AGENTS.md §14），
线上表现为两条方法 + 一条状态：

1. 宿主执行到需要审批的工具 → 把该工具调用置为 `status: "waiting_approval"`
   （`details.approval` 带工具名与参数），界面据此渲染批准/拒绝按钮；
2. 界面调 `approval.decide`（`{ callId, approved }`）→ 宿主唤醒等待中的审批闸门；
3. 拒绝时**不抛错**，而是把拒绝原因作为**工具结果**回给模型（AGENTS.md §14 的口径）。

同一形态用于 `ask_user` / `question.answer`（模型主动提问）。

> ⚠️ **超时**：审批闸门默认 300s（`DEFAULT_APPROVAL_TIMEOUT`）。超时按
> `FailDirection` 处理——默认 `Closed`（拒绝），且记 `by: Timeout` 以便事后分辨来路。

## 6. 客户端用法（TypeScript）

常量与 DTO 在 `crates/agent-proto/client-ts/`（**手写**，见 `spec/proto/README.md` 的校验表）：

```ts
import { THREAD_START, CONFIG_GET } from './methods'

const res = await client.request(THREAD_START, { threadId: 't1', prompt: '你好' })
client.on('evt.state.snapshot', (params) => { /* seq 对账 */ })
```

`tauri-ui/src/client/ws-client.ts` 是生产客户端实现，可直接参考它的
请求封装、事件订阅与重连逻辑。

## 7. 相关文档

| 文档 | 内容 |
|---|---|
| [`spec/proto/README.md`](../../spec/proto/README.md) | 登记表真源、命名规则、四处同步、三对副本校验 |
| `docs/agent-base-design.md` §6 | 协议单源与产物决策（W5-T1 降级记录） |
| `docs/agent-base-design.md` §7 | 运行时装配、宿主与传输形态 |
| `AGENTS.md` §16 | 加/删协议方法要同步三处（有守门测试） |
| `AGENTS.md` §18 | 工具结果是**结构化回执**（界面耗时与状态徽章依赖它） |
