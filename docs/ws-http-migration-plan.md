# 开发计划：把可转的接口迁到 HTTP API

> 前置分析：[`docs/ws-http-split-analysis.md`](ws-http-split-analysis.md)（77 个协议方法 + 12 个网关方法逐条分类）>   
> 本计划只做"**值得做且不破坏现有形态**"的部分；`evt.*` 推送与审批/问答**不迁**。

---

## 一、目标与非目标

### 目标

1. **网关管理面**（`gateway.*` 12 个）提供 REST 接口，运维/CI/脚本无需实现 WS 客户端；
2. **节点只读查询**（20 个）提供 `GET /api/*`，可 `curl`、可缓存、可观测；
3. **节点写操作**（约 35 个）提供 `POST/PATCH/DELETE /api/*`，供外部集成；
4. **长任务**（4 个 + 网关 1 个）语义改为 `202 Accepted` + `Location`；
5. 顺带修掉两个现有问题：**只读请求也触发全量快照广播**、**HTTP 与 WS 两套鉴权各写一份**。

### 非目标（明确不做，避免反复讨论）

| 不做                                                                                     | 理由                                            |
| -------------------------------------------------------------------------------------- | --------------------------------------------- |
| 用 HTTP 取代 WebSocket                                                                    | 界面靠推送刷新，WS 必须留着（分析文档 §4.4）                    |
| 迁移 `evt.state.snapshot` / `evt.state.patch` / `evt.message.delta` / `evt.card.updated` | 服务端主动推送，没有 HTTP 语义                            |
| 迁移 `session.initialize`                                                                | 它做两件连接级的事：置握手门、开增量帧模式                         |
| 迁移 `approval.decide` / `question.answer`                                               | 回答的是内存里 `await` 的 future，需稳定 id + 双向配对，收益不抵风险 |
| 改桌面端 UI 的调用路径                                                                          | UI 已连着 WS，改它纯属自找回归；HTTP 只作为**第二条入口**          |
| 为 HTTP 另写一套业务 handler                                                                  | 见 D2，这是本计划最硬的红线                               |

### 验收的最终形态

```
桌面端 UI ──WS /rpc──┐
外部脚本 ──HTTP /api─┴──► Dispatcher::dispatch(method, params) ──► AgentStore / 引擎
运维脚本 ──HTTP /agents（网关直连，不经过节点）
```

---

## 二、设计决策（D1–D7，含备选与否决理由）

### D1 另起端口，不做单端口多协议

| 方案                    | 优点                          | 缺点                                        | 结论       |
| --------------------- | --------------------------- | ----------------------------------------- | -------- |
| A. 双端口（`--http-port`） | 零回归；WS 代码一行不动；两个端口可分别监听不同网卡 | 多一个端口要下发（ready 行里带上）                      | ✅ **采用** |
| B. 同端口 peek 首行分流      | 省一个端口                       | 要自己解析首行、处理 `Upgrade`、错误路径更绕；`ws.rs` 改动风险高 | ❌        |

`ws.rs:219` 现在是 `accept_hdr_async(stream, callback)`——裸 TCP 直接进 WS 握手，**没有 HTTP 层**。  
方案 A 只需在 ready 行里多一个 `httpPort` 字段（`products/ada-coding/src/main.rs:540`、  
`crates/agent-gateway/src/main.rs:125` 的 ready JSON）。

### D2 HTTP 只做"路由 → (method, params) → 调同一个 `Dispatcher::dispatch`"

**不为 HTTP 重写任何业务逻辑。** HTTP 层的全部职责：

```
解析 (verb, path) → 查路由表 → 合并参数 → dispatcher.dispatch(method, params) → 错误码映射 → 序列化
```

理由：`dispatch()` 里已有 39 处手写参数解包与一套落盘/广播语义，重写一遍必然与 WS 侧漂移  
（`docs/code-redundancy-audit.md` §3.7 就是这个问题的记录）。

### D3 路由表是**单一真源**，并由门禁机械校验

新增 `crates/agent-proto/src/routes.rs`：

```rust
pub struct Route {
    pub verb: Verb,            // Get / Post / Put / Patch / Delete
    pub path: &'static str,    // "/api/threads/{threadId}/items"
    pub method: &'static str,  // THREAD_ITEMS_PAGE
    pub kind: RouteKind,       // Read | Write | Accepted(长任务)
}

pub const ROUTES: &[Route] = &[ ... ];
```

**约定：路径变量名一律等于 RPC 参数名**（`/api/threads/{threadId}/items`，而不是 REST 风格的 `/threads/{id}`）。  
代价是 URL 略啰嗦，收益是**不需要任何参数改名映射表**——少一张表就少一处漂移。

新增门禁（`cargo xtask verify-wiring` 里加 `check_http_routes`）：

1. 每条 `route.method` ∈ `ALL_METHODS`；
2. `(verb, path)` 不重复；同一路径的 GET 与 DELETE 允许共存；
3. `kind == Read` 的方法集合 **≡** 新增的 `READ_ONLY_METHODS`（见 D6）——两处引用同一常量，不可能漂移；
4. 每条路由在 `dispatch` 里都有对应臂（复用现有 `check_dispatch_arms` 的解析）。

### D4 鉴权：`Authorization: Bearer`，复用 `AuthConfig`

- **节点**：`WsHostServer` 现在用 `?token=`（`ws.rs:199-206`）。HTTP 侧读 `Authorization: Bearer <token>`，    
  与 WS 用**同一个** `token` 字段（`WsHostServer::bind_with_engines` 已经持有它）；
- **网关**：直接复用 `auth::AuthConfig::authorize(token)`（`auth.rs:282`）与 `TokenScope::allows(workspace)`    
  （`auth.rs:122`）——多租户 token 限定工作区的语义在 HTTP 上照旧生效；
- **禁止** query 里传 token（会进日志、进 Referer）。

### D5 长任务统一 `202 Accepted`

| 现状                                               | HTTP                                                                                        |
| ------------------------------------------------ | ------------------------------------------------------------------------------------------- |
| `thread.send` 立即返回 `{accepted, queued, queueId}` | `202` + `Location: /api/threads/{id}` + body 原样返回                                           |
| `gateway.delegate` **等目标 agent 跑完**才返回           | `202` + `Location: /api/delegations/{id}`，结果走 `evt.*` 或 `GET`（M4 补 `GET /delegations/{id}`） |

进度与结果**只走事件通道**，HTTP 不做长轮询（避免两套"等结果"的实现）。

### D6 广播触发改为"按方法是否只读"

现状：`ws.rs:318-320` 每次成功 dispatch 都 `broadcast_snapshot()`——`fs.list`、`config.get` 也广播。  
改为：

```rust
// crates/agent-proto/src/routes.rs
pub const READ_ONLY_METHODS: &[&str] = &[ CONFIG_GET, PROVIDER_LIST, /* … 20 个 */ ];

// ws.rs
if is_init_method || !requires_broadcast(&req.method) {
    // 不广播
} else {
    self.broadcast_snapshot().await;
}
```

HTTP 写操作**必须**同样触发广播（否则界面不刷新）；只读不触发。  
**这是本计划唯一一处会改变现有运行行为的改动**，单独成一个任务并配回归测试（见 T9）。

### D7 依赖选型：`hyper` 1.x（已在锁文件里）

`Cargo.lock` 里已有 `hyper 1.11.1` / `http` / `http-body` / `http-body-util` / `bytes` / `httparse`  
（都是 `reqwest` / `tokio-tungstenite` 的传递依赖），所以：

```toml
hyper = { version = "1", features = ["http1", "server"] }
http-body-util = "0.1"
```

**零新增下载**，比引 `axum + tower` 轻得多；路由本来就只有一张静态表，手写 `match` 足够。  
（若 M3 之后路由超过 ~60 条再考虑 axum，届时替换只影响 `http.rs` 一个文件。）

---

## 三、接口契约

### 3.1 网关 REST（M1，12 条）

| Verb   | Path                          | RPC method                  | 备注                                   |
| ------ | ----------------------------- | --------------------------- | ------------------------------------ |
| GET    | `/agents`                     | `gateway.listAgents`        |                                      |
| GET    | `/agents/{agentId}`           | `gateway.status`            |                                      |
| PUT    | `/agents/{agentId}`           | `gateway.register`          | 幂等注册                                 |
| POST   | `/agents/{agentId}/attach`    | `gateway.attach`            |                                      |
| DELETE | `/agents/{agentId}`           | `gateway.detach`            |                                      |
| GET    | `/fs/roots`                   | `gateway.fs.roots`          |                                      |
| GET    | `/fs/list`                    | `gateway.fs.listDirectory`  | query: `path`, `showHidden`, `limit` |
| POST   | `/fs/mkdir`                   | `gateway.fs.makeDirectory`  |                                      |
| GET    | `/workspaces`                 | `gateway.workspaces.list`   |                                      |
| DELETE | `/workspaces`                 | `gateway.workspaces.remove` | query: `path`                        |
| POST   | `/delegations`                | `gateway.delegate`          | `202` + `Location`                   |
| DELETE | `/delegations/{delegationId}` | `gateway.cancelDelegation`  |                                      |

### 3.2 节点只读（M2，20 条，`GET`）

| Path                                 | RPC method                                      |
| ------------------------------------ | ----------------------------------------------- |
| `/api/config`                        | `config.get`                                    |
| `/api/config/presets`                | `config.presets`（网络 IO，慢）                       |
| `/api/providers`                     | `provider.list`                                 |
| `/api/providers/{providerId}/models` | `provider.fetchModels`（网络 IO，慢）                 |
| `/api/plugins`                       | `plugin.list`                                   |
| `/api/plugins/builtin`               | `plugin.builtinCatalog`                         |
| `/api/prompts`                       | `prompt.list`                                   |
| `/api/skills`                        | `skill.list`                                    |
| `/api/subagent-profiles`             | `subagentProfile.list`                          |
| `/api/workspaces`                    | `workspace.entries`                             |
| `/api/changes`                       | `change.list`                                   |
| `/api/changes/count`                 | `change.count`                                  |
| `/api/threads/{threadId}/items`      | `thread.items.page`（query: `offset`, `limit`）   |
| `/api/snapshot`                      | `session.snapshot` ⚠️ 见风险 R4                    |
| `/api/fs/roots`                      | `fs.roots`                                      |
| `/api/fs/list`                       | `fs.list`（query: `path`, `showHidden`, `limit`） |
| `/api/fs/read`                       | `fs.read_base64`（query: `path`）                 |
| `/api/debug/host`                    | `debug.hostInfo`                                |
| `/api/stats/prompt-chars`            | `stats.promptChars`                             |
| ~~`/api/plugins/{id}/validate`~~     | `plugin.validate` → 归 M3（要跑插件代码，不是纯读）           |

### 3.3 节点写（M3，按域分批）

| 域    | 路由                                                                                                                                                                                                                                                                | 对应方法                                                                                                              |
| ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| 配置   | `PUT /api/config/provider`、`/approval`、`/effort`、`/retry-count`                                                                                                                                                                                                   | `config.setProvider` / `setApproval` / `setEffort` / `setRetryCount`                                              |
| 供应商  | `PUT /api/providers/{providerId}`、`DELETE /api/providers/{providerId}`、`PUT /api/providers/active`                                                                                                                                                                | `provider.save` / `delete` / `setActive`                                                                          |
| 工作区  | `POST /api/workspaces`、`DELETE /api/workspaces`                                                                                                                                                                                                                   | `workspace.add` / `remove`                                                                                        |
| 线程   | `POST /api/threads`、`PATCH /api/threads/{threadId}`、`DELETE /api/threads/{threadId}`、`PUT /api/threads/{threadId}/mode`、`PUT /api/threads/{threadId}/workspace`、`POST /api/threads/{threadId}/abort`                                                              | `thread.create` / `update` / `delete` / `setMode` / `setWorkspace` / `abort`                                      |
| 队列   | `DELETE /api/queue`、`POST /api/queue/{queueId}/promote`、`DELETE /api/queue/{queueId}`                                                                                                                                                                             | `queue.clear` / `promote` / `remove`                                                                              |
| 变更回滚 | `POST /api/changes/{changeId}/revert`、`POST /api/changes/checkpoints/{checkpointId}/revert`、`POST /api/changes/files/revert`、`POST /api/changes/revert-all`                                                                                                       | `change.revertCard` / `revertCheckpoint` / `revertFile` / `revertAll`                                             |
| 插件   | `POST /api/plugins/{pluginId}/enabled`、`PUT /api/plugins/{pluginId}/config`、`PUT /api/plugins/{pluginId}/secret`、`PUT /api/plugins/{pluginId}/capabilities`、`DELETE /api/plugins/{pluginId}`、`POST /api/plugins/template`、`POST /api/plugins/{pluginId}/validate` | `plugin.setEnabled` / `config.set` / `secret.set` / `capabilities.set` / `delete` / `createTemplate` / `validate` |
| 提示词  | `POST /api/prompts`、`PUT /api/prompts/{promptId}`、`DELETE /api/prompts/{promptId}`、`PUT /api/prompts/{promptId}/enabled`                                                                                                                                          | `prompt.create` / `update` / `delete` / `setEnabled`                                                              |
| 技能   | `POST /api/skills`、`DELETE /api/skills/{skillId}`、`PUT /api/skills/{skillId}/enabled`                                                                                                                                                                             | `skill.create` / `delete` / `setEnabled`                                                                          |
| 子智能体 | `DELETE /api/subagent-profiles/{profileId}`、`PUT /api/subagent-profiles/{profileId}/enabled`                                                                                                                                                                      | `subagentProfile.delete` / `setEnabled`                                                                           |
| 调试   | `POST /api/debug/logs`（`debug.trace` 实为写日志）、`DELETE /api/debug/logs`                                                                                                                                                                                              | `debug.trace` / `debug.log.clear`                                                                                 |

> `ui.*` 五个**不入 HTTP**：它们是客户端视图状态，正确去处是 localStorage（分析文档 §7.2）。>   
> 若将来要多窗口同步，单独开一个轻量 `PUT /api/ui`，不要塞进快照。

### 3.4 长任务（M4，5 条，`202`）

`POST /api/threads/{threadId}/messages`（`thread.send`）、`POST /api/threads/{threadId}/messages:resend`  
（`thread.editAndResend`）、`POST /api/threads/{threadId}/retry`、`POST /api/threads/{threadId}/compact`、  
`POST /api/plugins/{pluginId}/reload`（`plugin.reload`，重装配可能数秒）。

响应：

```http
HTTP/1.1 202 Accepted
Location: /api/threads/{threadId}
Content-Type: application/json

{ "accepted": true, "queued": false, "threadId": "thread_xxx" }
```

### 3.5 错误码 → HTTP 状态（唯一映射表，`crates/agent-proto/src/errors.rs` 旁新增）

| JSON-RPC code | 常量                        | HTTP |
| ------------- | ------------------------- | ---- |
| -32700        | `ParseError`              | 400  |
| -32600        | `InvalidRequest`          | 400  |
| -32601        | `MethodNotFound`          | 404  |
| -32602        | `InvalidParams`           | 422  |
| -32603        | `InternalError`           | 500  |
| -32000        | `ProtocolVersionMismatch` | 400  |
| -32001        | `Unauthorized`            | 401  |
| -32002        | `NotFound`                | 404  |
| -32003        | `NotReady`                | 503  |
| -32004        | `Denied`                  | 403  |
| -32005        | `Timeout`                 | 504  |
| -32006        | `AlreadyAnswered`         | 409  |
| -32007        | `WorkspaceDenied`         | 403  |
| -32008        | `TooLarge`                | 413  |
| -32009        | `Cancelled`               | 409  |
| -32010        | `NeedResync`              | 409  |
| -32011        | `Busy`                    | 429  |
| -32012        | `ConfigInvalid`           | 422  |

响应体保持 JSON-RPC 的 error 形状（`{code, message, data}`），只是套进 HTTP 状态码——  
**不两套错误体**，客户端一套解析逻辑能同时吃 WS 与 HTTP。

---

## 四、代码结构

### 4.1 新增文件

| 文件                                       | 职责                                                                        | 规模估计          |
| ---------------------------------------- | ------------------------------------------------------------------------- | ------------- |
| `crates/agent-proto/src/routes.rs`       | `Route` / `RouteKind` / `ROUTES` / `READ_ONLY_METHODS` / `is_read_only()` | ~180 行（大部分是表） |
| `crates/agent-rpc/src/server/http.rs`    | `HttpServer::bind(...)`、连接循环、`hyper` service、路由匹配、参数合并、错误映射               | ~220 行        |
| `crates/agent-gateway/src/http.rs`       | 网关 REST 面（直连 `Gateway`，不走 `Dispatcher`）                                   | ~200 行        |
| `crates/agent-gateway/tests/http_e2e.rs` | 网关 REST 端到端（注册→列表→attach→detach→回收）                                       | ~150 行        |
| `crates/agent-rpc/tests/http_api_e2e.rs` | 节点 HTTP 端到端（`TempAppHome` 注入）                                             | ~200 行        |
| `docs/http-api.md`                       | 对外接口文档（路径/参数/示例/错误码）                                                      | 文档            |


### 4.2 改动点

| 文件                                     | 改动                                                  |
| -------------------------------------- | --------------------------------------------------- |
| `crates/agent-proto/src/lib.rs`        | `pub mod routes;`                                   |
| `crates/agent-proto/src/errors.rs`     | 新增 `pub fn http_status(code: i32) -> u16`           |
| `crates/agent-rpc/src/server/mod.rs`   | `pub mod http;` 与 re-export                         |
| `crates/agent-rpc/src/server/ws.rs`    | 广播触发改为 `if !is_read_only(&req.method)`（D6）          |
| `products/ada-coding/src/main.rs`      | 新增 `--http-port`（默认 0 = 关闭）；ready JSON 加 `httpPort` |
| `crates/agent-gateway/src/main.rs`     | 同上（网关默认开启，管理面本来就是给外部用的）                             |
| `tools/xtask/src/gates.rs`             | 新增 `check_http_routes`（D3 的四条断言）                    |
| `Cargo.toml` / 两个 crate 的 `Cargo.toml` | `hyper` + `http-body-util`                          |
| `AGENTS.md`                            | 补一条"HTTP 面必须复用 `Dispatcher::dispatch`"的红线           |

### 4.3 参数合并规则（唯一规则，不做特例）

```
params = body(JSON object, 仅 POST/PUT/PATCH) ∪ path_vars ∪ query_string
冲突时：path > body > query（并记一条 warn，说明同一键被覆盖）
GET 不接受 body；body 非空对象而 verb 是 GET → 400
```

字符串参数（如 `showHidden`、`limit`）按 `dispatch` 现有期望做**最小**类型推断：能解析成数字就是数字，  
`"true"/"false"` 转 bool，其余保持字符串。**不做 schema**——schema 是 M3 之后的事（见 §8 后续）。

---

### 4.4 生命周期：HTTP 不能自己 `bind`，必须和 WS 共用同一批对象

`WsHostServer::bind_with_engines`（`ws.rs:52`）在内部构造 `dispatcher` 与 `broadcaster`，  
`HttpServer` 必须拿到**同一份** `Arc<Dispatcher>` 和 `Arc<StateBroadcaster>`：

- 同一个 `Dispatcher` → 同一个 `AgentStore`（否则 HTTP 改了配置，WS 的内存态看不见）；
- 同一个 `StateBroadcaster` → HTTP 写操作能触发 `broadcast_immediate()`（否则界面不刷新）。

因此实现方式固定为：`bind_with_engines(bind_port, token, store, injection, pm_injection)`  
增加一个 `http_port: Option<u16>`，在函数内部一并创建两个 server，返回值加 `http_port`。  
**不允许**在 `main.rs` 里另起一个 `HttpServer::bind` 并自己 new 一个 dispatcher——那样会造出  
第二份 `AgentStore`，症状是"HTTP 改完，界面不动"。

## 五、任务分解

工作量：S（半天内）/ M（1 天）/ L（2 天+）。顺序即依赖顺序。

### M0 地基（必须先做完，后续全部并行度都靠它）

| #  | 任务                                                                                              | 交付                                       | 验收                                                | 量 |
| -- | ----------------------------------------------------------------------------------------------- | ---------------------------------------- | ------------------------------------------------- | - |
| T1 | `routes.rs`：`Route`/`RouteKind`/`ROUTES`（先只填 M1 的 12 条）+ `READ_ONLY_METHODS` + `is_read_only()` | 表 + 单测                                   | 单测断言路径不重复、method 都在 `ALL_METHODS`                 | S |
| T2 | `errors.rs::http_status()` + 单测覆盖全部 18 个码                                                       | 映射函数                                     | 每个变体都有确定状态码，未知名回 500                              | S |
| T3 | `agent-rpc/src/server/http.rs::HttpServer`：bind / accept / hyper service / 路由匹配 / 参数合并 / 错误映射   | 能在 `--http-port` 上响应 `GET /api/fs/roots` | e2e：起服务 → curl → 200；错误 token → 401；未知路径 → 404    | M |
| T4 | 鉴权：`Authorization: Bearer`；节点复用 `WsHostServer.token`，网关复用 `AuthConfig::authorize`               | 鉴权中间层                                    | 无 token 401；错 token 401；网关多租户 token 只能看自己工作区（403） | S |
| T5 | 门禁 `check_http_routes`（D3 四条）                                                                   | `cargo xtask verify-wiring` 新检查          | 故意删一条 dispatch 臂 → 门禁变红                           | S |

### M1 网关 REST（收益最高，且不碰节点状态）

| #  | 任务                                                                                                       | 交付        | 验收                                         | 量 |
| -- | -------------------------------------------------------------------------------------------------------- | --------- | ------------------------------------------ | - |
| T6 | `agent-gateway/src/http.rs`：12 条路由直连 `Gateway`（`registry` / `fs_service` / `workspaces` / `delegations`） | 网关 HTTP 面 | `curl /agents` 返回与 `gateway.listAgents` 同形 | M |
| T7 | `gateway.delegate` 异步化：立刻返回 202 + `delegationId`，结果走原有 `evt.*`；补 `GET /delegations/{id}`                 | 202 + 查询  | e2e：发起派活 → 202 → 事件里拿到结果                   | M |
| T8 | `http_e2e.rs`：注册 → 列表 → attach → detach → **断连回收**（顺带覆盖 §3.5 修过的 `RegisteredAgentGuard`）                 | 集成测试      | `cargo test -p agent-gateway` 全绿           | M |

### M2 节点只读

| #   | 任务                                                                                          | 交付              | 验收                                                     | 量 |
| --- | ------------------------------------------------------------------------------------------- | --------------- | ------------------------------------------------------ | - |
| T9  | **D6 广播改造** + 回归测试：只读方法不再广播                                                                 | `ws.rs` 改动 + 测试 | 现有全量测试全绿；新测试断言 `fs.list` 不触发 `evt.state.snapshot`      | S |
| T10 | 20 条只读路由接进 `ROUTES` 并与 `READ_ONLY_METHODS` 对齐                                               | 路由表扩充           | 门禁绿；`curl /api/config`、`/api/skills`、`/api/fs/list` 正常 | M |
| T11 | 去掉只读接口的服务端写（`skill.list`/`plugin.list` 的扫描回写、`session.snapshot` 的 `sync_threads_from_disk`） | 只读真的只读          | 两次 GET 结果一致；不再产生写锁                                     | M |
| T12 | 慢接口加超时与提示：`config.presets` / `provider.fetchModels`                                         | 30s 超时 + `504`  | 断网时 30s 内返回 504，不挂住连接                                  | S |

### M3 节点写（按域分批，每域一个 PR）

| #   | 任务                                    | 交付       | 量 |
| --- | ------------------------------------- | -------- | - |
| T13 | 配置 + 供应商（4 + 3 条）                     | 路由 + e2e | M |
| T14 | 线程 + 队列（6 + 3 条）                      | 路由 + e2e | M |
| T15 | 插件 + 提示词 + 技能 + 子智能体（7 + 4 + 3 + 2 条） | 路由 + e2e | L |
| T16 | 变更回滚（4 条）+ 调试日志（2 条）                  | 路由 + e2e | M |

> 每域合并前必须跑：`cargo test --workspace -- --test-threads=1` + `cargo xtask verify-wiring` + `npm --prefix desktop-ui run typecheck`。

### M4 长任务与收尾

| #   | 任务                                                                | 交付     | 验收                             | 量 |
| --- | ----------------------------------------------------------------- | ------ | ------------------------------ | - |
| T17 | 5 条长任务改 `202` + `Location`（`thread.send` 系列 + `plugin.reload`）    | 202 语义 | 202 立即返回（<50ms）；进度只从 `evt.*` 来 | M |
| T18 | `http_api_e2e.rs`：设置项改完重启仍在、`thread.send` 202、错误码映射抽查             | 集成测试   | 全绿                             | M |
| T19 | `docs/http-api.md` + 两个 ready JSON 加 `httpPort` + `AGENTS.md` 补红线 | 文档     | 文档里的示例可直接复制执行                  | S |

---

## 六、里程碑与顺序

```
M0（T1–T5）──► M1（T6–T8）──► M2（T9–T12）──► M3（T13–T16）──► M4（T17–T19）
   地基           网关 REST        只读             写              长任务/文档
```

- **M1 结束即可对外交付**：运维/CI 已经能用 REST 管理网关，这是独立可用的最小闭环；
- **M2 结束**：外部工具能读全部状态（`curl` 可完成一次"看配置/看会话/看文件树"）；
- **M3/M4 是"能力补齐"**，不做也不影响 M1/M2 的价值。

---

## 七、测试计划

| 层      | 手段                                                            | 关键约束                                                               |
| ------ | ------------------------------------------------------------- | ------------------------------------------------------------------ |
| 单测     | `routes.rs` 表校验、`http_status` 全码覆盖、参数合并规则                     | 纯函数，无 IO                                                           |
| 集成（节点） | `AgentStore::with_home(&TempAppHome)` 注入                      | 🔴 **绝不 `set_var("A_DA_HOME")`**、绝不碰真实 `~/.a-da`（`AGENTS.md` 三条铁律） |
| 集成（网关） | `Gateway::with_workspaces(临时存储)`                              | 🔴 不让 `Gateway::new` 的默认行为写用户工作区列表                                 |
| e2e    | 起真二进制 + 真 HTTP（`reqwest` 已在依赖里，测试直接用）                         | 端口用 0 自动分配，从 ready 行读                                              |
| 回归     | `ws.rs` 广播改造后跑全量 `cargo test --workspace -- --test-threads=1` | 串行，否则 `agent-node::scripts::tests` 会因并行互相踩（已知问题）                   |

**HTTP 侧不要引入新的测试夹具**：`reqwest` 已是 `agent-rpc` 的依赖，e2e 直接用 `reqwest::Client` 打真端口。

---

## 八、风险与对策

| #  | 风险                                   | 影响                     | 对策                                                                         |
| -- | ------------------------------------ | ---------------------- | -------------------------------------------------------------------------- |
| R1 | HTTP 与 WS 语义漂移（同一方法两条实现）             | 字段/校验/落盘不一致            | **D2 红线 + 门禁**：HTTP 层不允许出现业务分支；新增检查"http.rs 里不得出现 method 名以外的协议常量"         |
| R2 | 广播改造（T9）导致界面不刷新                      | 严重（看起来"点不动"）           | 单独任务 + 专门回归测试；`READ_ONLY_METHODS` 与路由表共用一处；灰度：先只对 `fs.*` / `*.list` 生效     |
| R3 | 网关多租户 token 经 HTTP 越权                | 安全                     | 每条网关路由都过 `TokenScope::allows(workspace)`；e2e 断言跨工作区 403                    |
| R4 | `session.snapshot` 的 `seq` 与 WS 帧对不上 | 客户端自愈死循环（历史事故：376 次/秒） | `GET /api/snapshot` **不返回 `seq`**，或明确标注"仅供外部只读，不可用于增量对齐"；文档写死这条            |
| R5 | 长任务改 202 后调用方仍同步等待                   | 30s 超时、重复提交            | 202 响应带 `Location` + `Idempotency-Key` 可选；`thread.send` 现有 `queued` 语义原样保留 |
| R6 | 新增 `hyper` 依赖拖慢构建                    | 编译时间                   | 已确认 hyper/http/bytes 都在锁文件里；如仍变慢，用 `CARGO_BUILD_JOBS=2`（本机页面文件 2GB 限制）     |
| R7 | 端口暴露面变大                              | 安全                     | 默认只绑 `127.0.0.1`；非回环必须给 token（与网关现有规则一致，`main.rs:67`）                      |

---

## 九、完成定义（DoD）

- [ ] `cargo xtask verify-wiring` 含 `check_http_routes` 且为绿；
- [ ] `cargo test --workspace -- --test-threads=1` 全绿（含新增 e2e）；
- [ ] `npm --prefix desktop-ui run typecheck` 通过（UI 未改，作为"没误伤"的证据）；
- [ ] 网关 REST 12 条、节点只读 20 条可用 `curl` 完成一次完整巡检（见 `docs/http-api.md` 的"5 分钟巡检"示例）；
- [ ] `AGENTS.md` 有"HTTP 必须复用 `Dispatcher::dispatch`"的红线，且 `check_http_routes` 是这条红线的机械保证；
- [ ] `~/.a-da/config.json` 在所有测试跑完后哈希不变（本仓 2026-10-12 的真实事故守门项）。

---

## 十、后续（不在本计划内，但值得排期）

1. **参数类型化**：`docs/code-redundancy-audit.md` §3.7——`dispatch` 现在有 39 处手写解包，     
   HTTP 又加了一条入口之后，参数契约三份（spec / 解包 / 前端类型）的问题会更痛。     
   建议 M3 之前先把高频方法的 `...Params` 结构体补上，HTTP 侧顺带能吃到 `422` 的精确报错。
2. **`ui.*` 迁到客户端**（localStorage），从快照里摘掉 8 个面板开位。
3. **OpenAPI 描述**：路由表已经是结构化的，可机械生成 `openapi.json`（不要让文档手写第三份）。
4. **SSE 备选**：若将来有"非浏览器客户端也要收事件"的需求，可在 HTTP 侧加     
   `GET /api/events`（SSE）；但它只解决下行，`approval.decide` 仍需 HTTP POST 配对——届时要重新评估是否值得。

---

## 附录 A：一次完整巡检（M1+M2 完成后的目标体验）

```bash
# 端口从各自 ready 行的 httpPort 取（都是 127.0.0.1，端口 0 = 自动分配）
#   A_DA_GATEWAY_READY {"ready":true,"port":52353,"httpPort":52354,...}
#   A_DA_HOST_READY    {"ready":true,"port":51066,"token":"...","httpPort":51067,...}
GW=http://127.0.0.1:$GW_HTTP_PORT     # 网关 HTTP 面
API=http://127.0.0.1:$NODE_HTTP_PORT  # 节点 HTTP 面

curl -s $GW/agents | jq
curl -s $GW/workspaces | jq
curl -s "$GW/fs/list?path=E:/codes" | jq '.entries[:3]'

curl -s -H "Authorization: Bearer $TOKEN" $API/api/config | jq '.effort'
curl -s -H "Authorization: Bearer $TOKEN" $API/api/skills | jq 'length'
curl -s -H "Authorization: Bearer $TOKEN" \
     "$API/api/threads/$TID/items?offset=0&limit=20" | jq '.items | length'

# 写完立刻能在 WS 客户端看到快照更新（HTTP 写操作也必须广播）
curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
     -d '{"effort":"medium"}' $API/api/config/effort
```

## 附录 B：复核命令

```bash
grep -n "accept_hdr_async" crates/agent-rpc/src/server/ws.rs      # 单端口纯 WS
sed -n '315,321p' crates/agent-rpc/src/server/ws.rs               # 每次成功都广播
grep -n 'name = "hyper"' -A1 Cargo.lock                            # hyper 已在锁文件
grep -n "fn authorize" -A5 crates/agent-gateway/src/auth.rs        # 可复用的鉴权
```
