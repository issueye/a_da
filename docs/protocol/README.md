# a_da 接口文档（WebSocket 线协议 + HTTP API）

> **单一入口**：客户端与宿主之间的全部对外接口都在这里——**一套方法集，两种传输形态**。
> 机器可读真源是 [`spec/proto/`](../../spec/proto/README.md)（方法名 / `kind` / `since` / 错误码），
> 本文解释"怎么用"。**两处不一致时以 `spec/proto/` 为准**——且这种不一致会被
> `cargo xtask verify-wiring` / `cargo xtask compat` / `cargo test -p agent-proto` /
> `cargo xtask verify-docs`（断言本文覆盖全部事件主题）抓住。

---

## 0. 一页总览

| 面 | 传输 | 路径 | 用途 | 服务端事件 |
|---|---|---|---|---|
| **WS 线协议** | JSON-RPC 2.0 over WebSocket | `/rpc` | 全部方法；流式输出、状态增量、审批/提问、长任务 | ✅ 推送 `evt.*` |
| **HTTP API** | HTTP/1.1 unary | `/api/*` | 一问一答的命令/查询（配置、工作区、插件、文件、快照读取） | ❌ 不承载事件 |
| **静态页面** | HTTP GET | `/`、`/assets/*` | 前端产物（网页形态，见 §1.2） | — |

三条要点：

1. **方法集同源**：每个 `/api/*` 端点都是某个 JSON-RPC 方法的单次调用封装（§3.3），
   两者参数与结果形状一致——HTTP 只是换了信封，没有第二套语义。
2. **事件只能走 WS**：`evt.state.snapshot` / `evt.state.patch` / `evt.message.delta` /
   `evt.card.updated` 全部经 `/rpc` 推送，HTTP 不做轮询。
3. **人和流式只能走 WS**：审批（`approval.decide`）、提问（`question.answer`）、
   `thread.send` 这类"要等模型说很久"的操作没有 HTTP 形态（§8 说明了为什么）。

---

## 1. 部署形态与端点

### 1.1 两种宿主形态

| 形态 | 二进制 | 客户端连哪里 | 说明 |
|---|---|---|---|
| **直连 agent** | `ada-coding --host --port <p> --token <t>` | `ws://127.0.0.1:<p>/?token=<t>` | 单工作区、单进程；协议面就是 L0/L2 方法 |
| **经网关** | `ada-gateway --port <p> [--token ...]` | `ws://127.0.0.1:<p>/rpc`，HTTP `http://127.0.0.1:<p>/api/*` | 一工作区一进程、按需拉起/复用；多出 `gateway.*` 管理面 |

网关把页面、`/rpc`、`/api/*` **放在同一个端口**（同一个 axum Router，见
`crates/agent-gateway/src/router.rs`）。桌面端拉起的网关固定用 **52353**（被占用时顺延到 52363），
实际端口从 READY 行或发现文件里取（§1.3）。

**网关 ↔ agent 的本体链路**有两种，由 `A_DA_GATEWAY_AGENT_LINK` 选择（默认 `tcp`）：

| 值 | agent 侧 | 端点字符串 | 特点 |
|---|---|---|---|
| `tcp` | `--host --port 0 --token <t>` | `ws://127.0.0.1:<port>/rpc?token=…` | 回环端口；`wscat` 可直连调试 |
| `pipe` | `--host --pipe <name> --token <t>` | `pipe://\\.\pipe\…?token=…`（Windows）/ `unix:///tmp/….sock?token=…` | **不占端口、不监听 TCP**；生命周期跟子进程走 |

两种形态跑**同一套** WS 帧与 token 握手，客户端察觉不到差别——只有网关在拉起实例、
注册表存端点、以及 `relay` / `/api/*` / `gateway.delegate` 连过去时用不同字节流。
端点解析只有一处：`crates/agent-gateway/src/link.rs`。

协议版本：`PROTOCOL_VERSION = "1.0"`（`crates/agent-proto/src/methods.rs`）。

### 1.2 网关路由表（一个端口 = 整站）

| 请求 | 去向 |
|---|---|
| `GET /rpc` + `Upgrade: websocket` | `relay::ws_handler`：**先鉴权再升级**（Origin / token） |
| `GET /rpc`（无升级头） | `426 Upgrade Required` |
| `GET /api/*` | `http_api`：unary 调用（§3） |
| `GET /`、`/threads/*`、`/overview`、`/providers*`、`/workspaces*`、`/extensions` | **SSR 页面**（`crates/agent-web`；2026-10 起是唯一的界面） |
| `GET /threads/{id}/stream` | SSR 的 SSE 实时更新 |
| `GET /static/*`、`/ext/{id}/assets/*` | SSR 内嵌静态资源 / 插件资源 |
| `GET /web-ui/frontend-plugins` | SSR 前端插件自省（**不是** `/api/plugins`） |
| `POST` 等非 GET/HEAD（除 `/api/*`） | SSR 表单动作 |

静态资源**不需要 token**（页面只是 JS/CSS，不含用户数据）；要 token 才能打开"输入 token 的页面"是死锁。
`/rpc` 在升级之前鉴权：Origin 不合法 → `403`，token 不对 → `401`，都是普通 HTTP 状态码。

### 1.3 端口发现

桌面端每次启动都写 `<A_DA_HOME>/gateway_endpoint.json`：

```json
{
  "port": 52353,
  "pid": 1234,
  "workspace": "E:/proj",
  "url": "http://127.0.0.1:52353/",
  "rpc": "ws://127.0.0.1:52353/rpc",
  "authRequired": false
}
```

**不写 token**：凭据只从启动参数或 `gateway.pair` 配对码来，不落到磁盘上等人捡。

---

## 2. 鉴权与作用域

### 2.1 WS 面

| 项 | 规则 |
|---|---|
| 凭证 | **握手期 query 参数**：`ws://<host>:<port>/?token=<token>`（网关是 `/rpc?token=`） |
| 校验时机 | 只在握手，**不是每条消息校验**——token 泄漏即可直接建连 |
| 未通过 | 握手直接返回 **HTTP 401**，不建立连接 |
| 配对码 | 全局 token 之外还有一次性配对码 `gateway.pair {code}`（网关 `auth.rs`） |
| 作用域 | `--token <t>=<ws1>\|<ws2>`：该 token 只能在列出的工作区内操作 |

> 实现：`crates/agent-rpc/src/server/ws.rs`（`bind` / `bind_with_engine`，握手回调校验）；
> 网关侧 `crates/agent-gateway/src/auth.rs` / `relay.rs`。

### 2.2 HTTP 面

| 项 | 规则 |
|---|---|
| 凭证 | `Authorization: Bearer <token>`；兼容旧的 `?token=` |
| 未认证 | **只允许** `GET /api/gateway/info`；其余一律 `401` |
| Origin | 带 `Origin` 就必须在 `--allow-origin` 白名单（或回环 `localhost`/`127.0.0.1`），否则 `403` |
| CORS | 放行的 Origin 回 `Access-Control-Allow-*`；`OPTIONS` 预检返回 `204` |

### 2.3 SSR 页面面（`--web-ui ssr`）

| 项 | 规则 |
|---|---|
| 浏览器凭据 | `/__auth` 接受网关 `--token` 或一次性配对码；校验后换发随机 session id 的 `HttpOnly; SameSite=Strict` cookie（网关 token 不进 cookie） |
| 兼容项 | `--web-token` 是静态浏览器令牌；与网关 token 任一通过即可 |
| Host | 只接受绑定主机 + 回环别名，且端口必须与监听端口完全一致（挡 DNS rebinding）；配置 `--web-public-url` 后额外接受该公网主机 |
| Origin | 写请求的 `Origin` 必须与 `Host` 一致，或等于 `--web-public-url` 的公网来源；`Origin: null` 仅在 `Sec-Fetch-Site: same-origin` 时放行 |
| 反向代理 | `--web-public-url https://example.com`（`A_DA_WEB_PUBLIC_URL`）显式放行代理后的 Host/Origin；https 时 session cookie 带 `Secure`。不信任 `X-Forwarded-*` |
| 静态资源 | `/static/*` 免 token（只含 JS/CSS） |
| 作用域 | SSR 内部调用不经过网关 token；页面绑定网关默认工作区 |

### 2.4 工作区（多实例路由）

| 项 | 规则 |
|---|---|
| 指定方式 | query `?workspace=<abs path>` 或请求头 `X-A-Da-Workspace`；缺省用网关默认工作区 |
| 路径形态 | 统一分隔符、折叠 `.`/`..`、Windows 折叠大小写——`E:\x` / `E:/x/` / `e:/X` 是同一个工作区 |
| 作用域约束 | 受限 token 的 workspace 白名单**同时约束** query/body 里的 workspace |
| 🔴 踩坑 | **受限 token 请把工作区放 query/header**：body 里的 workspace 在 token 鉴权**之后**才检查，只写 body 会先按默认工作区被拒 |

### 2.5 对外部署的硬规则

网关只提供明文 `ws://`：

1. **非回环 `--host` 必须配 `--token`**；
2. **必须显式 `--allow-plaintext`** 承认"明文上的 token 可嗅探"（典型：反向代理终止 TLS）。

两条都是**拒绝启动**而不是警告（失败安全）。

---

## 3. 帧与信封

### 3.1 WS 帧

```json
// → 客户端请求
{ "jsonrpc": "2.0", "id": 7, "method": "thread.send", "params": { "threadId": "t1", "prompt": "..." } }

// ← 宿主响应
{ "jsonrpc": "2.0", "id": 7, "result": { "threadId": "t1" } }

// ← 宿主错误
{ "jsonrpc": "2.0", "id": 7, "error": { "code": -32601, "message": "Method not found (code: -32601)" } }

// ← 宿主通知（无 id）
{ "jsonrpc": "2.0", "method": "evt.state.snapshot", "params": { "seq": 42, "...": "..." } }
```

### 3.2 HTTP 信封

- **请求**：写接口的 body 基本就是该 JSON-RPC 方法的 `params`；资源 id 走路径段，
  工作区/文件路径走 query（Windows 路径里有 `\`，不适合做路径段）。
- **成功**：HTTP `200`，body 是该方法的 `result`；`result` 为 `null` 时返回 `{"ok":true}`。
- **失败**：`{"error":{"code":<JSON-RPC code>,"message":"...","data":...}}`，HTTP 状态按 §6.3 映射。
- **缓存**：`GET /api/snapshot` 返回 `ETag: "snap-<seq>"`；带同值 `If-None-Match` 得到 `304`。

```bash
# 读配置
curl -H 'Authorization: Bearer secret' 'http://127.0.0.1:52353/api/config?workspace=E:/proj'

# 改推演力度
curl -X PUT -H 'Authorization: Bearer secret' -H 'Content-Type: application/json' \
     -d '{"effort":"high"}' 'http://127.0.0.1:52353/api/config/effort?workspace=E:/proj'

# 快照 ETag → 304
curl -i -H 'Authorization: Bearer secret' 'http://127.0.0.1:52353/api/snapshot?workspace=E:/proj'
# -> ETag: "snap-42"
curl -i -H 'Authorization: Bearer secret' -H 'If-None-Match: "snap-42"' \
     'http://127.0.0.1:52353/api/snapshot?workspace=E:/proj'
# -> 304 Not Modified
```

### 3.3 方法 ↔ 端点总表

**纯查询**

| HTTP | 路径 | 对应 RPC |
|---|---|---|
| GET | `/api/snapshot` | `session.snapshot`（ETag） |
| GET | `/api/threads/{threadId}/items?from=&count=` | `thread.items.page` |
| GET | `/api/workspaces/sessions?workspace=` | `workspace.entries` |
| GET | `/api/config` | `config.get` |
| GET | `/api/config/presets` | `config.presets` |
| GET | `/api/providers` | `provider.list` |
| GET | `/api/fs/roots` | `fs.roots` |
| GET | `/api/fs/list?path=&showHidden=&limit=` | `fs.list` |
| GET | `/api/fs/raw?path=` | `fs.read_base64`（返回 `{dataUri,path}` JSON） |
| GET | `/api/threads/{threadId}/changes/count` | `change.count` |
| GET | `/api/threads/{threadId}/changes` | `change.list` |
| GET | `/api/plugins?workspace=` | `plugin.list` |
| GET | `/api/plugins/builtin` | `plugin.builtinCatalog` |
| GET | `/api/prompts?workspace=` | `prompt.list` |
| GET | `/api/skills?workspace=` | `skill.list` |
| GET | `/api/subagent-profiles?workspace=` | `subagentProfile.list` |
| GET | `/api/debug/host` | `debug.hostInfo` |
| POST | `/api/stats/prompt-chars` | `stats.promptChars`（纯计算） |

**unary 写**

| HTTP | 路径 | 对应 RPC |
|---|---|---|
| PUT | `/api/config/provider` | `config.setProvider`（body 可为 `{config}` 或直接字段） |
| PUT | `/api/config/approval` | `config.setApproval` |
| PUT | `/api/config/effort` | `config.setEffort` |
| PUT | `/api/config/retry-count` | `config.setRetryCount` |
| PUT | `/api/providers/{id}` | `provider.save`（body 可为 `{provider}` 或直接字段；id 以路径为准） |
| DELETE | `/api/providers/{id}` | `provider.delete` |
| POST | `/api/providers/{id}/activate` | `provider.setActive` |
| POST | `/api/workspaces` | `workspace.add` |
| DELETE | `/api/workspaces?workspace=` | `workspace.remove` |
| POST | `/api/workspaces/public/open` | `workspace.openPublic` |
| POST | `/api/fs/mkdir` | `fs.mkdir` |
| POST | `/api/threads` | `thread.create` |
| DELETE | `/api/threads/{id}` | `thread.delete` |
| PATCH | `/api/threads/{id}` | `thread.update` |
| POST | `/api/threads/{id}/focus` | `thread.focus` |
| PATCH | `/api/threads/{id}/mode` | `thread.setMode` |
| PATCH | `/api/threads/{id}/workspace` | `thread.setWorkspace` |
| DELETE | `/api/threads/{id}/queue` | `queue.clear` |
| POST | `/api/threads/{id}/queue/{index}/promote` | `queue.promote` |
| DELETE | `/api/threads/{id}/queue/{index}` | `queue.remove` |
| POST | `/api/approvals/{callId}/decision` | `approval.decide` |
| POST | `/api/questions/{callId}/answer` | `question.answer` |
| POST | `/api/threads/{id}/changes/revert-card` | `change.revertCard` |
| POST | `/api/threads/{id}/changes/revert-checkpoint` | `change.revertCheckpoint` |
| POST | `/api/threads/{id}/changes/revert-file` | `change.revertFile` |
| POST | `/api/threads/{id}/changes/revert-all` | `change.revertAll` |
| PATCH | `/api/plugins/{id}/capabilities` | `plugin.capabilities.set`（body 可为 `{patch}` 或直接字段） |
| PATCH | `/api/plugins/{id}/config` | `plugin.config.set`（body 可为 `{values}` 或直接字段） |
| PUT | `/api/plugins/{id}/secret` | `plugin.secret.set` |
| PATCH | `/api/plugins/{id}/enabled` | `plugin.setEnabled` |
| DELETE | `/api/plugins?filePath=&workspace=` | `plugin.delete` |
| POST | `/api/plugins/template` | `plugin.createTemplate` |
| POST | `/api/prompts` | `prompt.create`（body 可为 `{options}` 或直接字段） |
| PATCH | `/api/prompts` | `prompt.update`（body 可为 `{item}` 或直接字段） |
| DELETE | `/api/prompts?filePath=` | `prompt.delete` |
| PATCH | `/api/prompts/enabled` | `prompt.setEnabled` |
| PATCH | `/api/skills/{id}/enabled` | `skill.setEnabled` |
| POST | `/api/skills` | `skill.create` |
| DELETE | `/api/skills/{id}?workspace=` | `skill.delete` |
| PATCH | `/api/subagent-profiles/{id}/enabled` | `subagentProfile.setEnabled` |
| DELETE | `/api/subagent-profiles/{id}?workspace=` | `subagentProfile.delete` |
| DELETE | `/api/debug/log` | `debug.log.clear` |

**网关管理面**（只有经网关连接时存在）

| HTTP | 路径 | 对应 WS 方法 |
|---|---|---|
| GET | `/api/gateway/info` | `gateway.info`（**公开**，无需 token） |
| GET | `/api/gateway/status` | `gateway.status` |
| GET | `/api/gateway/agents` | `gateway.listAgents` |
| POST | `/api/gateway/attach` | `gateway.attach` |
| DELETE | `/api/gateway/agents/{id}` | `gateway.detach` |
| GET | `/api/gateway/fs/roots` | `gateway.fs.roots` |
| GET | `/api/gateway/fs/list?path=&directoriesOnly=&showHidden=&limit=` | `gateway.fs.listDirectory` |
| POST | `/api/gateway/fs/mkdir` | `gateway.fs.makeDirectory` |
| GET | `/api/gateway/workspaces` | `gateway.workspaces.list` |
| DELETE | `/api/gateway/workspaces?workspace=` | `gateway.workspaces.remove` |
| POST | `/api/gateway/delegations/{id}/cancel` | `gateway.cancelDelegation` |

> `gateway.register`（agent 侧注册）、`gateway.pair`（配对码换 token）、`gateway.delegate`（跨网关派活）
> **只有 WS 形态**：前者是内部面，后两者是长任务/一次性凭据交换。

**仅 WS 的方法**：`thread.send` / `thread.retry` / `thread.editAndResend` / `thread.abort` /
`thread.compact`、`plugin.load` / `plugin.reload` / `plugin.invoke` / `plugin.validate`、
`provider.fetchModels` / `config.checkProvider`、`task.list` / `task.kill`、
`session.initialize`、`ui.*`、`gateway.delegate`。原因见 §8。`plugin.invoke` 只由 SSR 宿主在服务端调用，
浏览器经 `/ext/{id}/backend/{binding}` 中转，不直接接触该方法。

---

## 4. 事件与状态帧

### 4.1 主题

`ALL_EVENTS` 登记 **3 个事件主题**，另有增量帧主题 `evt.state.patch`（能力协商后启用）：

| 主题 | 语义 |
|---|---|
| `evt.state.snapshot` | 全量快照：首帧、自愈、重连 |
| `evt.state.patch` | 增量帧：只推指纹变了的 item + `meta` 小字段 + `removed` |
| `evt.message.delta` | 流式增量文本 |
| `evt.card.updated` | 单卡片更新 |

`seq` 单调递增；客户端按 `seq` 去重、断线重连后对账。跳号 → 主动拉一次全量快照自愈。

### 4.2 增量帧

背景：一条长会话（实测 **822 个 item / 一帧 1852 KB**；合成负载 **800 item / 3705 KB**）时，
全量快照 16ms 一帧（大帧已降到 100ms）→ 每秒几十 MB 的序列化 + IPC + 前端 JSON 解析。
根治办法是按 item 增量。

```jsonc
// evt.state.patch —— 取代高频的全量 evt.state.snapshot（后者保留给首帧/自愈/重连）
{
  "seq": 42, "baseSeq": 41,      // 客户端发现不连续 → 主动请求一次全量快照自愈
  "meta": { /* queue / running / runningThreadIds / waitingThreadIds / log /
               config / ui / workspace / activeThreadId / providers / appearance */ },
  "threads": [{ "id": "…", "title": "…", "workspace": "…", "agentId": "…", "isSubagent": false,
                "itemCount": 12, "updatedAt": 1761000000000 }],
  "upserted": [{ "threadId": "…", "item": { /* 完整 item */ } }],
  "removed":  [{ "threadId": "…", "itemId": "…" }],
  "itemsWindow": { "from": 740, "to": 822, "total": 822 }
}
```

> 🔴 **`items` 会被裁剪，`itemCount` / `updatedAt` 不会**：快照与增量帧只给
> "激活会话 / 打开的 Tab / 运行中 / 子智能体"这几种线程带 `items`，其余线程的 `items`
> 是空数组（压帧体积）。所以判断"这条会话有没有内容""最近活动是什么时候"必须读
> `itemCount` / `updatedAt`，拿 `items.length` 当判据会把整屏历史会话误判成空会话
> （实测侧栏只剩一条"新会话"）。

四个环节：

1. **能力协商**：`session.initialize` 的 `client.capabilities` 里声明 `"state.patch"`；
   **没声明就继续发全量快照**——这是"宿主先落地、客户端后跟上"的安全绳，也是旧客户端/回滚路径的保证。
2. **宿主侧版本指纹**：`crates/agent-rpc/src/state/store.rs::item_visible_key()`
   （id + 文本长度 + 状态 + 完成时刻 …）；`StateBroadcaster` 按连接记住 `{itemId → 指纹}`，
   只推**指纹变了**的 item；连接首帧 / `baseSeq` 失配 / 有 item 被删 → 走全量或显式 `removed`。
3. **客户端合并**：SSR 页面在 `crates/agent-web/static/chat.js` 里维护
   同样的 `Map<threadId, Map<itemId, Item>>`；patch 只 upsert/remove；`meta` 覆盖小字段；
   `seq` 不连续 → 发一次 `session.snapshot` 并全量应用（自愈）。
   （原 React 客户端的 `desktop-ui/src/client/snapshot-merge.ts` 已随 `desktop-ui/` 移除。）
4. **分页**：`thread.items.page(threadId, from, count)`（HTTP 侧是
   `GET /api/threads/{threadId}/items?from=&count=`）让"显示更早的 60 条"按需拉取，
   内存里只留窗口。

### 4.3 🔴 顺序：`upserted` 按 **id** 排序，所以定位只能按 `at`

`upserted` 要保证帧内容可复现，因此按 `(threadId, itemId)` 排序——**到达顺序没有意义**。
客户端给"本地还没有的卡片"定位就只能按 `at`（`crates/agent-web/static/chat.js` 的
`insertByItemTime`：按时间插入，**等时间戳保持已有相对顺序**）。

于是有一条必须**两处都守**的不变量：**同一个会话里卡片的 `at` 严格递增**，且与显示顺序一致。

- **磁盘回填**（`agent_node::session::SessionManager::restore_all_threads`）：一条 assistant 消息
  会派生好几张卡（思考 / 正文 / 工具），它们原本共用消息自己的毫秒时间戳，先后就落到 id 字典序上——
  `..._restored_asst_*` 恰好排在 `..._restored_think_*` 前面，于是**轮次一收尾（内存卡片被磁盘回填
  整份换掉、id 全变），思考块从最上面掉到最下面**（用户实拍）。现在按折叠顺序发时间戳
  （`CardStamps`），顺序为**思考 → 正文 → 工具卡**，与流式现场一致。
- **内存侧**（`agent_rpc::state::store::next_item_at`）：同一毫秒里连建两张卡（思考 + 首段正文、
  非流式模型一次返回的思考与正文）也会踩同一个坑，所以新卡片的 `at` 不许小于上一张。

守门：`agent-node` 的 `test_restored_cards_have_increasing_at_so_client_order_matches`
（按客户端那条规则重放一遍，断言顺序一致）+ 实机验收
`bun tools/verify-card-order-e2e.mjs [--with-tool]`（本地假模型走真协议，比较
**收尾前后界面上看到的顺序**——只读服务端快照是看不出问题的，服务端数组一直是对的）。

### 4.4 已知取舍

- `meta` 仍是全量（但它只有几十字节到几 KB），真正的大头是 `items`，所以增量只做 items 就够了。
- `log`（调试条目）已按字节限幅（`DEBUG_PAYLOAD_LIMIT`），随 `meta` 走，不做增量。
- 旧路径 `evt.state.snapshot` **保留**：首帧、自愈、重连、未声明能力的客户端都走它。

---

## 5. 审批与提问（**唯一需要"人"的通道**）

审批的**策略**归插件 `approval-guard`、**执行**归核心（`AGENTS.md` §14），线上表现为两条方法 + 一条状态：

1. 引擎判定"要问" → 发 `ApprovalRequested`（**带 `args`**，审批卡要能回答"我在批准什么"）
   → 工具卡片进入 `status: "waiting_approval"`；
2. 界面在**输入框上方的浮动审批窗口**渲染批准/拒绝按钮 → 调 `approval.decide`
   （`{ callId, approved }`，HTTP 侧 `POST /api/approvals/{callId}/decision`）→ 唤醒等待中的审批闸门；
3. 拒绝时**不抛错**，而是把拒绝原因作为**工具结果**回给模型（模型知道被拒了，不会以为成功）。

同一形态用于 `ask_user` / `question.answer`（`POST /api/questions/{callId}/answer`，模型主动提问）。

> ⚠️ **超时**：审批闸门默认 300s。超时按 `ApprovalGate::direction()` 处理——
> 默认 `Closed`（拒绝），并记 `by: Timeout` 便于事后分辨来路。
> `Scope`（路径沙箱）的失败方向**刻意不可配置**：路径越界是确定的判定，
> 给它一个 `Open` 开关等于"拿不准时允许逃出沙箱"。

---

## 6. 错误码

### 6.1 JSON-RPC 标准段

| 码 | 名称 | 含义 |
|---|---|---|
| `-32700` | ParseError | 帧不是合法 JSON |
| `-32600` | InvalidRequest | 结构不合法（缺 `method` 等） |
| `-32601` | MethodNotFound | 方法未注册（**或未实现**——见 §6.4） |
| `-32602` | InvalidParams | 参数校验失败 |
| `-32603` | InternalError | 宿主内部错误 |

### 6.2 应用段（`-32000..-32099`）

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

### 6.3 HTTP 状态映射

| JSON-RPC | HTTP |
|---|---|
| `-32600` / `-32602` | 400 |
| `-32001` | 401 |
| `-32002` | 404 |
| `-32003` | 503 |
| `-32004` / `-32007` | 403 |
| `-32005` | 504 |
| `-32006` / `-32009` / `-32010` / `-32011` | 409 |
| `-32008` | 413 |
| `-32012` | 422 |
| 其他 | 500 / 502 / 503 |

### 6.4 `MethodNotFound` 的两种来路

一是客户端拼错了方法名；二是方法**在 `ALL_METHODS` 里但没有 dispatch 臂**（孤儿方法）。
后者由 `verify-wiring` 静态守着（已清零），因此运行期几乎只可能是第一种。

---

## 7. 客户端用法

常量与 DTO 在 `crates/agent-proto/client-ts/`（**手写**，三处副本由 `cargo test -p agent-proto` 守着）：

```ts
import { THREAD_SEND, CONFIG_GET } from './methods'

const res = await client.request(THREAD_SEND, { threadId: 't1', prompt: '你好' })
client.on('evt.state.snapshot', (params) => { /* seq 对账 */ })
```

客户端实现有两条现成参考，都在仓库里（**没有**第三套）：

- **SSR 页面**（`crates/agent-web`）：走 SSE + 表单动作，实时通道的契约见
  `crates/agent-web/static/chat.js`（EventSource 建流 + `/live` 轮询兜底，
  `verify-wiring` 有结构断言）；
- **宿主内的程序化客户端**：`crates/agent-rpc` 的桥接层（`agent-web` 的 `AgentClient` 就是这条）。

原 React 桌面端的 `ws-client.ts` / `reconnect-policy.ts` 已随 `desktop-ui/` 于 2026-10 移除；
要从历史看那套 WS 客户端，用 git 历史取回。

---

## 8. 刻意还没做成 HTTP 的（Phase 3）

- `thread.send` / `retry` / `editAndResend` / `abort` / `compact`：需要 `202 + turnId/jobId`
  与事件流配套，不能做成同步 HTTP。
- `plugin.load` / `reload` / `validate`：可能跑几十秒，需要任务化。
- `provider.fetchModels` / `config.checkProvider`：外网探测，需要超时/任务化。
- `session.initialize` 与 `ui.*`：连接/客户端态，继续留在 WS 或客户端本地。
- 事件：`evt.*` 继续走 WS；HTTP 不做轮询。

---

## 9. 已知取舍

- 当前每个 HTTP 请求开一条短 WS 到 agent（带 `?api=1`），没有连接池；延迟主要是本地握手，
  够用。后续可换成按实例复用的多路复用连接。
- 写接口暂未做 `Idempotency-Key` / `If-Match`；并发写是 last-write-wins。
  多客户端场景应补版本号与幂等。
- `api=1` 连接**不注册进 agent 的事件广播**、不推种子快照、成功请求后不自动广播；
  写操作仍由 dispatch 臂广播给 UI 的 WS 连接。守门用例：`crates/agent-rpc/tests/ws_api_mode.rs`。

---

## 10. 真源与相关文档

| 位置 | 内容 |
|---|---|
| [`spec/proto/base.json`](../../spec/proto/base.json) | L0 方法 + L1 能力位/错误码（**真源**） |
| [`spec/proto/ada-coding.ext.json`](../../spec/proto/ada-coding.ext.json) | L2 产品扩展方法（**真源**） |
| [`spec/proto/README.md`](../../spec/proto/README.md) | 登记表规矩、命名、四处同步与副本校验 |
| `crates/agent-proto/src/methods.rs` | `ALL_METHODS`（77 个）、`ALL_EVENTS`（3 个）、`PROTOCOL_VERSION` |
| `crates/agent-gateway/src/http_api.rs` | HTTP 端点实现（路由组装在 `router.rs`） |
| [../architecture/conventions.md](../architecture/conventions.md) §16 | 加/删协议方法要同步四处（有守门测试） |
| [../architecture/conventions.md](../architecture/conventions.md) §18 | 工具结果是**结构化回执**（界面耗时与状态徽章依赖它） |
| [../architecture/kernel-design.md](../architecture/kernel-design.md) §6 / §7 | 协议单源、运行时装配与传输形态的设计决策 |
| [../gateway/README.md](../gateway/README.md) | 网关的设计意图与取舍（工作区门户、极限瘦客户端、路径安全） |

**层与规模**：

| 层 | 内容 | 登记处 |
|---|---|---|
| **L0** | 核心通信与调度：`session.*`、`thread.*`、`approval.*`、`question.*`、`queue.*`、`ui.*`、`evt.*` | `spec/proto/base.json` |
| **L1** | 系统能力位与标准错误码 | `spec/proto/base.json`（`capabilities` / `errorCodes`） |
| **L2** | 产品扩展：工作区、配置、供应商、插件、技能、提示词、子智能体、Git/FS、检查点、调试 | `spec/proto/<product>.ext.json` |

L2 不得重新定义 L0/L1 的方法——`cargo xtask compat` 会断言，并要求同名方法的 `kind` 一致。
当前规模：方法 **77** 个（base 23 + `ada-coding.ext.json` 54，`ALL_METHODS` 有断言守着）。
