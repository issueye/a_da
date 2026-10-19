# HTTP API（Phase 0 / 1 / 2）

> 状态：**已落地**。HTTP 只承载 **unary（一问一答）** 的方法；服务端事件
> （`evt.state.snapshot` / `evt.state.patch` / `evt.message.delta` / `evt.card.updated`）
> 仍然走原来的 JSON-RPC over WebSocket。实时界面/流式输出必须继续用 `/rpc`。

实现位置：`crates/agent-gateway/src/http_api.rs`；路由组装在
`crates/agent-gateway/src/router.rs`。HTTP 与 `/rpc`、前端静态资源共用同一个端口。

## 1. 鉴权与工作区

| 项 | 规则 |
|---|---|
| 凭证 | `Authorization: Bearer <token>`；兼容旧的 `?token=` |
| 未认证 | 只允许 `GET /api/gateway/info`；其余返回 `401` |
| Origin | 有 `Origin` 就必须在 `--allow-origin` 白名单（或 localhost/127.0.0.1）；否则 `403` |
| 工作区 | `?workspace=<abs path>` 或 `X-A-Da-Workspace`，缺省用网关默认工作区 |
| 作用域 | token 的 `--token token=ws1\|ws2` 作用域会同时约束 query/body 里的 workspace。**受限 token 请把工作区放 query/header**（body 里的 workspace 在 token 鉴权之后才检查，只写 body 会先按默认工作区被拒） |
| CORS | 允许的 Origin 会回 `Access-Control-Allow-*`；`OPTIONS` 预检返回 `204` |

## 2. 请求与响应

- 成功：HTTP 200，body 是该 JSON-RPC 方法的 `result`；`result` 为 `null` 时返回 `{"ok":true}`。
- 失败：`{"error":{"code":<JSON-RPC code>,"message":"...","data":...}}`，HTTP 状态按协议码映射：

| JSON-RPC | HTTP |
|---|---|
| `-32600/-32602` | 400 |
| `-32001` | 401 |
| `-32002` | 404 |
| `-32003` | 503 |
| `-32004/-32007` | 403 |
| `-32005` | 504 |
| `-32006/-32009/-32010/-32011` | 409 |
| `-32008` | 413 |
| `-32012` | 422 |
| 其他 | 500 / 502 / 503 |

- 写接口的 body 基本就是该 JSON-RPC 方法的 `params`。资源 id 走路径段；
  工作区/文件路径走 query（Windows 路径里有 `\`，不适合做路径段）。
- `GET /api/snapshot` 返回 `ETag: "snap-<seq>"`；带同值 `If-None-Match` 得到 `304`。

## 3. Phase 1：纯查询

| 方法 | 路径 | 对应 RPC |
|---|---|---|
| GET | `/api/snapshot` | `session.snapshot`（ETag） |
| GET | `/api/threads/{threadId}/items?from=&count=` | `thread.items.page` |
| GET | `/api/workspaces/sessions?workspace=` | `workspace.entries` |
| GET | `/api/config` | `config.get` |
| GET | `/api/config/presets` | `config.presets` |
| GET | `/api/providers` | `provider.list` |
| GET | `/api/fs/roots` | `fs.roots` |
| GET | `/api/fs/list?path=&showHidden=&limit=` | `fs.list` |
| GET | `/api/fs/raw?path=` | `fs.read_base64`（当前返回 `{dataUri,path}` JSON） |
| GET | `/api/threads/{threadId}/changes/count` | `change.count` |
| GET | `/api/threads/{threadId}/changes` | `change.list` |
| GET | `/api/plugins?workspace=` | `plugin.list` |
| GET | `/api/plugins/builtin` | `plugin.builtinCatalog` |
| GET | `/api/prompts?workspace=` | `prompt.list` |
| GET | `/api/skills?workspace=` | `skill.list` |
| GET | `/api/subagent-profiles?workspace=` | `subagentProfile.list` |
| GET | `/api/debug/host` | `debug.hostInfo` |
| POST | `/api/stats/prompt-chars` | `stats.promptChars`（纯计算） |

## 4. Phase 2：unary 写

| 方法 | 路径 | 对应 RPC |
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

## 5. 网关管理面

| 方法 | 路径 | 对应 WS 方法 |
|---|---|---|
| GET | `/api/gateway/info` | `gateway.info`（公开） |
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

## 6. 示例

```bash
# 发现（无需 token）
curl http://127.0.0.1:52353/api/gateway/info

# 读配置
curl -H 'Authorization: Bearer secret' \
     'http://127.0.0.1:52353/api/config?workspace=E:/proj'

# 改推演力度
curl -X PUT -H 'Authorization: Bearer secret' -H 'Content-Type: application/json' \
     -d '{"effort":"high"}' \
     'http://127.0.0.1:52353/api/config/effort?workspace=E:/proj'

# 快照 ETag
curl -i -H 'Authorization: Bearer secret' \
     'http://127.0.0.1:52353/api/snapshot?workspace=E:/proj'
# -> ETag: "snap-42"
curl -i -H 'Authorization: Bearer secret' -H 'If-None-Match: "snap-42"' \
     'http://127.0.0.1:52353/api/snapshot?workspace=E:/proj'
# -> 304 Not Modified
```

## 7. 刻意还没做的（Phase 3）

- `thread.send` / `retry` / `editAndResend` / `abort` / `compact`：需要 `202 + turnId/jobId`
  与事件流配套，不能做成同步 HTTP。
- `plugin.load` / `reload` / `validate`：可能跑几十秒，需要任务化。
- `provider.fetchModels` / `config.checkProvider`：外网探测，需要超时/任务化。
- `session.initialize` 与 `ui.*`：连接/客户端态，继续留在 WS 或客户端本地。
- 事件：`evt.*` 继续走 WS；HTTP 不做轮询。

## 8. 已知取舍

- 当前每个 HTTP 请求开一条短 WS 到 agent（带 `?api=1`），没有连接池；延迟主要是
  本地握手，Phase 0/1/2 够用。后续可换成按实例复用的多路复用连接。
- Phase 2 写接口暂未做 `Idempotency-Key` / `If-Match`；并发写是 last-write-wins。
  多客户端场景应在 Phase 3 一并补版本号与幂等。
- `api=1` 连接不注册进 agent 的事件广播、不推种子快照、成功请求后不自动广播；
  写操作仍由 dispatch 臂广播给 UI 的 WS 连接。守门用例：
  `crates/agent-rpc/tests/ws_api_mode.rs`。
