# 线协议总览

a_da 的客户端与宿主之间只有**一套状态/事件线协议**：JSON-RPC 2.0 over WebSocket。
另有一层 HTTP **unary API**（`/api/*`，见 [http-api.md](http-api.md)）：它只承载
一问一答的命令/查询，**不承载事件**；流式输出、状态增量与实时交互仍然走 `/rpc`。
本目录是它们的人类可读说明；机器可读的登记表在 [`spec/proto/`](../../spec/proto/README.md)。

> 分工：`spec/proto/*.json` 是**真源**（方法名、`kind`、`since`、错误码）；
> 本目录解释"怎么用"。**两处不一致时以 `spec/proto/` 为准**——且这种不一致会被
> `cargo xtask verify-wiring` / `cargo xtask compat` / `cargo test -p agent-proto`
> 与 `cargo xtask verify-docs`（断言本文覆盖全部事件主题）抓住。

## 1. 传输与握手

| 项 | 值 |
|---|---|
| 传输 | WebSocket（`tokio-tungstenite`） |
| 承载 | JSON-RPC 2.0（请求/响应/通知三种帧） |
| 鉴权 | **握手期 query 参数**：`ws://<host>:<port>/?token=<token>` |
| 未通过鉴权 | 握手直接返回 **HTTP 401 Unauthorized**，不建立连接 |
| 帧结构 | `{"jsonrpc":"2.0","id":<n>,"method":"...","params":{...}}` |
| 通知 | 无 `id`（服务端推送事件用 `method: "evt.*"`） |
| 协议版本 | `PROTOCOL_VERSION = "1.0"`（`crates/agent-proto/src/methods.rs`） |
| HTTP unary API | `/api/*`（Bearer token；与 `/rpc` 同端口；不含事件） |

实现位置：`crates/agent-rpc/src/server/ws.rs`（`bind` / `bind_with_engine`，握手回调校验 token）。
网关侧另有一条**对外接入面**：`crates/agent-gateway/src/auth.rs`（token / 作用域 / Origin / 配对码）。

> **注意**：`?token=` 是**握手期**校验，不是每条消息校验。因此 token 一旦泄漏，
> 攻击者可直接建连；宿主侧只在启动时打印/写入一次 token，不随消息轮换。
> 网关还有一条硬约束：**非回环 `--host` 必须配 `--token` 且显式 `--allow-plaintext`**，
> 两条都拒绝启动而不是警告（明文 `ws://` 上的 token 可嗅探）。

## 2. 三层协议

| 层 | 内容 | 登记处 |
|---|---|---|
| **L0** | 核心通信与调度：`session.*`、`thread.*`、`approval.*`、`question.*`、`queue.*`、`ui.*`、`evt.*` | `spec/proto/base.json` |
| **L1** | 系统能力位与标准错误码 | `spec/proto/base.json`（`capabilities` / `errorCodes`） |
| **L2** | 产品扩展：工作区、配置、供应商、插件、技能、提示词、子智能体、Git/FS、检查点、调试 | `spec/proto/<product>.ext.json` |

**L2 不得重新定义 L0/L1 的方法**——`cargo xtask compat` 会断言这一点，
并要求同名方法的 `kind` 一致。

**当前规模**：方法 **77** 个（base 23 + `ada-coding.ext.json` 54，`ALL_METHODS` 有断言守着）。

## 3. 帧类型

### 3.1 请求 / 响应

```json
// → 客户端
{ "jsonrpc": "2.0", "id": 7, "method": "thread.send", "params": { "threadId": "t1", "prompt": "..." } }

// ← 宿主
{ "jsonrpc": "2.0", "id": 7, "result": { "threadId": "t1" } }

// ← 宿主（错误）
{ "jsonrpc": "2.0", "id": 7, "error": { "code": -32601, "message": "Method not found (code: -32601)" } }
```

### 3.2 通知（服务端 → 客户端）

```json
{ "jsonrpc": "2.0", "method": "evt.state.snapshot", "params": { "seq": 42, "...": "..." } }
```

`ALL_EVENTS` 登记了 **3 个事件主题**：`evt.state.snapshot`、`evt.message.delta`、`evt.card.updated`。
另外 `evt.state.patch` 是**增量帧**主题（能力协商后启用，见
[state-patch.md](state-patch.md)）。`seq` 单调递增；界面按 `seq` 去重与断线重连对账。

| 主题 | 语义 |
|---|---|
| `evt.state.snapshot` | 全量快照：首帧、自愈、重连 |
| `evt.state.patch` | 增量帧：只推指纹变了的 item + `meta` 小字段 + `removed` |
| `evt.message.delta` | 流式增量文本 |
| `evt.card.updated` | 单卡片更新 |

> **卡片顺序由 `at` 表达，且必须两处都守**：增量帧的 `upserted` 按 `id` 排序（帧要可复现），
> 界面只能按 `at` 定位新卡片，所以**一个会话内卡片的 `at` 必须严格递增**——
> 概要见 [state-patch.md](state-patch.md)，实机验证用 `bun tools/verify-card-order-e2e.mjs`。

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
> 后者由 `verify-wiring` 静态守着（已清零），因此运行期几乎只可能是第一种。

## 5. 审批与提问（**唯一需要"人"的通道**）

审批的**策略**归插件 `approval-guard`、**执行**归核心（`AGENTS.md` §14），
线上表现为两条方法 + 一条状态：

1. 引擎判定"要问" → 发 `ApprovalRequested`（**带 `args`**，审批卡要能回答"我在批准什么"）
   → 工具卡片进入 `status: "waiting_approval"`；
2. 界面在**输入框上方的浮动审批窗口**渲染批准/拒绝按钮 → 调 `approval.decide`
   （`{ callId, approved }`）→ 唤醒等待中的审批闸门；
3. 拒绝时**不抛错**，而是把拒绝原因作为**工具结果**回给模型（模型知道被拒了，不会以为成功）。

同一形态用于 `ask_user` / `question.answer`（模型主动提问）。

> ⚠️ **超时**：审批闸门默认 300s。超时按 `ApprovalGate::direction()` 处理——
> 默认 `Closed`（拒绝），并记 `by: Timeout` 便于事后分辨来路。
> `Scope`（路径沙箱）的失败方向**刻意不可配置**：路径越界是确定的判定，
> 给它一个 `Open` 开关等于"拿不准时允许逃出沙箱"。

## 6. 客户端用法（TypeScript）

常量与 DTO 在 `crates/agent-proto/client-ts/`（**手写**，见 `spec/proto/README.md` 的校验表）：

```ts
import { THREAD_SEND, CONFIG_GET } from './methods'

const res = await client.request(THREAD_SEND, { threadId: 't1', prompt: '你好' })
client.on('evt.state.snapshot', (params) => { /* seq 对账 */ })
```

`desktop-ui/src/client/ws-client.ts` 是生产客户端实现（**唯一协议客户端**），
可直接参考它的请求封装、事件订阅、增量帧合并与重连逻辑
（退避策略在 `desktop-ui/src/client/reconnect-policy.ts`，`verify-wiring` 有结构断言）。

## 7. 相关文档

| 文档 | 内容 |
|---|---|
| [http-api.md](http-api.md) | HTTP unary API：鉴权、端点、ETag、错误映射与 Phase 3 缺口 |
| [`spec/proto/README.md`](../../spec/proto/README.md) | 登记表真源、命名规则、四处同步、三对副本校验 |
| [state-patch.md](state-patch.md) | 增量帧与卡片顺序契约 |
| [../architecture/kernel-design.md](../architecture/kernel-design.md) §6 | 协议单源与"不做代码生成"的决策记录 |
| [../architecture/kernel-design.md](../architecture/kernel-design.md) §7 | 运行时装配、宿主与传输形态 |
| [../architecture/conventions.md](../architecture/conventions.md) §16 | 加/删协议方法要同步三处（有守门测试） |
| [../architecture/conventions.md](../architecture/conventions.md) §18 | 工具结果是**结构化回执**（界面耗时与状态徽章依赖它） |
