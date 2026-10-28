# 前端插件的注入数据（点位数据结构）

> 适用版本：`ssr-h` v0.10+（`ModelPicker` / `data.usage` 扩展之后）。
> 这份文档是**插件作者与宿主之间的数据契约**：字段名、类型、可用性与刷新时机。
> 组件与插槽机制见 [TSX.md](TSX.md) / [COMPONENTS.md](COMPONENTS.md)。

## 1. 一次渲染拿到什么

服务端在**每个插槽 / 页面 / 仪表盘卡片**渲染时构造一份 `props`，TSX 是纯函数：

```jsonc
{
  "plugin": { "id": "token-weather", "name": "Token 天气", "version": "0.1.0", "description": "…" },
  "page":   { "slug": "weather", "title": "Token 天气" },        // 仅插件独立页面
  "slot":   { "name": "composer.above", "order": 10, "index": 0 }, // 仅插槽
  "app":    { "workspace": "E:/proj", "gateway": "http://127.0.0.1:52353", "product": "ssr-h · agent frontend", "version": "0.1.0" },
  "route":  { "path": "chat", "query": "" },                      // 插件页面会被覆盖成 /ext/{id}/{slug}
  "data":   { /* 见 §2 */ }
}
```

- 每次渲染都重新构造：**模块级变量不要缓存 props**，下一次 `parts` 刷新就会换掉；
- 拿不到数据时字段是 `null`，不是 `0` / `""`：`percent === null` 表示"算不出来"，
  不是"0%"（`0%` 会被误读成上下文很空）；
- 数据结构只做**加法**：新增字段会写在这里；已有字段语义不变。

## 2. `data` 字段

### 2.1 总览

| 字段 | 类型 | 含义 |
|---|---|---|
| `plugin_count` | number | 当前注册的前端插件数 |
| `render.at` | number | 本次渲染时刻（epoch ms），做"几秒前"用 |
| `tokens` | object | 当前会话的用量（§2.2；没有快照时为"全 0 + available=false"） |
| `thread` | object \| null | 当前会话摘要（§2.3） |
| `model` | object | 当前模型与供应商（§2.4，**不含密钥**） |
| `config` | object | 运行配置（§2.5） |
| `workspace` | object | 工作区（§2.6） |
| `counts` | object | 计数（§2.7） |
| `todo` | object \| null | `todo` 工具的当前计划（§2.8） |
| `questions` | ~~array~~ 已删除：提问由 `plugins/ask-user` 的 `backend["ask-user"]["list"]` 提供 |
| `providerAdmin` | object | 供应商只读投影（§2.9；仅声明 `providers.read` 的插件） |
| `processes` | object \| null | 网关与 agent 实例的只读投影（§2.11；仅声明 `processes.read` 的插件） |
| `backend` | object | 统一插件只读动作的预取结果（§2.12；`data.backend.<pluginId>.<binding>`） |
| `tasks` | object | 当前会话的后台命令任务（§2.14；仅声明 `tasks.read` 的插件） |
| `runs` | object | 当前会话最近的前台命令（§2.15；仅声明 `runs.read` 的插件） |

### 2.2 `tokens`（上下文 / 缓存用量）

| 字段 | 类型 | 来源 / 口径 |
|---|---|---|
| `available` | bool | 是否找到"最近一次带用量的助手回复" |
| `used` | number | 最近一次 `usage.promptTokens`（当前上下文规模，不是累计消费） |
| `window` | number | `config.contextWindow`（0 = 未配置） |
| `percent` | number \| null | `used ÷ window`，四舍五入整数百分比；窗口为 0 时 `null` |
| `cached` | number | `usage.cachedTokens`（见下方"来源"） |
| `cachePercent` | number \| null | `cached ÷ 该次调用的 promptTokens`（整数，四舍五入） |
| `cacheRate` | number \| null | 同上，但保留一位小数（`99.8`）；展示优先用它 |
| `cacheStale` | bool | `true` = 本轮调用未报缓存，数字来自**更早一次**上报过缓存的调用 |
| `completion` | number | `usage.completionTokens` |
| `thinking` | number | `usage.thinkingTokens`（推理 token；没报就是 0） |
| `total` | number | `usage.totalTokens`（缺失时用 `used + completion`） |
| `tokensPerSec` | number \| null | `completion ÷ (durationMs/1000)`；耗时 < 200ms 或没有输出时为 `null` |
| `durationMs` | number | 最近一次模型调用的耗时 |
| `threadId` | string | 这条用量属于哪条会话 |
| `threadTitle` | string | 与侧栏同口径的显示名（"新会话"用首条用户消息兜底） |
| `model` | string | 当前模型 id（兼容字段；完整信息看 `data.model`） |

> 这些数**全部来自模型自己报回的 usage**，服务端不做估算。插件不要把 `0` 当成
> "没有数据"：看 `available` / `percent === null`。
>
> 缓存字段不是每次调用都有：`cached` / `cachePercent` 优先取本轮；本轮没报时
> 回落到最近一次上报过 `cachedTokens` 的调用，并置 `cacheStale = true`。
> 展示时请标注来源（例如在百分比后加 `*`，tooltip 写"来自上一次上报"），
> 不要把它当成本轮命中率。

### 2.3 `thread`

```json
{ "id": "thread_…", "title": "提交代码", "workspace": "E:/proj", "mode": "code",
  "agentId": "ada-coding", "createdAt": 1761000000000,
  "running": true, "waiting": false, "isSubagent": false }
```

没有活动会话时是 `null`。`mode` 为 `code` / `pm`；`running`/`waiting` 来自快照的
`runningThreadIds` / `waitingThreadIds`。

### 2.4 `model`

```json
{ "id": "senseaudio/deepseek-v4.1-flash", "name": "DeepSeek V4.1 Flash",
  "providerId": "senseaudio", "providerName": "SenseAudio", "protocol": "openai_chat" }
```

- `name` 取供应商模型表里的显示名；找不到时回落成 `id`；
- 🔴 **永远不包含 `apiKey` / 自定义请求头**：原始 `providers[]` 不进插件上下文。

### 2.5 `config`

```json
{ "effort": "high", "mode": "code", "contextWindow": 1000000,
  "maxOutputTokens": 64000, "supportsImages": true, "retryCount": 5 }
```

- `effort`: `low` / `medium` / `high` / `max`（推理等级）
- `maxOutputTokens` 可能是 `null`；`contextWindow` 为 0 表示未配置。

### 2.6 `workspace`

| 字段 | 含义 |
|---|---|
| `project` | agent 的当前项目（快照 `workspace.project`） |
| `current` | 当前会话所属目录（会话没绑定时回落 `project`） |
| `publicWorkspace` | 对外可见工作区（`~/.a-da/workspace` 一类的公共目录） |

### 2.7 `counts`

| 字段 | 含义 |
|---|---|
| `threads` | 快照里的会话总数 |
| `running` / `waiting` | 运行中 / 等待用户答复的会话数（全工作区） |
| `queue` | **当前会话**的排队消息数 |

### 2.8 `todo`

```json
{ "done": 2, "total": 3, "allDone": false, "active": "写条目",
  "notes": "可选备注", "markdown": "### 任务规划步骤 (2/3)\n- [x] …",
  "steps": [{ "id": "t1", "title": "读列表", "status": "completed" }] }
```

`status`：`pending` / `in_progress` / `completed`。没有规划过时为 `null`。
状态推演（`set` 替换 / `update` 按 id 合并 / `get` 只读）只在核心做，插件不要重演。

### 2.9 `providerAdmin`

仅当插件 manifest 声明了 `providers.read` 时注入；页面路径仍由宿主的
`/ext/{id}/{slug}` 提供。API Key **永远不出现在这里**，只有掩码提示。

```json
{
  "providers": [{
    "id": "prov_…", "name": "commandcode_chat", "protocol": "openai_chat",
    "protocolLabel": "OpenAI Chat", "baseUrl": "https://…", "proxyUrl": "http://…",
    "headers": "x-api-key: …", "apiKeySet": true, "apiKeyHint": "sk-t…7890",
    "modelCount": 3, "active": true,
    "models": [{ "index": 0, "id": "…", "name": "…", "contextWindow": "1000000",
                 "maxOutputTokens": "65536", "supportsImages": true, "current": false }]
  }],
  "selectedId": "prov_…", "activeTab": "models",
  "currentModel": "…", "currentBaseUrl": "…",
  "canDeleteProviders": true,
  "protocols": [{ "value": "openai_chat", "label": "OpenAI Chat" }]
}
```

- `headers` 是**自定义请求头原文**：它可能含额外凭据，`providers.read` 应只授予可信插件；
- 写操作不在 TSX 里执行：表单提交到核心 `/providers/*`，成功后宿主按 `Referer`
  把重定向改回插件页（`?provider=…&tab=…&notice=…`）；
- API Key 输入框提交后直接到核心，不经过 Boa；插件只负责渲染“已配置 + 掩码”。
### 2.10 `backend["ask-user"]["list"]`（待答复的提问）

提问已完全由 `plugins/ask-user` 提供：核心不再投影 `data.questions`。
前端读取的是该插件只读动作 `list` 的预取结果：

```jsonc
{
  "ok": true,
  "data": {
    "questions": [{
      "id": "q_…",                 // 插件生成的提问 id
      "threadId": "thread-…",
      "createdAt": 1761000000000,
      "items": [{
        "id": "q1",
        "question": "用哪个环境？",
        "allowText": true,
        "multiSelect": false,
        "choices": [{ "id": "c1", "label": "开发环境", "description": "…" }]
      }]
    }]
  },
  "error": null,
  "fetchedAt": 1761000001000
}
```

- 作答表单提交到插件自己的桥接路由 `POST /ext/ask-user/backend/answer`，
  字段是 `question_id` / `thread_id` / `choice__{id}` / `choices__{id}` / `text__{id}`；
- 后端 `answer` 动作写回 `.ada/ask-user/questions.json`，正在等待的 `ask_user` 工具
  随即被唤醒；下一次 `parts` 帧里 `list` 不再返回该提问；
- 这条数据来自插件动作，不是核心 `plugin_data` 字段；插件停用则整条链消失。

### 2.10 `api`（插件 API 端点的请求面）

`plugin.json` 声明 `api` 后，端点在 `/ext/{id}/api/{path}`，由 TSX 入口函数处理——
**返回什么就序列化什么**（不走 JSX runtime；写 JSX 得到的是 VNode 对象）。

```jsonc
{
  "method": "get",              // get / post（白名单）
  "path": "info",               // 端点路径（静态段）
  "query": { "from": "browser" },  // 查询参数（键值对）
  "body": { "text": "…" }          // POST 的 JSON 体；GET 为 null
}
```

- 请求体必须是 JSON 且 ≤256KB，否则 400/413；响应 `no-store`；
- `props.data` 只有 `plugin_count`——API 可能被高频调用，**不拖会话快照**；
  需要会话数据的端点让调用方带 id 走核心路由；
- 端点查找走注册表（热）：`/extensions` 重载后新端点立即生效；插件停用/加载失败
  端点立即消失（404 JSON）；
- 插件浏览器 JS 可以 fetch 这些同源端点（CSP `connect-src 'self'`），
  完整示例见 `plugins/hello-tsx/`（`api/info.tsx` + `static/hello.js`）。

### 2.11 `processes`（网关与 agent 实例）

仅当插件 manifest 声明 `capabilities: ["processes.read"]` 时注入。数据来自网关自己的
管理面 `gateway.status` + `gateway.listAgents`；**端点已经剥掉 token**，不含任何凭据。

```jsonc
{
  "status": {                       // 网关状态；读取失败时为 null
    "pid": 1234,
    "product": "ada-coding",
    "defaultWorkspace": "E:/proj",
    "agentCount": 2,
    "routableCount": 2,
    "methodPrefix": "gateway."
  },
  "agents": [{                      // 网关管理的 agent 实例（已 redacted）
    "id": "ada-coding::…",
    "product": "ada-coding",
    "workspace": "E:/proj",
    "endpoint": "ws://127.0.0.1:5555/rpc",
    "status": "ready",              // ready / starting / unhealthy / stopped
    "pid": 777,
    "startedAt": 1761000000000,
    "lastActiveMs": 1761000000000,
    "products": ["ada-coding"]
  }],
  "count": 1,
  "error": null                     // 读取失败时的原因（status 为 null、agents 为空数组）
}
```

- 进程数据在**页面渲染时**抓取；插件里的「刷新」用整页重载重新取（插件没有网络能力，
  不需要自己发请求）；
- 只读：不提供终止/重启入口——SSR 页面就跑在网关进程里；
- 相对时间用 `data.render.at`，不要信浏览器时钟。

### 2.12 `backend`（统一插件动作预取）

仅统一插件包（`plugin.json` 同时有 `frontend` / `backend`）会出现。宿主只预取
`frontend.actions` 中 `read: true` 的绑定：

```jsonc
{
  "backend": {
    "unified-demo": {
      "status": {
        "ok": true,
        "data": { "count": 0, "lastRefresh": null },
        "error": null,
        "fetchedAt": 1761000000000
      }
    }
  }
}
```

- 键是**前端插件 id** 与 **binding 名**；`data` 是后端动作返回的 `data`；
- 失败时 `ok:false`、`data:null`、`error` 是可读原因；页面不要因此 500；
- 写入动作（`read:false`）不会出现在这里，必须由表单/脚本 `POST`；
- 预取每个 binding 独立超时 6 秒，最多 16 个；不要在 `data.backend` 里期待写入结果。

### 2.13 `approvals`（已删除）

审批已随核心物理删除（2026-10-27）：`data.approvals`、`thread.approval` 插槽、
`/approvals/{call_id}/decision` 路由与 `plugins/approval` 均不再存在。
历史会话里的 `waiting_approval` 卡片按已结束记录展示。

### 2.14 `tasks`（当前会话的后台命令任务）

仅当插件 manifest 声明 `capabilities: ["tasks.read"]` 时注入。数据来自 agent 进程里
`run_background` 族共享的 `BackgroundTasks` 实例（`task.list`，按当前 `threadId` 过滤）：

```jsonc
{
  "tasks": [{
    "id": "task_1",
    "command": "npm run dev",
    "pid": 4321,
    "startedAt": 1761000000000,
    "threadId": "thread_…",
    "running": true,
    "exitCode": null,
    "outputPreview": "VITE ready in 300ms"
  }],
  "count": 1,
  "error": null
}
```

- 任务表是**实例态**（进程重启后为空），不落盘；
- `outputPreview` 截断到 4000 字符；完整输出用 `check_task`；
- 终止走核心路由 `POST /tasks/{id}/kill`（内部 `task.kill`），插件只渲染按钮；
- 没有 `tasks.read` 的插件拿不到这份数据。

**这份数据是"推"过来的，不是插件自己拉的**：任务表在 agent 进程里、网关不广播它，
所以聊天页的 SSE 事件循环按 1 秒问一次 `task.list`，用指纹（`id` / `running` /
`exitCode` / 输出长度）判断变化，变了就补一帧 `parts`。因此：

- 有任务在跑时，浮窗里的输出大约 1 秒长一截；没有任务在跑时不轮询（表不会自己变），
  结构变化（新卡片 / 条目重取）后强制重拉一次；
- 同一帧里已经取到的任务表会直接交给插槽渲染，**一帧只打一次 `task.list`**；
- 2 秒兜底轮询（SSE 断流时）与首屏渲染仍各自取一次，语义与其它投影一致。

### 2.15 `runs`（当前会话最近的前台命令）

仅当插件 manifest 声明 `capabilities: ["runs.read"]` 时注入。

**数据源不是任务表，是会话快照里的 tool item。** `run_command` 是阻塞调用：它结束后
不留任何活体（不在 `BackgroundTasks` 里、没有可终止的进程），唯一权威记录就是对话流里
那条「执行命令」卡片本身。所以这里**只做投影**，不新增 RPC、不缓存第二份状态。

```jsonc
{
  "runs": [{
    "id": "item_…",            // 会话 item id（不是 task id，不能拿去 kill）
    "callId": "call_…",
    "command": "ping -n 6 127.0.0.1",
    "cwd": "",                 // 未显式指定 cwd 时为空串（= 引擎工作区）
    "status": "done",          // done / error / timeout / aborted / running …
    "running": false,
    "exitCode": 0,             // 只认 details.exitCode / details.exit_code，缺失为 null
    "durationMs": 5100,
    "startedAt": 1760999994000,
    "at": 9,
    "outputLines": 13,
    "outputPreview": "来自 127.0.0.1 的回复: 字节=32 时间<1ms…"
  }],
  "count": 1,                  // 本帧给出的条数（≤ 20）
  "total": 1                   // 会话里一共有多少条（可能大于 count）
}
```

- **只投影 `run_command`**：`run_background` / `check_task` / `kill_task` 属于 `tasks`（§2.14）；
- 最新的排最前；最多回看 20 条（面板是"最近命令"，不是完整历史），超出部分用 `total` 如实报数；
- `outputPreview` 截断到 1200 字符（比后台任务的 4000 小：一屏要挤 20 条）；
- 历史被 `compact` 掉的老 item 不再出现——这正是"最近"的语义，不是缺陷；
- 没有会话 / 还没跑过命令 → `{ runs: [], count: 0 }`，**这不是读取失败**（没有 `error` 字段）；
- 只读：这些记录**不可终止**（进程早已结束），插件不要给它们渲染 kill 表单。

## 3. 点位与刷新时机

| 插槽 | 出现在 | 内容刷新 |
|---|---|---|
| `sidebar.top` / `sidebar.bottom` | 所有页面（侧栏） | 整页加载 / 跳转 |
| `topbar.right` | 聊天页与二级页顶栏 | 整页加载（聊天页另有状态药丸 OOB） |
| `chat.corner` | **仅聊天页**，会话区右上角 | 首屏 + SSE `parts.corner` / 2s live 轮询 |
| `composer.left` / `composer.right` | 聊天页输入框工具行 | 整页加载（`live` 会整块换输入框辅助区，但工具行不换） |
| `composer.footer` | 聊天页输入框下方辅助区 | 首屏 + `parts.aux` / 2s live 轮询 |
| `composer.above` | 聊天页右下角悬浮区 | 首屏 + `parts.above` / 2s live 轮询；有后台任务在跑时，输出变化会额外触发 `parts`（约 1 秒一帧） |
| `thread.tray` | 聊天页托盘区（输入框上方 in-flow）的**提问段** | 首屏 + `parts.trays` / 2s live 轮询；提问由 `plugins/ask-user` 自己渲染与作答，核心不再兜底 |
| `transcript.after` | 聊天页对话流末尾 | 首屏（整段同步时随对话流重渲染） |
| `extensions.list` | `/extensions` | 整页加载 |
| `dashboard.widgets` | `/overview` | 整页加载 / 5s 片段刷新（如插件自己声明） |

`data` 的可用性取决于**渲染点有没有快照**：

| 渲染点 | `data` |
|---|---|
| 聊天页首屏 | 完整（当前会话，显式 thread） |
| 聊天 `parts` / `/live` | 完整（当前会话，显式 thread；items 是投影后的工具卡 + 助手 usage，够算 `tokens` / `todo` / `counts.approvals`） |
| 插件独立页 `/ext/{id}/{slug}` | 完整（快照的 `activeThreadId`） |
| 概览卡片 `dashboard.widgets` | 完整（快照的 `activeThreadId`） |
| `/workspaces`（有快照） | 完整（快照的 `activeThreadId`） |
| `/extensions`、错误页 | **只有 `plugin_count`**（`tokens = null` 语义：字段缺省，不要假设结构） |

> 判断"有没有数据"用字段本身（`props.data.tokens ? … : …`），不要按路由猜。

## 4. 写插件的几条硬规矩

1. **只读**：`props` 是数据不是句柄；没有事件、没有 `fetch`、没有 `window`。
   要交互就用表单/Htmx（`POST` 到自己的路由）或 `<details>` 渐进增强。
2. **数字自己格式化**：核心给原始数值；展示层统一用 `formatTokens`
   （≥1M → `1.2M`、≥1K → `18.7K`、其余整数）之类的一处实现，别每个卡片各写一份。
3. **空值语义**：`null` = 算不出来；`0` = 真的是 0。别把 `null` 渲染成 `0%`。
4. **密钥红线**：`data` 里没有 apiKey；插件也不要试图从别处读配置。
5. **体积**：`data` 会随每次 `parts`（约 500ms）重新序列化；不要在模板里
   `JSON.stringify(props)` 输出整份数据。
6. **可降级**：任何字段缺失都要能渲染（老网关 / 断线时 `tokens` 会是 0 值）。

## 5. 示例：天气插件怎么用

```tsx
const t = props.data.tokens;
const cache = typeof t?.cachePercent === "number" ? t.cachePercent : null;
const used = t ? t.used : 0;
const windowSize = t ? t.window : 0;

<p>缓存命中 {cache === null ? "—" : cache + "%"}</p>
<p>{formatTokens(used)} / {formatTokens(windowSize)} tokens</p>
```

完整实现见 [`plugins/token-weather/`](../plugins/token-weather/)，
数据投影的唯一出口在 `src/handlers/mod.rs::plugin_data`。


