# 统一插件包：一个包同时包含前端与后端

> **状态：v1 已落地（2026-10-26）。** 这是当前实现说明。旧版“前端插件 + 独立后端插件”
> 仍兼容；两者之间没有直连通道，统一包补上了这一条。
> 精确的工厂 API 见 [extension-factory.md](extension-factory.md)，前端数据契约见
> [PLUGIN-DATA.md](../../crates/agent-web/docs/PLUGIN-DATA.md)。

---

## 1. 包形态

一个目录、一个 `plugin.json`，同时声明前端与后端：

```text
<plugins-root>/unified-demo/
  plugin.json          # 顶层元数据 + frontend / backend 两段
  index.ts             # 后端 Extension Factory（包根）
  slot.tsx             # 前端插槽/页面模板（路径相对包根）
  static/
    unified-demo.css
  skills/              # 可选：后端技能目录
    unified-demo/SKILL.md
```

```json
{
  "id": "unified-demo",
  "name": "统一插件示例",
  "version": "0.1.0",
  "description": "一个包同时包含前端插槽与后端动作",
  "frontend": {
    "slots": [
      { "slot": "sidebar.bottom", "template": "slot.tsx", "order": 20 }
    ],
    "assets": "static",
    "styles": ["unified-demo.css"],
    "actions": {
      "status": { "action": "status", "read": true },
      "refresh": { "action": "refresh", "confirm": "确定刷新？" }
    }
  },
  "backend": {}
}
```

- `id` / `name` / `version` / `description` 是**包级元数据**，两侧共用；
- `frontend` 段与旧版扁平清单字段相同，另加 `actions`；
- `backend` 段表示“这个包有后端入口”，入口固定为包根 `index.ts` / `index.js`；
  可选 `backend.pluginId` 覆盖后端 id（默认 `global:<包目录名>`）。

旧版扁平清单仍然有效：

```json
{
  “id”: “hello-tsx”,
  “name”: “Hello TSX”,
  “pages”: [{ “slug”: “hello”, “title”: “Hello TSX”, “template”: “page.tsx” }]
}
```

`frontend` 与顶层扁平字段**不能同时写**；加载期会拒绝，避免“哪份生效”说不清。
仓库自带的纯前端插件（`ask-user` / `hello-tsx` / `process-manager` /
`provider-admin` / `task-manager` / `token-weather`）已经统一到 `frontend` 段，
它们没有后端入口，因此不写 `backend`。`task-plan` 与 `approval` 是完整的前后端
一体例子：同包 `index.ts` 订阅钩子维护状态，前端通过只读 action 读取；
审批策略/闸门仍在核心。

---

## 2. 发现路径

| 侧 | 扫描根 | 规则 |
|---|---|---|
| 前端 | `--web-plugins-dir` / `~/.a-da/web-plugins` / 包内 `frontend-plugins` | 旧扁平清单；有 `plugin.json` 即可 |
| 前端 | `<app_home>/plugins` 或 `A_DA_UNIFIED_PLUGINS_DIR` | 只收有 `frontend` 段的包 |
| 后端 | `<workspace>/.ada/agent-coding/extensions`、`~/.a-da/agent-coding/extensions` | 旧包格式；包根 `index.ts` |
| 后端 | `<app_home>/plugins` 或 `A_DA_UNIFIED_PLUGINS_DIR` | 只收 `backend` 段的包 |

开发时让网关与 agent 看到同一个仓库目录：

```bash
A_DA_UNIFIED_PLUGINS_DIR=E:/codes/a_da/plugins \
  cargo run -p agent-gateway -- --port 52353 --workspace E:/your-project
```

`A_DA_UNIFIED_PLUGINS_DIR` 会被网关拉起的 agent 继承；两侧扫的是同一个包。
默认安装位置是 `~/.a-da/plugins`。开发构建（debug）会自动优先仓库 `plugins/`；
打包后 `plugins/` 随 exe 放在同级（Electron 是 `resources/bin/plugins`），
两侧都按“与 exe 同级”找到它。

**后端 id**：默认 `global:<包目录名>`；`backend.pluginId` 可显式覆盖。
**前端 id**：`plugin.json` 的 `id`，可以不同于目录名（不推荐，便于排查请保持一致）。

---

## 3. 后端动作：`ada.registerAction`

```ts
// @ada-needs fs
const metadata = {
  name: "unified-demo",
  description: "统一插件示例",
  tools: { count: 0, names: [] },
  systemPrompt: { count: 0, names: [] },
  hooks: { count: 0, names: [] },
  actions: { count: 2, names: ["status", "refresh"] },
};

export default function (ada) {
  ada.metadata = metadata;

  ada.registerAction({
    name: "status",
    label: "读取状态",
    description: "读取工作区里的状态文件",
    mode: "read",
    async execute(input, ctx) {
      return { ok: true, data: { count: 0 } };
    },
  });

  ada.registerAction({
    name: "refresh",
    description: "递增计数并写回工作区",
    mode: "write",
    async execute(input, ctx) {
      return { ok: true, data: { refreshedAt: ctx.now } };
    },
  });
}
```

规则：

- `name` 必填、同插件内唯一；`mode` 只认 `read`，其余一律按 `write`（失败安全）；
- `execute(input, ctx)` 的 `ctx` 包含 `workspace` / `caller: "web"` / `now`；
- 返回值：任意 JSON → 宿主包成 `{ok:true,data}`；返回 `{ok:false,error}` → 软失败；
- `metadata.actions` 可选；**注册了动作就必须写**，`count` / `names` 逐项对账；
- 动作**不是模型工具**：不进工具目录、不产生工具卡、不进入审批闸门；
- 每次调用新建沙箱，模块状态不跨请求；需要持久化请写工作区文件；
- 动作没有读取 `pluginConfig` / `secret` 的 API（见 §7）。

---

## 4. 前端绑定与读取

`frontend.actions` 的键是 **binding 名**（URL 段），值是后端动作名：

```json
"actions": {
  "status": { "action": "status", "read": true },
  "refresh": { "action": "refresh", "confirm": "确定刷新？" }
}
```

- `read: true` 才允许首屏预取与 `GET`；写入动作只能 `POST`；
- `confirm` 只是插件脚本/表单的交互提示，**不是权限边界**；
- binding 名只允许小写字母/数字/`-`/`_`，首字符为字母或数字。

### 首屏预取

`agent-web` 渲染页面前，对所有启用的 `read` binding 调用一次
`plugin.invoke(intent=read)`，结果注入：

```jsonc
{
  "data": {
    "backend": {
      "unified-demo": {
        "status": {
          "ok": true,
          "data": { "count": 0 },
          "error": null,
          "fetchedAt": 1760000000000
        }
      }
    }
  }
}
```

- 每个 binding 独立超时（6 秒），失败只写 `ok:false/error`，**不让整页 500**；
- 最多预取 16 个 binding；
- 写入动作永远不预取。

### 浏览器动作

```text
GET  /ext/{frontend_id}/backend/{binding}   → 只读，JSON
POST /ext/{frontend_id}/backend/{binding}   → 读/写；JSON 或 303
```

- `GET` 只接受 `read:true`，否则 405；
- `POST` 接受 `application/json` 或 `application/x-www-form-urlencoded`，体 ≤64KB；
- `Accept: application/json` / `HX-Request: true` → JSON `{ok,data}` / `{ok:false,error}`；
- 普通表单 → 303 回原页面（`notice=backend_action_ok` 或 `error=...`），
  回跳只接受同源相对路径，挡开放重定向；
- 浏览器不接触网关 token，也不直接连 `/rpc`。

### 动态路由（`ada.route`）

动作适合“固定 binding 的按钮”，但后端能力经常需要**自己的 URL**。这类接口
不需要改宿主：在 `index.ts` 里声明即可，宿主只提供一条兜底通道。

```ts
ada.route({
  method: "GET",
  path: "/hello/:name",
  async handler(req, ctx) {
    return { hello: req.params.name, q: req.query.q, at: ctx.now };
  },
});
```

```text
GET /ext/{frontend_id}/route/hello/ada?q=1
  → agent-web 查插件 + backend id
  → plugin.route（method/path/query/headers/body）
  → TS 沙箱按 ada.route 注册表匹配
  → handler 返回 JSON / ada.text / ada.html / ada.redirect / ada.response
```

- 路径支持静态段、`:param` 与末段 `*rest`；静态优先于参数，参数优先于通配；
- 请求体 ≤1MB（JSON / 表单 / 文本 / base64），响应 ≤2MB，15 秒沙箱超时
  （`multipart/form-data` 暂未支持，按文本/base64 处理）；
- `cookie` / `authorization` 不转发给插件；响应头只放行常见内容头与 `x-*`；
- 一个包同时有 frontend/backend 才有浏览器入口，`backend` 段仍然是开关；
- `route` 是保留的页面 slug（`/ext/{id}/route` 归动态路由使用），插件页面不要再占它；
- 路由不是模型工具，也不进审批闸门；它只是**后端 HTTP 能力**的同源入口；
- v1 是请求-响应，**不支持 SSE / WebSocket 流式**；需要长流时再补 `ada.stream` 一类的入口；
- `GET` 是否只读由插件自己守：宿主不检查 handler 的副作用；需要写操作请用
  `POST/PUT/PATCH/DELETE`（入口 guard 会校验跨站写请求）；
- 每次请求重新执行工厂、重新收集 `ada.route`，模块状态不跨请求（要状态请写文件）。

完整 API 与响应助手见 [extension-factory.md](extension-factory.md) 的
“Dynamic HTTP routes”。

---

## 5. 一次完整链路

```text
1. 首屏
   agent-web 扫描统一包 → 发现 frontend.actions.status(read)
     → plugin.invoke(global:unified-demo, status, intent=read)
     → agent 沙箱执行 registerAction('status')
     → props.data.backend["unified-demo"].status
     → slot.tsx 渲染

2. 写入
   用户点表单
     → POST /ext/unified-demo/backend/refresh
     → agent-web 查 binding → plugin.invoke(..., intent=write)
     → 沙箱检查 mode=write → 执行 → 写工作区文件
     → 303 回原页面
     → 首屏预取重新读取 status，显示新计数
```

---

## 6. 安全与边界

| 边界 | 规则 |
|---|---|
| 浏览器 → SSR | 会话 cookie、Host 白名单、Origin/CSRF；与普通页面动作一致 |
| SSR → agent | 服务端 `AgentClient`；网关 token 不下发浏览器 |
| 前端能调什么 | 只有 `frontend.actions` 显式列出的 binding |
| 后端怎么判 | 沙箱按动作自己的 `mode` 判 `intent`；前端把写动作标只读也绕不过 |
| 数据面 | 动作输入 ≤64KB、输出 ≤256KB、单次超时 5 秒 |
| 沙箱 | 与工具相同的 `@ada-needs` / 工作区 fs / net 策略；无模型、无工具、无审批 |
| 信任 | 统一包仍是**本地受信任内容**；没有签名校验 |

---

## 7. 当前 v1 边界

- 一个统一包只有**一个后端 id**；跨包绑定（前端插件调用另一个包的后端）尚未实现；
- `confirm` 只由插件自己呈现，核心不强制二次确认；
- 动作列表还没有并入 `plugin.list`，管理页暂时看不到 `actions` 计数；
- 动作没有 `pluginConfig` / `secret` 读取 API，也没有动作级并发限流（只有超时/大小上限）；
- 不支持后端 → 前端主动推送；需要实时刷新时用只读动作轮询或整页刷新；
- 旧版“单独前端插件 + 单独后端插件”仍没有直连通道；统一包是唯一支持直连的形态。

---

## 8. 代码落点

| 关注点 | 文件 |
|---|---|
| 统一清单解析 / 校验 | `crates/agent-web/src/plugins/manifest.rs` |
| 前端双根扫描 / actions | `crates/agent-web/src/plugins/registry.rs` |
| 首屏预取 | `crates/agent-web/src/handlers/mod.rs` 的 `attach_backend_actions` |
| SSR 桥接路由 | `crates/agent-web/src/handlers/bridge.rs` |
| 统一包后端扫描 | `crates/agent-node/src/plugins/manager.rs` |
| 动作沙箱 | `crates/agent-node/src/plugins/sandbox.rs` 的 `call_action_budgeted` |
| 路由沙箱 | 同文件的 `call_route_budgeted`（`ROUTE_RUNTIME_JS` 负责匹配） |
| `plugin.invoke` 分发 | `crates/agent-rpc/src/server/dispatch/mod.rs` |
| `plugin.route` 分发 | 同文件；SSR 入口在 `crates/agent-web/src/handlers/plugin_route.rs` |
| 协议登记 | `spec/proto/ada-coding.ext.json`、`crates/agent-proto/src/methods.rs` |
| 示例 | `plugins/unified-demo/` |

---

## 9. 验证

- `cargo test -p agent-node actions_are_inspected_and_read_intent_cannot_run_write`
- `cargo test -p agent-node unified_package_is_scanned_and_actions_enforce_intent`
- `cargo test -p agent-node dynamic_routes_match_params_and_return_http_shapes`
- `cargo test -p agent-node unified_package_dynamic_routes_go_through_the_ts_engine`
- `cargo test -p agent-rpc plugin_route_dispatches`
- `cargo test -p agent-web --lib`（含统一清单与仓库示例加载测试）
- 实机：起网关时设 `A_DA_UNIFIED_PLUGINS_DIR=plugins`，访问
  `/ext/unified-demo/backend/status`，再 `POST .../refresh` 看计数递增；
  动态路由用 `bun tools/verify-dynamic-routes-e2e.mjs` 一次跑完
  （路径参数 / JSON / 表单 / 敏感头 / text / 404 / 停用）。
