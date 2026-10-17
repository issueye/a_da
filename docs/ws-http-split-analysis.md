# 前后端对接方式分析：哪些接口可以转成 HTTP API

> 分析日期：2026-10-17 ｜ 范围：`crates/agent-rpc`（节点 RPC）、`crates/agent-gateway`（网关）、`desktop-ui/src/client`
> 落地计划见 [`docs/ws-http-migration-plan.md`](ws-http-migration-plan.md)
> 方法：`dispatch.rs` 76 个 match 臂逐条读 + `ws.rs` 传输层 + `relay.rs` 网关面 + 前端调用面

---

## 一、结论（先说能拍板的部分）

**当前不是"所有东西都挤在 WebSocket 上"，而是"一条 WS 同时承担了三种语义"**：

1. **请求/响应**（JSON-RPC，带 `id`）——绝大多数方法属于这类，**本来就是 HTTP 的形状**；
2. **服务端主动推送**（`evt.state.snapshot` / `evt.state.patch` / `evt.message.delta` / `evt.card.updated`）——**必须**有长连接或 SSE；
3. **连接态**（握手门 `initialized`、增量帧开关 `patch mode`）——只在"一条连接"这个概念下成立。

**判断结论：**

| 对象 | 能不能转 HTTP | 值不值得 |
|---|---|---|
| **网关管理面** `gateway.*`（12 个） | **完全可以**，其中 10 个是标准 REST、2 个要异步化 | **值得**，收益最大、风险最低 |
| **节点只读查询**（约 20 个：`config.get` / `*.list` / `fs.*` / `thread.items.page` …） | 可以（`GET`） | 值得——给脚本、CLI、调试用；桌面端 UI 不必改 |
| **节点写操作**（约 35 个：设置项、插件/技能/提示词增删、`queue.*`、`change.revert*`） | 可以（`POST/DELETE`） | 有限——**UI 仍然要 WS 收快照**，转了对桌面端没有收益 |
| **长任务**（`thread.send` / `thread.retry` / `gateway.delegate` / `plugin.reload`） | 可以（`POST` + `202` + 事件或轮询） | 只为外部集成做，UI 不需要 |
| **事件推送 + 审批/问答** | **不行**（单向 SSE 替代不了 `approval.decide` 的上行配对，且审批卡是"内存里有人在等"） | **不转** |

**一句话**：可以转，但**转完 WS 也不会消失**——因为界面靠推送刷新。所以正确的目标是
**"把 WS 缩成纯事件通道，把请求/响应挪到 HTTP"**，而不是"用 HTTP 取代 WebSocket"。
桌面端 UI 自身不需要这次迁移；真正受益的是**外部集成与网关**。

---

## 二、现状：传输层与协议形态

### 2.1 传输实现（`crates/agent-rpc/src/server/ws.rs`，331 行）

```rust
// ws.rs:219
let ws_stream = tokio_tungstenite::accept_hdr_async(stream, callback).await?;
```

- **单端口、纯 WS**：`handle_connection` 直接对裸 TCP 流做 WS 升级，没有 HTTP 路由层；
- **鉴权在升级回调里**：只有 `?token=` 与期望值相等才放行（`ws.rs:196-217`），用的是 **query 串**；
- **握手门是连接级状态**：`let mut initialized = false;`，握手前除 `session.initialize` 一律拒（`ws.rs:290-305`）；
- 🔴 **每次成功调用都广播一次全量快照**（`ws.rs:318-320`）：

```rust
let resp = JsonRpcResponse::success(req.id, result);
let _ = tx.send(...);
self.broadcast_snapshot().await;   // ← 连 fs.list / config.get 这种纯读也广播
```

这条对本次分析很关键：**"一次调用 → 所有人收到全量快照"** 是当前 UI 的刷新模型，
HTTP 化之后**写操作仍然必须触发这次广播**，否则界面不动。

### 2.2 协议面（`crates/agent-proto/src/methods.rs`：77 个协议方法 / 76 个 match 臂）

- 请求/响应：`JsonRpcRequest { id, method, params }` → `JsonRpcResponse { id, result | error }`；
- 推送：`evt.*` 四个主题，其中 `evt.state.patch` 需要在 `session.initialize` 里声明
  `client.capabilities: ["state.patch"]` 才启用（`dispatch.rs:1676-1686`）；
- **增量帧开关挂在广播器上**（`bc.enable_patch_mode()`），是**全局开关**而不是按连接的——
  这一点会影响 HTTP 方案（见 §5.3）。

### 2.3 网关面（`crates/agent-gateway/src/relay.rs`）

`gateway.*` 前缀由网关自己处理，其余原样透传给节点（白名单而非黑名单，`relay.rs:48`）。
网关方法共 12 个，见 §4.5。

---

## 三、判定标准：四个问题

一个方法能不能搬 HTTP，只看四件事：

| # | 问题 | 结论 |
|---|---|---|
| 1 | **有没有服务端主动推送的依赖？** | 有 → 留 WS（或另配 SSE） |
| 2 | **是不是"等一个内存里的 future"？**（审批/问答） | 是 → 留 WS（`approval.decide` 必须与"谁在等"配对） |
| 3 | **是不是连接态的一部分？**（握手门、patch mode） | 是 → 留 WS |
| 4 | **执行时长是否秒级以上？** | 是 → HTTP 只做 `202 Accepted`，结果仍走事件/轮询 |

四个都"否"的，就是纯 HTTP 的形状——**这类占了 77 个方法里的绝大多数**。

---

## 四、分类清单

### 4.1 可以直接转 HTTP（**A 类：只读查询**）

| 方法 | 现在的副作用 | HTTP 形态 |
|---|---|---|
| `config.get` | 无 | `GET /api/config` |
| `provider.list` | 无 | `GET /api/providers` |
| `plugin.builtinCatalog` | 无 | `GET /api/plugins/builtin` |
| `prompt.list` | 无（扫目录） | `GET /api/prompts` |
| `skill.list` | **有**：扫描结果回写 store（`store.write()`） | `GET /api/skills`（先去掉回写） |
| `plugin.list` | **有**：同上 | `GET /api/plugins` |
| `subagentProfile.list` | 无 | `GET /api/subagent-profiles` |
| `workspace.entries` | 无 | `GET /api/workspaces` |
| `change.count` | 无 | `GET /api/changes/count` |
| `change.list` | 无 | `GET /api/changes` |
| `thread.items.page` | 无 | `GET /api/threads/{id}/items?offset=&limit=` |
| `fs.roots` | 无 | `GET /api/fs/roots` |
| `fs.list` | 无 | `GET /api/fs/list?path=&showHidden=&limit=` |
| `fs.read_base64` | 无 | `GET /api/fs/read?path=` |
| `debug.hostInfo` | 无 | `GET /api/debug/host` |
| `stats.promptChars` | 无 | `GET /api/stats/prompt-chars` |
| `session.snapshot` | **有**：`sync_threads_from_disk` 回写内存 | `GET /api/snapshot`，但**`seq` 与 WS 帧耦合**，见 §5.3 |
| `config.presets` | **网络 IO**（拉供应商预设） | `GET /api/config/presets`（慢，注意缓存） |
| `provider.fetchModels` | **网络 IO**（打供应商接口） | `GET /api/providers/{id}/models`（慢，建议带超时） |
| `plugin.validate` | 无（重跑插件校验，可能慢） | `POST /api/plugins/{id}/validate`（要跑代码，不算纯读） |
| `debug.trace` | **写**：`store.push_log` | ⚠️ 名字像读，其实是**写日志**；应为 `POST /api/debug/logs` |

> 只读项里 **4 个带服务端写**：`skill.list` / `plugin.list` 的扫描结果回写、`debug.trace` 实为写日志、
> `session.snapshot` 会 `sync_threads_from_disk` 回写内存。转 GET 之前要么把回写挪走，要么承认它不是幂等 GET。

### 4.2 可以转 HTTP（**B 类：写操作，秒级内完成**）

设置与配置：`config.setProvider`、`config.setApproval`、`config.setEffort`、`config.setRetryCount`、
`provider.save`、`provider.delete`、`provider.setActive`、`workspace.add`、`workspace.remove`、
`workspace.openPublic`。

线程：`thread.create`、`thread.delete`、`thread.update`、`thread.setMode`、`thread.setWorkspace`、
`thread.focus`、`thread.abort`、`thread.items.page`（已列 A）。

UI 视图：`ui.setShell`、`ui.openTab`、`ui.closeTab`、`ui.activeThread`、`ui.activeProject`
（**注意**：这 5 个是**客户端视图状态**，其中 `ui.setShell` 一次改 8 个面板开合位，见 §7.2）。

队列与回滚：`queue.clear`、`queue.promote`、`queue.remove`、`change.revertFile`、
`change.revertAll`、`change.revertCard`、`change.revertCheckpoint`。

插件 / 技能 / 提示词：`plugin.setEnabled`、`plugin.load`、`plugin.delete`、`plugin.config.set`、
`plugin.secret.set`、`plugin.capabilities.set`、`plugin.createTemplate`、`prompt.setEnabled`、
`prompt.create`、`prompt.update`、`prompt.delete`、`skill.setEnabled`、`skill.create`、
`skill.delete`、`subagentProfile.setEnabled`、`subagentProfile.delete`、`debug.log.clear`。

> 这批转 HTTP 的**实际收益只对外部工具成立**：桌面端改完设置还是要靠 WS 的 `evt.state.snapshot`
> 刷新界面。**不要为了"用 HTTP"去动 UI 的调用路径。**

### 4.3 长任务：HTTP 只能给 `202`（**C 类**）

| 方法 | 时长特征 | HTTP 形态 |
|---|---|---|
| `thread.send` | 立即返回 `{accepted, queued}`，真正的轮次在后台跑 | `POST /api/threads/{id}/messages` → `202`；进度走 `evt.*` |
| `thread.editAndResend` | 同上 | `POST /api/threads/{id}/messages:resend` → `202` |
| `thread.retry` | 同上 | `POST /api/threads/{id}/retry` → `202` |
| `thread.compact` | 同步跑一轮压缩，秒级 | `POST /api/threads/{id}/compact` → `202` |
| `plugin.reload` / `plugin.setEnabled` | 重装配（重新执行插件脚本），可能数秒 | `POST /api/plugins/{id}/reload` → `202` |
| `gateway.delegate` | **等目标 agent 跑完**才返回 | `POST /api/delegations` → `202` + `GET /api/delegations/{id}` |

这 6 个的关键不是"能不能"，而是**语义要改成异步**：现在的调用方都在等一个 `result`，
HTTP 版必须约定"`202` + 事件/轮询"，否则就是一条挂 30 秒的 HTTP 请求。

### 4.4 必须留在 WS（**D 类**）

| 项 | 为什么不能转 |
|---|---|
| `evt.state.snapshot` / `evt.state.patch` / `evt.message.delta` / `evt.card.updated` | 服务端**主动**推送，HTTP 没有下行通道；SSE 可以替代这一半，但见下 |
| `session.initialize` | 它做两件连接级的事：置 `initialized` 门、开 `patch mode`。HTTP 无连接态 |
| `approval.decide` / `question.answer` | 表面是请求，实质是**回答一个只在内存里 await 的 future**。要转 HTTP 必须先给每个待决项发**稳定 id**，并保证"决定"能命中那个等待者；而且它是**双向**语义（界面上有卡在等），只改上行会让状态机更难守 |
| `thread.send` 的流式增量 | 一次轮次的 token 级输出只能推，不能拉（除非改成轮询，那会让"流式打字"变成"每 200ms 刷一段"） |

**关于 SSE**：SSE 只能替代"服务端→客户端"，`approval.decide` 仍然要上行通道。
真要统一，等价于"SSE + 一条 POST 上行"，两条通道 vs 一条 WS——**当前形态更简单**。

### 4.5 网关管理面（收益最高的一类）

| 方法 | HTTP |
|---|---|
| `gateway.listAgents` | `GET /agents` |
| `gateway.status` | `GET /agents/{id}` |
| `gateway.attach` | `POST /agents/{id}/attach` |
| `gateway.register` | `PUT /agents/{id}`（幂等注册） |
| `gateway.detach` | `DELETE /agents/{id}` |
| `gateway.fs.roots` | `GET /fs/roots` |
| `gateway.fs.listDirectory` | `GET /fs/list?path=` |
| `gateway.fs.makeDirectory` | `POST /fs/mkdir` |
| `gateway.workspaces.list` | `GET /workspaces` |
| `gateway.workspaces.remove` | `DELETE /workspaces?path=` |
| `gateway.delegate` | `POST /delegations` → `202` |
| `gateway.cancelDelegation` | `DELETE /delegations/{id}` |

网关这 12 个（含 `delegate` 的两个）**没有一个是推送依赖**，全部是"看一眼/改一下"的管理动作。
这是本仓最适合先做 HTTP 的地方——而且它天然面向**非桌面客户端**（运维脚本、健康检查、CI）。

---

## 五、如果要做，方案长什么样

### 5.1 不要写第二套 handler（最重要的一条）

`Dispatcher::dispatch(&self, method: &str, params: Value) -> Result<Value, ProtocolError>`
已经是"方法与参数进、JSON 出"的纯面。HTTP 侧只需要：

```
请求 → 解析路径与 body → 组装 (method, params) → dispatcher.dispatch(...) → 序列化
```

**不要**为 HTTP 重写 `config.save` 之类的逻辑——一旦两套实现，字段/校验/落盘必然漂移
（这正是 `dispatch.rs` 里 39 处手写解包已经埋下的问题）。

### 5.2 端口与路由

`ws.rs` 现在直接 `accept_hdr_async`，没有 HTTP 层。两条路：

- **A（推荐，改动小）**：**另起一个端口**跑 HTTP（`ada-coding --http-port`）。
  WS 保持原样，零回归风险；
- **B**：同端口双协议——在 `accept` 之后先 `peek` 首行，`GET/POST /api/` 走 HTTP，
  带 `Upgrade: websocket` 的走 WS。省一个端口，但要自己写一层 HTTP 解析（或引入 hyper/axum）。

### 5.3 迁移时必须处理的四个耦合点

| 耦合 | 现状 | 处理办法 |
|---|---|---|
| **广播触发** | 每次成功 dispatch 都 `broadcast_snapshot()` | HTTP 写操作要调用同一个 hook；**只读不要触发**（顺手修掉 §7.1 的浪费） |
| **握手门** | `initialized` 是连接级 bool | HTTP 侧用 `Authorization: Bearer <token>` 直接放行，不模拟握手 |
| **patch mode** | `bc.enable_patch_mode()` 是**广播器全局开关** | HTTP 不能让单个请求打开它；要么改成"按连接注册"，要么作为启动参数 |
| **快照 seq** | `session.snapshot` 返回 `broadcaster.last_sent_seq()` | HTTP 的 `GET /api/snapshot` 只能给"最新 seq"，与 WS 客户端的 `baseSeq` 对不上就会触发自愈循环（`dispatch.rs:1601-1610` 记着这个 376 次/秒的事故） |

### 5.4 鉴权

WS 用 `?token=`（`ws.rs:199-206`）。HTTP 必须换成 `Authorization: Bearer`（query 里的 token 会进日志/Referer）。
两套鉴权要读**同一处**配置，别各自解析。

---

## 六、收益与成本（按对象算，不按"技术优雅"算）

| 对象 | 转 HTTP 的收益 | 结论 |
|---|---|---|
| **桌面端 UI（Electron）** | **接近零**：界面靠 `evt.state.snapshot` 刷新，WS 无论如何都得留着 | **不动 UI** |
| **外部集成（脚本 / CLI / CI / 第三方）** | **高**：`curl` 就能查状态、改配置、发起一轮对话；不需要实现 WS 客户端与 JSON-RPC 握手 | **这是真正的目标** |
| **网关管理面** | **高**：健康检查、实例列表、工作区增删天然是 REST；也方便反向代理/网关层做鉴权 | **优先做** |
| **调试与可观测** | **中**：`GET /api/snapshot`、`/api/debug/host` 可直接在浏览器打开 | 顺带获益 |

**成本**：约 1 个新模块（HTTP 路由 + 鉴权 + 错误映射）+ 一次性 4 个耦合点改造；
**不需要**改任何 dispatch 业务逻辑——这是这个方案最大的便宜之处。

---

## 七、顺带发现的两个问题（与本次分析直接相关，但不属于"转 HTTP"）

### 7.1 只读请求也会触发全量快照广播

`ws.rs:320`：成功即 `broadcast_snapshot()`。也就是说打开一次文件浏览器（`fs.list`）、
读一次配置（`config.get`），都会让**所有连接的客户端**重收一遍全量快照。
文件树点开一个目录扇出 N 次请求 = N 次全量广播。

这与 `AGENTS.md` 里记的增量帧（`evt.state.patch`）设计是矛盾的：本来只有"状态变了"才需要推。
**建议**：把广播从"传输层无条件触发"改成"dispatch 返回是否脏"（或只在写方法后触发）。
这一项即使不转 HTTP 也应该修。

### 7.2 `ui.*` 是客户端视图状态，却放在服务端

`ui.setShell` 一次写 8 个面板开合位（`debugOpen` / `settingsOpen` / `pluginsOpen` / `changesOpen` /
`paletteOpen` / `sidebarOpen` / `searchOpen` …），`ui.openTab` / `closeTab` 同理，**每次改动还会广播全量快照**。

这类状态天然属于客户端（localStorage / 浏览器内存），放在服务端的唯一理由是"多窗口同步"。
如果确实要同步，也应该是 HTTP `PUT /api/ui` 或独立的轻量通道，而不是挤在状态快照里。

---

## 八、建议的执行顺序

**第 1 步（收益/风险比最高，1～2 天）**
网关管理面转 REST：`GET /agents`、`GET /agents/{id}`、`DELETE /agents/{id}`、
`GET /workspaces`、`GET /fs/*`、`gateway.delegate` 异步化。网关本来就不碰会话状态，边界干净。

**第 2 步（1 天）**
节点侧只读 `GET`：`config.get`、`provider.list`、`plugin/prompt/skill.list`、`fs.*`、
`change.list`、`thread.items.page`、`debug.hostInfo`。同时**去掉只读的副作用**与**只读不广播**。

**第 3 步（按需）**
写操作 `POST/DELETE`，只为外部集成开放；桌面端继续用 WS，两条入口共用 `dispatch`。

**明确不做**
不把 `evt.*`、`approval.decide`、`question.answer`、`session.initialize` 搬出 WS；
不让 UI 改成 HTTP（收益为零、回归风险不小）。

---

## 附录：复核命令

```bash
# 方法清单（77 个协议方法；76 个臂，其中 change.revert* 两条共用一个臂）
grep -cE '^            [A-Z_][A-Z0-9_]*( \| [A-Z_][A-Z0-9_]*)* =>' crates/agent-rpc/src/server/dispatch.rs

# 网关面
sed -n '/pub mod methods {/,/^}/p' crates/agent-gateway/src/relay.rs

# 单端口纯 WS（无 HTTP 路由）
grep -n "accept_hdr_async" crates/agent-rpc/src/server/ws.rs

# 每次成功调用都广播
sed -n '315,321p' crates/agent-rpc/src/server/ws.rs

# 握手门（连接级状态）
sed -n '290,305p' crates/agent-rpc/src/server/ws.rs

# 分类扫描脚本（本次口径）
python tmp/scan_methods.py
```
