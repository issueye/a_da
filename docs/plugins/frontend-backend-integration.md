# 前端插件与后端插件：现有对接方案

> **状态：现状说明（2026-10-26）。**
> **统一插件包 v1 已落地**：一个 `plugin.json` 同时声明 `frontend` / `backend` 的包，
> 可以通过 `registerAction` + `plugin.invoke` 直接对接，见
> [unified-package.md](unified-package.md)。本文其余部分描述的是**旧版分离插件**
> （前端插件目录 + 独立后端插件包）的对接方式；它们之间仍没有直连通道。
> 完整设计提案见 [PLAN-frontend-backend-bridge.md](PLAN-frontend-backend-bridge.md)。
> 两套插件各自的完整契约见 [README.md](README.md) 与
> [extension-factory.md](extension-factory.md)。

---

## 1. 一页结论

**旧版分离插件**没有“前端插件 → 后端插件”的直接调用通道；**统一插件包**有。
两套插件仍然只在**核心定义的契约**上相遇：

```text
后端插件（agent）
  └─ 工具 / 钩子 / 提示词 / 技能
        │
        ▼
   agent 引擎与状态（tool 卡、todo、提问、审批、会话快照）
        │
        ▼
   agent-web 的 props.data 投影
        │
        ▼
前端插件（SSR TSX）
  └─ 页面 / 插槽 / 仪表盘 / 插件 JSON API
```

- 前端能**读**后端产生的状态，是因为宿主把状态投影进了 `props.data`；
- 前端能**触发**动作，是因为它提交到了**核心路由**（发送、审批、作答、供应商保存等）；
- 前端**不能**按名字调用某个后端插件的工具或钩子；
- 后端**不能**定义页面或插槽，也不能直接向前端插件推送事件。

一句话：**读走投影，写走核心路由；插件之间不直连。**

---

## 2. 拓扑

```text
┌────────────────────────────── 浏览器 ──────────────────────────────┐
│  SSR 页面 / TSX 插槽 / 可选插件脚本（同源）                        │
│  表单或 fetch → 核心路由（/threads/*、/questions/*、/providers/*）  │
└───────────────────────────────┬────────────────────────────────────┘
                                │ 会话 cookie + Host/Origin 校验
                                ▼
┌────────────────────────── agent-web（SSR 宿主） ───────────────────┐
│  - 读 agent 快照 / 配置 / 进程管理面                                │
│  - 组装 props.data（会话、token、todo、提问、provider、processes）  │
│  - 渲染 TSX 页面 / 插槽 / 仪表盘                                   │
│  - 核心路由内部再调 AgentClient.rpc(...)                            │
│  - 不跑多轮引擎，不缓存会话真源（INV-8）                            │
└───────────────────────────────┬────────────────────────────────────┘
                                │ 进程内（网关）或 WS（独立运行）
                                ▼
┌────────────────────────── agent-rpc / agent ──────────────────────┐
│  - JSON-RPC 方法（thread.send、approval.decide、question.answer…）  │
│  - 后端插件装配：工具 / 提示词 / 技能 / 钩子                        │
│  - 引擎执行工具、触发钩子、产生事件与快照                           │
└────────────────────────────────────────────────────────────────────┘
```

关键点：**浏览器不持有网关 token，也不直接连 `/rpc`**。前端插件脚本与普通页面 JS 一样，
只能带会话 cookie 调同源核心路由；真正到 agent 的调用由 SSR 宿主在服务端完成。

---

## 3. 两套插件的身份与作用域

| 项 | 前端插件 | 后端插件 |
|---|---|---|
| 清单位置 | `<plugins-dir>/<目录>/plugin.json` | `<workspace>/.ada/agent-coding/extensions/<目录>/index.ts` 或 `~/.a-da/agent-coding/extensions/<目录>/index.ts` |
| 身份 | `plugin.json` 的 `id`（不必等于目录名） | `workspace:<目录名>` / `global:<目录名>`；内置为 `builtin:<id>` |
| 作用域 | **网关全局**：一份目录服务所有工作区 | **agent 级**：工作区插件只在该工作区可见，全局插件跨工作区 |
| 发现方式 | 启动扫描 + `/extensions` 重载 | 启动装配 + `plugin.reload` 重装配 |
| 停用状态 | `~/.a-da/web-ui-state.json` 的 `disabled` | `config.json` 的 `disabledPlugins` |
| 主要消费者 | 人和浏览器 | 模型与引擎 |

这个作用域差异是后续所有对接设计的前提：**前端插件是全局 UI，后端插件是某个
工作区 agent 的能力**。两者没有天然的 1:1 绑定关系。

---

## 4. 现有对接点

### 4.1 宿主数据投影：前端“读”后端状态

`agent-web` 在渲染页面前组装 `RenderContext.data`，TSX 通过
`props.data.*` 拿到只读投影。当前字段包括：

| 字段 | 来源 | 谁在用 |
|---|---|---|
| `tokens` | 最近一次助手回复的 usage | `token-weather` |
| `thread` / `counts` | 会话快照 | 通用 |
| `todo` | 核心 `todo_view` 推演；**统一 `task-plan` 的后端 action 优先** | `task-plan`（降级） |
| `backend` | 统一插件只读动作的预取结果 | `task-plan`（`plan` action） |
| `questions` | `ask_user` 工具挂起的提问 | `ask-user`（`thread.tray`） |
| `model` / `config` / `workspace` | 快照与配置 | 通用 |
| `providerAdmin` | 供应商 RPC + 配置，**仅声明 `providers.read` 的插件页面** | `provider-admin` |
| `processes` | 网关管理面，**任一启用插件声明 `processes.read` 时全页注入** | `process-manager` |
| `tasks` | 后台任务表（`run_background` 族），仅声明 `tasks.read` | `task-manager` |
| `runs` | 会话快照里的 `run_command` tool item，仅声明 `runs.read` | `task-manager` |

实现位置：

- 通用投影：`crates/agent-web/src/handlers/mod.rs` 的 `plugin_data` / `render_context_with_tokens`；
- `processes.read`：`crates/agent-web/src/handlers/mod.rs:315`；
- `runs.read`：`crates/agent-web/src/handlers/mod.rs` 的 `runs_view`（纯函数，不新增 RPC）；
- `providers.read`：`crates/agent-web/src/handlers/extensions.rs:199`；
- TSX props 组装：`crates/agent-web/src/plugins/registry.rs` 的 `tsx_props`。

完整字段、类型与刷新时机见
[PLUGIN-DATA.md](../../crates/agent-web/docs/PLUGIN-DATA.md)。

🔴 **这是只读投影，不是后端插件句柄**：`props.data` 里没有“调用某个后端插件”的函数，
也没有后端插件的 id / 动作 / 工具名。前端插件只能消费宿主已经决定要投影的字段。

### 4.2 核心路由：前端“写”后端状态

前端插件的表单、HTMX 片段或浏览器脚本，提交到**核心路由**，由 SSR 宿主在服务端调用
agent RPC：

| 前端动作 | 核心路由 | 最终落到 agent 的 |
|---|---|---|
| 发送消息 | `/threads/{id}/send` | `thread.send` |
| 回答提问 | `/questions/{call_id}/answer` | `question.answer` |
| 保存供应商 | `/providers/*` | `provider.save` 等 |
| 新建会话/工作区 | `/threads`、`/workspaces/select` | `thread.create` / `workspace.add` |
| 重载后端插件 | `/agent-plugins/reload` | `plugin.reload` |

这条路是**用户级动作面**，不是“任意插件动作面”。如果某个动作没有对应的核心路由，
前端插件不能自己发明一条到 agent 的调用；它只能新增核心路由，或者等插件动作桥接落地。

### 4.3 后端工具/钩子产生界面可见的状态

后端插件不能直接渲染 UI，但它的工具结果、提问、计划事件会被核心投影成卡片，
再被前端插件消费。典型链路：

```text
后端插件/内置工具
  └─ ask_user 工具挂起
       └─ 事件 / 快照里的 waiting question
            └─ props.data.questions
                 └─ plugins/ask-user 的 thread.tray 插槽
                      └─ 表单提交到 POST /questions/{callId}/answer
                           └─ question.answer → ask_user 工具返回
```

`todo` 与 `task-plan` 也是同一模式：**后端产生状态，核心投影状态，前端渲染状态**。
前端插件与后端插件之间没有直接调用，只有共享的核心状态契约。

### 4.4 管理页：两套插件并列，但管理动作不同

`/extensions` 同时列出：

- 前端插件：可重载、启用、停用（`POST /extensions/{id}/enable|disable`）；
- Agent 插件：通过 `plugin.list` 读取，页面只提供“重载 agent 插件”
  （`POST /agent-plugins/reload`），**启停不在 SSR 页面暴露**。

两者共享一个页面，但生命周期是两套：前端插件重扫目录，后端插件重装配引擎贡献。

### 4.5 浏览器脚本：只走同源端点

前端插件的可选 `scripts` 会在所有页面加载（`PluginRegistry::scripts`），它：

- 能 `fetch` 自己的 `/ext/{id}/api/*` 同源 JSON API；
- 能提交核心路由（带会话 cookie）；
- 不能拿网关 token，也不能直接连 `/rpc`；
- 没有 `props`，拿不到服务端渲染上下文。

插件 API 端点的 `props.data` **只有 `plugin_count`**，不会拖入会话快照；需要会话数据时，
必须由调用方带 id 走核心路由。这是刻意的数据面收窄。

---

## 5. 现有协同案例

### 5.1 `ask-user`：同主题，但不是插件互调

- 后端：内置插件 `ask-user` 提供 `ask_user` 工具；
- 前端：`plugins/ask-user` 用 `thread.tray` 渲染 `data.questions`；
- 中间：`question.answer` 核心路由与提问事件。

前端插件停用后，核心提问托盘自动接管。这是“共享核心契约”的正面例子，不是两个插件
互相调用。

### 5.2 `task-plan`：核心投影上的前端卡片

- `todo` 工具执行后，核心 `todo_view` 从工具调用参数投影出计划；
- `plugins/task-plan` 是**纯前端插件**，直接读 `props.data.todo`；
- 2026-10-27：`afterTodoUpdate` 已从核心移除，不再有后端状态文件或 `plan` action；
- 后续需要拦截/旁注时，由统一事件系统提供。

### 5.3 `process-manager`：不依赖后端插件

`processes.read` 的数据直接来自网关管理面，不是某个 agent 插件。它只读展示网关 PID、
实例数、工作区、状态等；刷新是整页重载。

### 5.4 `provider-admin`：前端页面，核心写入

- `providers.read` 只影响**插件独立页面**的 `providerAdmin` 投影；
- 新建/保存/删除/切换供应商仍提交到核心 `/providers/*`；
- API Key 不进入 Boa，插件只渲染“已配置 + 掩码”。

### 5.5 `approval`：已随审批物理删除（2026-10-27）

- `plugins/approval`、`thread.approval` 插槽、`/approvals/*` 路由与 `approval.decide`
  均已删除；核心不再产生 `waiting_approval` 新卡；
- 需要策略时由受信插件通过 `event_before_*` 决定，不再有核心审批闸门；
- 历史会话里的 `waiting_approval` 卡片按已结束记录展示，不带操作按钮。

### 5.6 `task-manager`：后台任务表的核心投影

- `run_background` 族在 agent 进程里持有一张实例态 `BackgroundTasks` 表；
- `AgentClient` 经 `task.list` 按 `threadId` 取当前会话任务，注入 `props.data.tasks`；
- `plugins/task-manager` 挂在 `composer.above`（输入框上方悬浮区）：胶囊 + 向上弹出的
  浮窗，渲染命令、PID、状态与输出预览（与 Token 天气同构）；
- **输出是推送的**：任务表没有事件通道（网关不广播它），聊天页的 SSE 事件循环按 1 秒
  问一次 `task.list`，用指纹（`id` / `running` / `exitCode` / 输出长度）判断变化，
  变了就补一帧 `parts`；同一帧里已取到的表会直接交给插槽渲染，不会打两次 RPC。
  只在有任务在跑时轮询，没有任务时不空转；
- 终止走核心 `POST /tasks/{id}/kill` → `task.kill`，插件不直接操作进程。

任务表不落盘、不跨进程；插件停用或读取失败时胶囊不显示/显示错误，不影响工具本身。

### 5.7 `ada.route`：后端接口由 TS 自己长出来

动作是“固定 binding 的按钮”，动态路由是“插件自己的 URL”：

```text
浏览器 GET /ext/unified-demo/route/hello/ada?q=1
  → agent-web 查启用插件 + backend id
  → plugin.route(method/path/query/headers/body)
  → TS 沙箱重新执行工厂，收集 ada.route 注册表
  → 匹配 /hello/:name → handler
  → 归一化 status/headers/body → HTTP 响应
```

- 路由表完全在 TS 侧；宿主只保留一条兜底路由，**加接口不改 Rust**；
- 请求体 ≤1MB、响应 ≤2MB、15 秒超时；`cookie` / `authorization` 不转发；
- 响应可用 `ada.json/text/html/redirect/response`，头只放行常见内容头与 `x-*`；
- 这条路不进模型工具目录、不产生工具卡、不经过审批闸门；它是同源后端数据面。

契约见 [unified-package.md](unified-package.md) 与 [extension-factory.md](extension-factory.md)。

---

## 6. 今天能做什么、不能做什么

| 需求 | 现状 | 做法 |
|---|---|---|
| 前端展示后端插件产生的状态 | ✅ 可以 | 核心把状态投影进 `props.data`；统一包也可预取动作 |
| 前端触发一个后端插件工具 | ❌ 不可以 | 没有“调用工具”的动作接口；统一包用 `registerAction`，不是工具 |
| 前端按名字调用后端插件动作 | ✅ 统一包可以；旧版分离插件不可以 | `frontend.actions` binding + `plugin.invoke` |
| 后端插件定义页面/插槽 | ❌ 不可以 | 统一包也是前端 TSX + 后端动作，后端不定义 UI |
| 后端插件向前端插槽推送数据 | ❌ 不可以 | 没有插件事件总线；只能经核心事件/快照 |
| 前端列出后端插件 | ✅ 可以 | `/extensions` → `plugin.list` |
| 前端重载后端插件 | ✅ 可以 | `/agent-plugins/reload` → `plugin.reload` |
| 前端启停后端插件 | ❌ 未暴露 | RPC 有 `plugin.setEnabled`，SSR 页面没有入口 |
| 前端读取插件配置/密钥 | ❌ 不可以 | `plugin.config.set` / `secret.set` 仅管理面可见；前端插件拿不到，后端插件沙箱也没有读取 API |

**不要用“让模型去调那个工具”当插件对接方案**：它会引入模型决策、审批、上下文和
不确定性，无法保证某个按钮一定执行某个动作。统一包的动作桥接正是为了避免这一点。

---

## 7. 今天要新增一个对接，应该改哪里

### 方案 A：只读展示（推荐）

1. 在 agent 侧产生数据（工具结果、事件、快照字段）；
2. 在 `agent-web` 的 `plugin_data` 或专用投影里增加字段；
3. 如果数据敏感，增加一个 manifest capability（像 `providers.read` / `processes.read` / `tasks.read` / `runs.read`）；
4. 在 `plugins/<id>/` 里消费 `props.data.<field>`；
5. 补 `PLUGIN-DATA.md` 与测试。

代价：需要改核心投影层；但边界清晰，符合 INV-8（只投影，不缓存真源）。

### 方案 B：界面触发动作

1. 在核心加一条**有明确语义的路由**（如 `/providers/{id}/test`）；
2. 路由内部调 `AgentClient.rpc(...)` 或直接调用宿主服务；
3. 前端插件表单/脚本提交该路由；
4. 走现有 guard（cookie + Origin/CSRF）；
5. 补路由测试与模板测试。

代价：每新增一个动作都要加核心路由；不能由插件作者自行扩展动作集合。

### 方案 C：插件直连（当前不可用）

需要 `registerAction` + `plugin.invoke` + 前端 binding + SSR 桥接路由，
见 [PLAN-frontend-backend-bridge.md](PLAN-frontend-backend-bridge.md)。在它落地之前，
不要在前端插件里伪造 `/rpc` 调用或把网关 token 带进浏览器。

---

## 8. 安全边界

| 边界 | 现状 |
|---|---|
| 浏览器 → SSR | 登录 cookie、Host 白名单、Origin / `Sec-Fetch-Site`、CSRF 防护 |
| SSR → agent | 服务端 `AgentClient`，网关 token 不下发浏览器 |
| 前端 TSX → 宿主 | 无 fs/net/process；HTML 白名单；500ms 渲染预算；无内存上限 |
| 前端浏览器脚本 | 与页面同源，能调核心路由；**能力声明不约束脚本** |
| 后端插件 → 宿主 | `@ada-needs` 模块注入清单；路径只解析；net 按策略；sqlite/exec 需声明；钩子分级 |
| 前端能力 | `providers.read` / `processes.read` / `tasks.read` / `runs.read` 是**数据最小化**，不是授权系统 |

两个需要特别记住的事实：

1. **前端插件是本地受信任内容**。它的 `scripts` 可以调用同源核心路由；CSP 里的
   `connect-src 'self'` 和 `form-action 'self'` 限制不了同源请求，也不构成插件沙箱。
2. **`processes.read` 当前是全局注入**：只要任一启用插件声明了它，所有插槽的
   `props.data` 都会带上 `processes`。这与文档里“仅声明该能力的插件”表述不完全一致，
   属于后续应收紧的作用域问题。

---

## 9. 代码落点

| 关注点 | 文件 |
|---|---|
| 前端插件清单 / 能力白名单 | `crates/agent-web/src/plugins/manifest.rs` |
| 前端插件扫描 / 重载 / 启停 / props | `crates/agent-web/src/plugins/registry.rs` |
| 数据投影 / 插槽刷新 | `crates/agent-web/src/handlers/mod.rs` |
| 插件页面 / API / `/extensions` | `crates/agent-web/src/handlers/extensions.rs` |
| TSX 沙箱与预算 | `crates/agent-web/src/tsx/mod.rs` |
| 入口防护 / CSP | `crates/agent-web/src/guard.rs` |
| 后端插件扫描与管理 | `crates/agent-node/src/plugins/manager.rs` |
| 后端插件装载与贡献 | `crates/agent-node/src/scripts/assembly.rs` |
| 后端插件沙箱 | `crates/agent-node/src/plugins/sandbox.rs` |
| `@ada-needs` 解析 | `crates/agent-node/src/scripts/needs.rs` |
| 后端工具适配 | `crates/agent-node/src/scripts/plugin_tool.rs` |
| 钩子分级 | `crates/agent-base/src/ports/hooks.rs` |
| `plugin.*` RPC | `crates/agent-rpc/src/server/dispatch/mod.rs` |
| 前端 ↔ 后端动作桥接（统一包 v1） | [unified-package.md](unified-package.md)、`crates/agent-web/src/handlers/bridge.rs` |
| 动作契约与后续提案 | [PLAN-frontend-backend-bridge.md](PLAN-frontend-backend-bridge.md) |

---

## 10. 结论

旧版分离插件的对接方案可以概括为：

> **后端插件产生状态，核心负责投影与动作路由，前端插件负责呈现与提交。**

统一插件包在此之上补了确定性的动作通道：

> **一个 `plugin.json` 同时声明两端；后端 `registerAction` 提供动作，前端
> `frontend.actions` 绑定；SSR 只做服务端中转，浏览器不碰 token。**

它适合“把 agent 的事实展示出来、把用户动作送回后端”的场景，边界清楚，也符合
网关不跑引擎、不缓存会话真源的设计约束。旧版分离插件仍没有直连通道；需要直连时，
把包改成统一格式。无论哪种形态，都不要绕过宿主直接连 agent，也不要把网关 token
暴露给浏览器。
