# 前端插件 ↔ 后端插件交互方案

> **状态：v1 已部分落地（2026-10-26）。** 已实现：`registerAction`、`plugin.invoke`、
> 统一插件包（一个 `plugin.json` 同时声明 `frontend` / `backend`）、SSR 只读预取与
> `GET/POST /ext/{id}/backend/{binding}` 桥接。**当前实际契约以
> [unified-package.md](unified-package.md) 为准**；本文保留完整设计与批次记录。
> 尚未实现：多后端分组、`confirm` 的核心强制、`plugin.list` 的动作计数、事件推送。
> 旧版分离插件仍没有直连通道。

---

## 1. 一句话结论

给**后端插件**增加一种新的声明式贡献 `registerAction`（只给界面用的只读/写入动作），
新增 JSON-RPC 方法 `plugin.invoke`；**前端插件**在 `plugin.json` 里声明要绑定的后端动作，
由 SSR 宿主在**服务端**调用，前端只拿到渲染好的数据或同源 JSON 回执。

浏览器**不接触网关 token，也不直接连 `/rpc`**。前端插件的按钮走一条核心桥接路由：

```
POST /ext/{frontend_id}/backend/{group}/{binding}
```

---

## 2. 为什么不能直接复用现有两条路

| 现成能力 | 为什么不适合做界面动作 |
|---|---|
| 后端插件的**工具**（`registerTool`） | 工具是**给模型**的：会进工具目录、参与审批与检查点、失败会进模型上下文。把 UI 按钮伪装成工具调用，等于绕开"批准的是模型要执行什么"这条语义；而且界面动作不需要模型看见。 |
| 后端插件的**钩子**（`ada.on`） | 钩子是事件驱动的旁路，**没有请求-响应返回值**，也不能由某次点击定向触发；它回答的是"某件事发生时要不要干预"，不是"帮我把这份数据取来"。 |
| 前端插件自带的 **JSON API**（`/ext/{id}/api/...`） | 处理器跑在 SSR 进程的 Boa 里，没有 `fetch`、没有 agent 句柄、拿不到插件配置；让它自己连 `/rpc` 就必须把 token 带进浏览器或服务端，前者泄露凭据，后者把两个进程的生命周期绑死。 |
| 直接让浏览器连 `/rpc` | 网关 token 会出现在页面 JS / URL 里；浏览器一旦持有 token，就能调用**所有**方法，不只是某个动作。这与"SSR 页面只拿到投影"的现有边界冲突。 |
| 让前端 TSX `import` 后端插件代码 | 两套运行时（SSR 的 Boa vs agent 的 Boa）与两套权限模型；前端插件目录通常随包只读，后端插件在工作区/用户目录，路径与信任级别都不同。 |

结论：必须新增一个**窄的、声明式的、服务端中转的**动作面。它不是第二个工具系统，
也不是通用 RPC 代理，只做"前端插件声明的某个绑定 → 后端插件声明的某个动作"。

---

## 3. 目标与非目标

### 目标

1. 前端插件能**确定性地**调用后端插件：点按钮、提交表单、轮询状态，都有明确回执。
2. 后端插件主动声明自己暴露哪些界面动作，以及每个动作是**只读**还是**写入**。
3. SSR 首屏可以**预取**只读动作，避免页面先空白再由浏览器补一刀。
4. 复用现有登录态、Origin/CSRF 防护、插件启停与重载语义；不新增第二套鉴权。
5. 动作运行在**声明了 `@ada-needs` 的沙箱**里，与工具共享同一套 fs/net 边界。
6. 前端插件被停用、后端插件缺失/停用/损坏时，页面**降级而不是 500**。

### 非目标（v1 明确不做）

- 不做后端 → 前端的主动推送（WebSocket/SSE 事件）。后端动作是请求-响应；
  需要"实时"就用只读动作轮询，或等后续插件事件总线。
- 不允许前端动作调用模型、工具、钩子或会话 API；动作不是"第二个 agent 入口"。
- 不把动作自动暴露成模型工具；两套消费者（模型 / 界面）保持分离。
- 不让动作读取插件 `config` / `secret`；那是另一条需要单独设计的信任边界。
- 不做跨工作区调用：动作只在**当前 SSR 页面绑定的工作区 agent**里执行。
- 不支持内置插件的动作（v1 只支持磁盘插件：`workspace:*` / `global:*`）。

---

## 4. 总体形态

```
┌──────────────────────────── 浏览器（同源页面） ────────────────────────────┐
│  前端插件 TSX 渲染出的按钮 / 表单 / 轮询脚本                                 │
│      GET  /ext/{frontend}/backend/{group}/{binding}   （只读）              │
│      POST /ext/{frontend}/backend/{group}/{binding}   （写入）              │
└──────────────────────────────────┬────────────────────────────────────────┘
                                   │ 会话 cookie + Origin 校验
                                   ▼
┌──────────────────────────── agent-web（SSR 宿主） ─────────────────────────┐
│  1. 查 PluginRegistry：前端插件启用？binding 声明存在？                     │
│  2. 查 manifest：read=true 才允许 GET；写入必须有 confirm 文案              │
│  3. AgentClient.rpc("plugin.invoke", { pluginId, action, input, intent })   │
│  4. 首屏渲染时把只读结果注入 props.data.backend.{group}.{binding}           │
└──────────────────────────────────┬────────────────────────────────────────┘
                                   │ 进程内（网关）或 WS（独立运行）
                                   ▼
┌──────────────────────────── agent-rpc / agent-node ────────────────────────┐
│  plugin.invoke → PluginManager::invoke_action                              │
│    · 插件存在且已启用？action 已注册？mode 与 intent 相符？                 │
│    · 组装沙箱（插件路径 + workspace + @ada-needs + Limits）                │
│    · execute(input, ctx) → { ok, data } / 抛错                             │
└──────────────────────────────────┬────────────────────────────────────────┘
                                   ▼
                        后端插件 action（Boa 沙箱）
```

关键点：**前端 manifest 只声明"我想调哪个动作"，真正的权限判定在后端**。
前端把 `read` 写成 `true` 不能把写动作变成只读——后端按 action 自己的 `mode` 拒绝。

---

## 5. 后端插件契约

### 5.1 `ada.registerAction`

在工厂函数里注册，与 `registerTool` / `registerSystemPrompt` / `on` 同级：

```ts
ada.registerAction({
  name: "status",                    // 必填；同插件内唯一
  label: "读取状态",                  // 可选，给管理面板看
  description: "返回监控摘要",         // 必填，给管理面板/诊断看
  mode: "read",                      // read | write；缺省 write（失败安全）
  parameters: {                      // 可选；JSON Schema 子集，只用于校验 input
    type: "object",
    properties: {},
    additionalProperties: false,
  },
  async execute(input, ctx) {
    // input 是调用方给的 JSON；没给就是 {}
    // ctx: { workspace, caller: "web", now }，v1 只读
    return { cached: 12, healthy: true };
  },
});
```

`execute` 的返回值：

| 返回 | 解释 |
|---|---|
| 任意 JSON 值 | 宿主包成 `{ ok: true, data: <返回值> }` |
| `{ ok: false, error: "...", hint?: "..." }` | 动作**软失败**；HTTP/RPC 回执同样是失败，但不抛栈 |
| `throw` / Promise reject | 硬失败；宿主映射为 `plugin_action_failed`，文案带动作名 |

约定：

- **每次调用都是新沙箱**，模块状态不跨请求；动作不要依赖进程内缓存。
- 动作是**单次、无状态**的；需要跨调用状态请写工作区文件（受 fs 边界约束）。
- `execute` 不接收 `callId`，也不接收模型消息；它不是工具。
- 动作不能注册新工具/动作/钩子；调用路径里的这些入口是**空实现**，
  与现有 `call_tool` 路径的口径一致（工厂必须能跑完，但不能改变装配结果）。

### 5.2 metadata

`ada.metadata` 增加一个**可选**的 `actions` 组：

```ts
ada.metadata = {
  name: "system-monitor",
  description: "系统监控数据源",
  tools: { count: 0, names: [] },
  systemPrompt: { count: 0, names: [] },
  hooks: { count: 0, names: [] },
  actions: { count: 2, names: ["status", "refresh"] },
};
```

校验规则：

- `actions` 组**存在**时，`count` / `names` 必须与实际注册逐项、按顺序一致；
- 实际注册了动作但没写 `actions` 组 → 装载期拒绝（和 `tools` 不同步一样）；
- 没有任何动作的旧插件**不需要补字段**，保持向后兼容；
- 对象形态的旧插件不支持动作（与 `registerSkill` 的处置一致：不静默生效，
  给一条诊断说明"动作只支持工厂形态"）。

### 5.3 `plugin.invoke`

新增 JSON-RPC 方法：

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "method": "plugin.invoke",
  "params": {
    "pluginId": "global:system-monitor",
    "action": "status",
    "input": {},
    "intent": "read"
  }
}
```

`intent` 是调用方声明的意图：

- `read`：只允许 `mode == "read"` 的动作；SSR 首屏预取与 GET 桥接必须用它。
- `write`：允许只读与写入动作；POST 桥接用它。
- 缺省 `write`（失败安全：不声明就按可能有副作用处理）。

回执：

```json
{ "ok": true, "data": { "cached": 12 } }
```

失败走 JSON-RPC error，`data` 里带结构化原因：

```json
{
  "code": -32004,
  "message": "动作 refresh 声明为 write，不能用 read 意图调用",
  "data": {
    "kind": "plugin_action_denied",
    "pluginId": "global:system-monitor",
    "action": "refresh"
  }
}
```

错误映射（复用现有 `AppErrorCode`，不新增错误码）：

| 情况 | code | 桥接 HTTP |
|---|---|---|
| 插件不存在 / 动作不存在 | `-32002` NotFound | 404 |
| 插件未启用、损坏、未就绪 | `-32003` NotReady | 409 |
| `read` 意图调用 `write` 动作 | `-32004` Denied | 403 |
| 动作超时 | `-32005` Timeout | 504 |
| 输入/输出超过大小上限 | `-32008` TooLarge | 413 |
| 同插件并发动作已满 | `-32011` Busy | 429 |
| 动作抛错 | `-32603` InternalError | 502 |
| 插件装配失败（语法/元数据） | `-32003` NotReady | 409 |

### 5.4 管理面

`plugin.list` 的每个插件增加一个**只读**字段：

```json
"actions": [
  { "name": "status", "label": "读取状态", "description": "返回监控摘要", "mode": "read" }
]
```

`plugin.validate` 同样校验动作契约（名称、mode、metadata 对账）。
`plugin.reload` / `plugin.setEnabled` 之后动作表随贡献一起重装配；
下一次 `plugin.invoke` 看到的就是新表（不做进程内缓存）。

### 5.5 资源与并发

| 项 | 默认 | 说明 |
|---|---|---|
| `actionTimeoutMs` | 5000 | 单次动作墙钟上限；上限 30000；超时终止 VM |
| 输入大小 | 64 KB | JSON 序列化后；`plugin.invoke` 与桥接路由都校验 |
| 输出大小 | 256 KB | 与前端插件 API 的 POST 体上限同口径；超出即 `TooLarge` |
| 同插件并发 | 4 | 超出返回 `Busy`；避免一个页面轮询把 agent 拖死 |
| 指令/递归预算 | 与工具相同 | 复用 `ada-ts-engine::Limits`，不另设一套 |

`actionTimeoutMs` 放在 `PluginCapabilities` 里，由 `plugin.capabilities.set` 读写；
**必须有生产消费者**（`verify-wiring` 的 check L2 会核对），不能只落配置不生效。

---

## 6. 前端插件契约

### 6.1 `plugin.json` 增加 `backend`

```json
{
  "id": "system-panel",
  "name": "系统面板",
  "backend": {
    "monitor": {
      "plugin": "global:system-monitor",
      "actions": {
        "status": { "action": "status", "read": true },
        "refresh": { "action": "refresh", "confirm": "确定刷新监控缓存？" }
      }
    }
  },
  "slots": [
    { "slot": "topbar.right", "template": "slot.tsx" }
  ]
}
```

字段规则：

| 字段 | 规则 |
|---|---|
| `backend` | 可选；对象，键是 **group 名**（`[a-z0-9][a-z0-9-]{0,31}`，同插件内唯一） |
| `backend.*.plugin` | 必填；**完整插件 id**：`workspace:<目录名>` 或 `global:<目录名>`。不要写裸目录名——两个 scope 可能同名 |
| `backend.*.actions` | 必填；键是 **binding 名**（同上规则），值是动作声明 |
| `...action` | 必填；后端插件里 `registerAction` 的 `name` |
| `...read` | 可选，缺省 `false`；只有显式 `true` 才允许 SSR 预取与 GET |
| `...confirm` | 写入动作可选；给插件脚本/表单用的确认文案。**这是交互提示，不是权限边界** |

`backend` 是**显式授权**：只有这里列出的动作能被桥接路由调用。
不需要再声明 `capabilities: ["backend.invoke"]`——binding 本身就是最小授权，
再加一层只会让"到底哪份清单算数"变得含糊。

### 6.2 渲染数据

首屏渲染时，宿主对每个 `read: true` 的 binding 调用一次 `plugin.invoke`，
把结果注入 `props.data.backend`：

```json
{
  "backend": {
    "monitor": {
      "status": {
        "ok": true,
        "data": { "cached": 12, "healthy": true },
        "error": null,
        "fetchedAt": 1760000000000
      }
    }
  }
}
```

失败也**不抛页面**，而是把错误放进同一个位置：

```json
{
  "ok": false,
  "data": null,
  "error": "后端插件未启用",
  "errorKind": "plugin_not_ready",
  "fetchedAt": 1760000000000
}
```

前端插件的 TSX 据此渲染"读取失败"状态，而不是整页 500。
`read: false` 的 binding **不会**出现在 `props.data.backend` 里；它们只能由
浏览器显式 POST 触发，结果由插件脚本自己处理。

### 6.3 浏览器侧调用

只读轮询：

```js
const res = await fetch("/ext/system-panel/backend/monitor/status", {
  headers: { Accept: "application/json" },
});
const { ok, data, error } = await res.json();
```

写入：

```html
<form method="post"
      action="/ext/system-panel/backend/monitor/refresh"
      data-backend-action
      data-confirm="确定刷新监控缓存？">
  <button type="submit">刷新</button>
</form>
```

- 有 JS 时：插件脚本拦表单、弹确认、`fetch` 带 `Accept: application/json`，
  按 `{ ok, data, error }` 就地更新 DOM。
- 无 JS 时：POST 后 303 回原插件页，成功带 `notice=backend_action_ok`，
  失败带 `error=<文案>`；页面重新渲染时只读 binding 会预取到新状态。
  写入动作的返回值不保证在无 JS 场景可见——那是插件脚本的职责。
- **写入必须 POST**；GET 只接受 `read: true` 的 binding，否则 405。
- `confirm` 由插件自己呈现。核心不渲染插件专属确认框，也不把 `confirm`
  当作安全边界；真正的边界是登录态、同源校验、后端 action 的 `mode`。

### 6.4 前端插件 API 端点能不能调后端动作？

v1 **不提供** `props.backend.call(...)`。理由：

- 前端 TSX 的 API 处理器是服务端渲染路径，若允许任意调用动作，
  一个 GET 页面请求就可能触发写入；
- 预取只做 `read: true`，语义清晰；写入必须是一次明确的浏览器 POST。

如果未来确实需要"插件 API 组合后端数据"，再加一个**只读**的
`props.backend.read(group, binding)`，并强制 `intent: "read"`；
写入永远只走 POST 桥接。

---

## 7. 核心桥接路由

### 7.1 路由

在 `crates/agent-web/src/router.rs` 注册（放在 `/ext/{id}/{slug}` 之前）：

```text
GET  /ext/{frontend_id}/backend/{group}/{binding}   → 只读，JSON
POST /ext/{frontend_id}/backend/{group}/{binding}   → 读/写，JSON 或 303
```

处理放在新模块 `crates/agent-web/src/handlers/bridge.rs`，不塞进 `extensions.rs`
（后者已经有页面、API、启停三件事）。

### 7.2 请求处理顺序

1. **guard**：沿用现有 SSR 中间件——Host 白名单、会话 cookie、写请求 Origin /
   `Sec-Fetch-Site` 校验。桥接路由不新增鉴权分支。
2. **前端插件**：`state.frontend.find(frontend_id)`；未启用/未就绪 → 404。
3. **binding**：按 `group` / `binding` 查 manifest；不存在 → 404。
4. **方法**：
   - GET 且 `read != true` → 405；
   - POST 永远允许（动作自己的 `mode` 仍是最终判定）。
5. **入参**：
   - GET：query 只接受标量，转成 `input` 对象；
   - POST：`application/json` 或表单；JSON 体 ≤64 KB；
   - 写入动作若表单带 `confirm` 字段，原样传给动作（动作可以自行二次校验）。
6. **调用**：`state.agent.rpc("plugin.invoke", { pluginId, action, input, intent })`；
   `intent = GET ? "read" : "write"`。
7. **回执**：
   - `Accept: application/json` 或 `HX-Request` → JSON `{ ok, data }`；
   - 其它（普通表单导航）→ 303 回 `Referer`（必须是 `/ext/{frontend_id}/...`），
     成功 `?notice=backend_action_ok`，失败 `?error=<urlencoded>`；
   - `Referer` 不在本插件页内 → 303 到 `/ext/{frontend_id}/{第一个页面 slug}`，
     不跳任意外部地址。

### 7.3 首屏预取

在 `handlers/mod.rs` 的 `base_with_snapshot` 里，紧跟现有的 `processes.read`
注入之后，增加一段：

```rust
if state.frontend.has_backend_reads() {
    let backend = prefetch_backend_bindings(state).await;
    if let Some(data) = context.data.as_object_mut() {
        data.insert("backend".to_string(), backend);
    }
}
```

`prefetch_backend_bindings`：

- 只遍历**已启用**前端插件与 `read: true` 的 binding；
- 同一个 `(pluginId, action, input)` 在一次页面渲染里去重（两个前端插件绑同一动作只调一次）；
- 每个绑定独立超时（`actionTimeoutMs`），失败只影响自己；
- 页面渲染预算紧张时可以直接跳过预取（返回 `ok:false, errorKind:"prefetch_skipped"`），
  但 v1 先不做全局预算，按绑定超时兜底。

> 注意：预取会**启动/唤醒当前工作区的 agent**（网关的 `ensure_agent` 语义）。
> 这与打开会话一致；如果产品希望"只看页面不唤醒 agent"，应在
> `GatewayBackend::rpc` 之前加一个"已有实例才调用"的开关，而不是在桥接层特判。

---

## 8. 安全与生命周期

### 8.1 信任边界

| 边界 | 规则 |
|---|---|
| 浏览器 → SSR | 登录 cookie + Host/Origin/CSRF；与现有页面动作完全一致 |
| SSR → agent | 服务端 `AgentClient`；网关 token **不下发浏览器** |
| 前端插件 → 后端插件 | 只能调用 manifest 里显式列出的 binding；不能传任意插件 id / 动作名 |
| 后端插件 → 宿主 | 动作在 `@ada-needs` 沙箱里跑；fs 以当前工作区为根，net 先过域名白名单 |
| 动作 → 模型/工具 | **不连通**；动作不能触发轮次、不能注册工具、不能读写会话 |

### 8.2 只读与写入

- `mode` 由**后端插件**声明，是唯一权威；前端 manifest 的 `read` 只是"是否预取"的意图。
- 后端用 `intent` 做二次校验：`intent=read` 时拒绝 `mode=write`。
- 写入动作不进入审批闸门：审批闸门回答的是"模型要执行什么"，而这里用户已经点了按钮。
  但写入动作仍受登录态、同源校验和插件自身校验约束。
- 前端 `confirm` 只是提示；不要把它写成"安全确认"。

### 8.3 启停与重载

| 事件 | 行为 |
|---|---|
| 前端插件停用 | 桥接路由 404；该插件的插槽/页面消失；预取不再调用它 |
| 前端插件重载 | manifest 重新读取；binding 变化立即生效 |
| 后端插件停用 | `plugin.invoke` 返回 `NotReady`；预取写 `errorKind: plugin_not_ready`；页面照常渲染 |
| 后端插件重载 | 动作表重装配；下一次调用看到新动作；旧动作立即 404 |
| 后端插件删除 | 同停用；前端插件不用改，错误文案说明"后端插件不存在" |
| agent 未就绪 | 桥接 503 / `NotReady`；页面显示"未连接"，不渲染假成功 |

### 8.4 诊断

新增诊断点位（需要进 `agent-base/src/domain/diag.rs` 的 `DIAG_POINTS`，
并保证真有发射方，`verify-wiring` check K 会核对）：

| 点位 | 载荷 |
|---|---|
| `plugin.action_started` | pluginId / action / intent |
| `plugin.action_finished` | pluginId / action / status / durationMs |
| `plugin.action_denied` | pluginId / action / reason（`write_via_read` / `not_ready` / `busy`） |

动作的输入/输出**不进日志**（可能含路径、密钥、用户数据）；只记大小与耗时。

---

## 9. 协议与代码落点

### 9.1 协议四处同步

| 位置 | 改动 |
|---|---|
| `spec/proto/ada-coding.ext.json` | 登记 `plugin.invoke`（插件管理方法属于 ada-coding 的产品扩展，不在 `base.json`） |
| `crates/agent-proto/src/methods.rs` | `pub const PLUGIN_INVOKE` |
| `crates/agent-rpc/src/server/dispatch/mod.rs` | 分发到 `PluginManager::invoke_action` |
| `crates/agent-proto/client-ts/methods.ts` | `PLUGIN_INVOKE` |

守门测试：现有方法覆盖测试会同时检查 spec / methods / dispatch / client-ts，
少一处就红。`plugin.list` 的 `actions` 字段是**加载期视图**，不新增方法。

### 9.2 Rust 侧

| crate / 文件 | 改动 |
|---|---|
| `crates/agent-node/src/plugins/types.rs` | `PluginActionDeclaration`、`PluginActionInfo`、`PluginCapabilities::action_timeout_ms`；`LoadedPluginContributions.actions` |
| `crates/agent-node/src/plugins/sandbox.rs` | inspect 路径收集 `registerAction`；新增 `call_action_budgeted`（与 `call_tool_budgeted` 同构，独立的调用脚本）；调用路径的 `registerAction` 是空实现 |
| `crates/agent-node/src/plugins/manager.rs` | 装配动作表；`invoke_action(plugin_id, action, input, intent)`；metadata 对账；并发计数 |
| `crates/agent-node/src/plugins/view.rs` | `PluginContributionView` / `PluginItem` 增加 `actions` |
| `crates/agent-rpc/src/server/dispatch/mod.rs` | `PLUGIN_INVOKE` 分支；错误映射 |
| `crates/agent-web/src/plugins/manifest.rs` | `backend` 字段与校验（group/binding 名、完整插件 id、read/confirm） |
| `crates/agent-web/src/plugins/registry.rs` | 暴露 `backend_bindings()` / `find_backend_binding()`；`has_backend_reads()` |
| `crates/agent-web/src/handlers/bridge.rs` | 新路由处理器 |
| `crates/agent-web/src/handlers/mod.rs` | 首屏预取注入 `props.data.backend` |
| `crates/agent-web/src/router.rs` | 注册 GET/POST 路由 |

### 9.3 文档与示例

- 更新 [README.md](README.md)（新增"前端 ↔ 后端"一节）、
  [extension-factory.md](extension-factory.md)（`registerAction` 精确形状）、
  [extension-api.d.ts](extension-api.d.ts)（类型声明）、
  [protocol/README.md](../protocol/README.md)（新方法与错误表）、
  [PLUGIN-DATA](../../crates/agent-web/docs/PLUGIN-DATA.md)（`data.backend` 契约）；
- 新增一个**成对示例**：
  - `plugins/system-panel/`：插槽 + 一个只读状态块 + 一个写入按钮；
  - 后端示例插件放进文档/模板（不默认启用）：`system-monitor`，提供 `status`（read）与
    `refresh`（write）两个动作；
- `plugin.createTemplate` 生成的新插件骨架里加一个注释掉的 `registerAction` 示例，
  避免作者只能从文档里抄。

---

## 10. 落地批次

### M0：后端动作运行时（约 1.5 天）

- [ ] `registerAction` 收集、`metadata.actions` 对账、`plugin.list.actions`；
- [ ] `call_action_budgeted`（新沙箱、输入/输出上限、超时、并发）；
- [ ] `plugin.invoke` 的四处协议登记与 dispatch；
- [ ] 错误映射与诊断点位；
- [ ] 单测：read/write、intent 拒绝、未启用、未知动作、超时、超大输出、抛错、并发满、
      重载后动作表更新、旧插件无 `actions` 字段仍可装载。

**出场标准**：不接任何前端插件，直接用 `plugin.invoke` 能调通一个动作；
`cargo test -p agent-node --lib` 与 `-p agent-rpc` 绿；方法覆盖门绿。

### M1：前端 manifest 与首屏预取（约 1 天）

- [ ] `backend` 字段解析与校验（含错误信息可读）；
- [ ] `props.data.backend` 注入与失败降级；
- [ ] 成对示例的只读部分；
- [ ] 测试：manifest 校验、只读预取、后端停用降级、同动作去重。

**出场标准**：前端插件无需写任何 JS，首屏就能显示后端动作返回的数据；
后端插件停用后页面仍 200，只是那块显示"读取失败"。

### M2：浏览器写入桥接（约 1 天）

- [ ] GET/POST 路由、JSON/303 两种回执、`Referer` 限制；
- [ ] guard 复用（Origin/CSRF/cookie）与 GET 只读强制；
- [ ] 示例的写入按钮与插件脚本；
- [ ] 测试：GET 写入 405、跨站 POST 403、未声明 binding 404、
      写入成功 303/JSON、写入失败文案、`read:false` 不能被 GET。

**出场标准**：真实浏览器点一次写入按钮，动作执行、页面回显成功；
用 curl 伪造跨站 Origin 被 403。

### M3：清尾与文档（约 0.5 天）

- [ ] 诊断点位、日志字段、错误文案统一；
- [ ] 文档四处更新、示例 README、`docs/README.md` 登记；
- [ ] `cargo test --workspace -- --test-threads=1`、`bun run typecheck`、
      `cargo xtask verify --fast` 全绿；
- [ ] 实机脚本：真网关 + 真 `ada-coding` + 示例前后端插件，
      覆盖"首屏只读、写入、停用、重载、跨站拒绝"。

### 后续（不在本次范围）

- 后端 → 前端推送：插件动作产生的事件走现有 SSE `parts`，或新的插件事件总线；
- `props.backend.read(...)`（只读组合调用）；
- 动作读取非敏感 `config`（需要单独的键白名单设计）；
- 内置插件动作。

---

## 11. 测试清单（按层）

| 层 | 用例 |
|---|---|
| 沙箱 | `registerAction` 收集；对象形态被拒；`execute` 抛错带栈；超时终止；输入/输出上限；`registerTool` 在动作路径是空实现 |
| 装配 | `metadata.actions` 数量/顺序不符即拒；旧插件无 `actions` 仍可装载；重载后动作表替换；停用即不可调用 |
| 协议 | `plugin.invoke` 参数校验；`intent=read` 拒绝 write；错误码/`data.kind`；方法覆盖门 |
| SSR | manifest 校验（重复 group、非法 binding、裸插件名、`..` 路径）；预取只读绑定；失败降级为 `ok:false`；同动作去重 |
| 桥接 | GET 只读；GET 写入 405；POST JSON/表单；303 回跳只限本插件页；跨站 403；未登录 401/302；未知 binding 404 |
| 实机 | 真网关 + 真 agent + 示例插件：首屏显示、点击写入、停用后端、重载动作、跨站拒绝 |

---

## 12. 风险与对策

| 风险 | 对策 |
|---|---|
| 页面一次预取 N 个动作，首屏变慢 | 只预取 `read: true`；按动作去重；单动作超时；后续可加"最多预取 8 个"的上限 |
| 前端插件把写动作标成 `read: true` | 后端按 action 的 `mode` + `intent` 拒绝；前端声明不是权限 |
| 动作变成绕过审批的写文件通道 | 动作在 `@ada-needs` 沙箱里跑，fs 根是工作区；不提供宿主写文件 API；写入动作仍需用户点击 |
| Boa 同步死循环拖住 agent | 复用 `Limits` 指令/递归预算 + 墙钟超时 + 同插件并发上限 |
| 动作输出过大撑爆 SSR 页面 | 256 KB 上限；预取失败只影响该块；页面不整页 500 |
| 新增协议面扩大攻击面 | 只登记 `plugin.invoke`；不映射到 HTTP `/api/*`；浏览器只能经 binding 路由 |
| 插件重载时动作表与前端缓存不一致 | 不缓存；每次桥接都查注册表；前端预取数据带 `fetchedAt` |
| 旧插件被 `metadata.actions` 破坏兼容 | 字段可选；只有"注册了动作但没声明"才拒；旧插件零改动 |

---

## 13. 待拍板项

1. **动作是否允许返回 HTML**：v1 只返回 JSON，渲染交给前端 TSX。若允许 HTML，
   就要处理白名单清洗与 CSP，收益不大。
2. **写入动作是否需要核心二次确认页**：v1 用插件自己的 `confirm` + 登录态；
   若产品要求"无 JS 也必须确认"，再加一个通用确认页。
3. **`plugin.invoke` 是否对 `/api/*` 暴露**：建议**不暴露**，只走 WS RPC 与 SSR 桥接；
   如果要给非 SSR 客户端用，再单独评估鉴权与 CSRF。
4. **动作超时默认值**：5000 ms 是否足够；慢动作（如扫描大目录）是否应引导用户
   改成长任务 + 轮询，而不是放宽超时。
5. **是否允许 `workspace:` 插件被全局前端插件绑定**：技术上可以，
   但换工作区后 binding 可能 404；建议前端 manifest 优先绑 `global:`，
    workspace 绑定在文档里注明"只在该工作区可用"。
