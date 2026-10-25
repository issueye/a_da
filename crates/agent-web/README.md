# agent-web — a_da 的 SSR 网页界面（产品代号 ssr-h）

> **已并入网关**：`crates/agent-web` 是根 workspace 成员，`ada-gateway` 默认挂载它的
> `router`。一个 `ada-gateway` 进程 + 一个端口 = 整站；页面、`/static/*`、SSE、`/api/*`、
> `/rpc` 都在同一个 axum Router 上。**Electron 桌面壳默认也加载这份 SSR 页面**
> （`http://127.0.0.1:<port>/`，无边框窗口；preload 在 SSR 顶栏注入窗口控制），
> `--ui=desktop` 回退本地 React/Vite。
> 独立运行的 `ssr-h` 二进制只用于开发与实机验收。

UED 设计基线、组件规范与 desktop-ui 对齐差距见 [docs/UED.md](docs/UED.md)。
组件库与插件插槽体系的架构说明见 [docs/COMPONENTS.md](docs/COMPONENTS.md)；
插槽/页面注入的数据契约（字段、类型、刷新时机）见 [docs/PLUGIN-DATA.md](docs/PLUGIN-DATA.md)。
合并方案与批次记录见 [docs/gateway/PLAN-merge-ssr-h.md](../../docs/gateway/PLAN-merge-ssr-h.md)。

## 目标

- 参考 `crates/plugin-hub` 的前端框架：Axum MVC + Askama SSR + HTMX/Alpine + 本地 Tailwind CSS。
- 以 **动态前端插件** 为核心：`frontend-plugins/<id>/plugin.json` 可在运行时扫描、重载，新增导航、仪表盘卡片、独立页面与静态资源，不需要重新编译或重启。
- Agent 数据通过 `WebBackend` 端口获取：网关挂载时走**进程内调用**（`GatewayBackend`），
  独立运行时走 HTTP `/api/*` + WebSocket `/rpc`（`HttpBackend`）；两种实现共用同一套处理器。
- 本 crate 是**投影层**：不持有线程/消息/工具状态的第二份真相，不跑多轮引擎。

## 主界面

- `/`：截图式 Agent 界面——左侧工作区树（按工作区分组会话、当前工作区标记、组内新建/切换）、导航、前端插件、中间对话流、底部输入框。
- `/threads/{id}`：同一界面定位到指定会话；`/overview` 保留原来的统计概览。
- 助手正文用 `pulldown-cmark` 渲染 Markdown，再经 `ammonia` 白名单清洗后输出；工具/思考折叠展示。
- 页面通过 **SSE**（`GET /threads/{id}/stream`）实时更新：服务端持有一条到网关的长连接，订阅增量帧 `evt.state.patch`，把**变过的 item** 渲染成 HTML 推给浏览器——助手正文按 item 原位替换（约 25FPS 合帧），新增/删除卡片才整段同步一次对话流；审批/排队托盘、任务规划、顶栏状态、输入框辅助区走 `parts` 事件（最小间隔 500ms）。断流时 `EventSource` 自动重连，期间由同一段脚本用 2 秒 `fetch` 轮询 `/threads/{id}/live` 兜底，重连后自动停掉。发送消息仍走服务端 WS RPC 后整页跳回。
- 输入框：Enter 发送、Shift+Enter 换行、随内容自动增高；代码块悬停有复制按钮；思考/工具折叠按 item id 在增量同步后保持展开状态，并还原面板内部的滚动位置（否则展开的工具会自己滚回顶部）；只有接近底部时才自动跟随新内容。
- **模型选择（参考 DeepSeek）**：输入框右下角的药丸显示「模型 + 推理等级（Auto/Low/Medium/High）」，
  点开是向上弹出的面板：`模型` 按供应商分组的清单（点一条即切到该供应商 + 该模型）、`推理等级`
  （写网关的 `config.setEffort`）、底部「管理供应商与模型」进插件页 `/ext/provider-admin/providers`。无 JS 也能开合与提交；
  点外面 / Esc 关闭由 `chat.js` 增强；live 轮询只替换药丸标签 `#composer-model`，不会把菜单关掉。
- 向上阅读时对话区底部出现「回到最新」胶囊：点一下回到底部并恢复自动跟随。它是核心 UI（不依赖插件插槽），水平居中，不会和右下角 `composer.above` 的插件悬浮卡抢位置。
- 任务规划（`todo` 工具）由 **`frontend-plugins/task-plan/` 插件**渲染：默认挂在会话区右上角（`chat.corner`），
  收成胶囊药丸（进度 + 当前步骤）/ 展开是步骤清单卡片；改 `plugin.json` 的 `slot` 可换到输入框上方
  （`composer.above`）。计划仍按 `set`/`update`/`get` 的动作在核心推演（不是倒序取最后一张卡片），
  插件只消费 `props.data.todo` 的投影。
- 对话流：工具与思考采用 desktop-ui 的单行日志式——单行显示图标、中文标签、目标对象与真实耗时，展开后是左侧导轨线里的参数/输出面板；工具输出按 Markdown 渲染（JSON 走等宽原文）。
- 侧栏支持按标题搜索会话**和工作区名**（工作区名命中时整组展开）；插件 / 供应商 / 概览 / 插件页也共用同一套侧栏外壳，不再切回旧布局。
- 图标统一为 14–16px 线性 SVG（替代早期字符符号）；工具/思考用单行日志式展示；输入框为桌面端形态（大圆角输入卡 + 底部工具行：工作区/审批档位/模型 + 近黑圆形发送键）；窄窗口（≤760px）侧栏收为抽屉，顶栏出汉堡按钮；**宽屏下同一个按钮是"收起/展开侧栏"**（状态存 `localStorage`，跨刷新保持）；中止等危险操作用共享确认对话框（原生 `<dialog>`）。
- 审批：`waiting_approval` 工具在输入框上方托盘展示命令/参数与等待时长，可批准/拒绝；托盘随事件流出现/消失（断流时由兜底轮询接管），不需要手工刷新。
- 提问（`ask_user`）：待答复的提问在输入框上方展示题目、选项（单选/多选/文本补充），可提交回答；
  提问卡**浮动覆盖在对话流之上、不占布局空间**（desktop-ui 提问浮窗同款，审批/排队托盘仍是 in-flow）；
  **多题一次只显示一题 + 上一题/下一题**（`chat.js` 的 `data-question-pager`
  契约实现）；**已勾选的选项、已输入的文本与当前页码在换帧与整页刷新后都保留**
  （sessionStorage 按 callId 记忆，提交后清除；无 JS 时全部题目平铺）。对话流里的提问卡只留记录。
  作答表单提交到核心路由 `POST /questions/{call_id}/answer`
  （字段约定见 [docs/PLAN-question-panel.md](docs/PLAN-question-panel.md) §4）；
  渲染默认是核心托盘，`frontend-plugins/ask-user/` 示例插件可接管（`thread.tray` 插槽），
  停用或失败自动回退核心实现。提问被回答/超时后托盘随事件流消失。
- 排队：当前会话的排队消息可见，可提升、删除、清空（提升会中断当前回答，已加确认）。
- 失败：`notice.level=error` 的卡片带「重试这一轮」；动作失败会回到会话页用 flash 说明原因，不再把整页换成裸错误页。
- 聊天页在网关断开时保留界面壳并显示「未连接」，已打开会话的 SSE 会自动重连、期间由兜底轮询继续探测，恢复后自动接回；首屏就未连接时请点「重新连接」；`/overview`、`/extensions` 等二级页同样保壳显示「未连接」（不渲染假统计）。

## 工作区

侧栏是**工作区树**，与 desktop-ui 的分组口径一致（同一个 agent 进程能看到所有工作区的会话）：

- 会话按 `workspace` 分组；组头显示目录名、会话数、运行中标记，**当前工作区**默认展开并标「当前」；
- 组内只有空占位会话的工作区（例如刚添加、还没说话）也会出现——每个空工作区保留最新的一条空会话，有真实内容后自动收掉占位；
- 组头操作：组内 `+` 新建；非当前组的 ✓ 切换（有空会话就打开最近一条，没有就新建）；
- **会话行悬浮操作**：重命名（行内展开表单，走 `thread.update`）与删除（确认弹窗 + `thread.delete`）；
- **侧栏随事件流增量刷新**：会话树在 SSE `parts` / 兜底轮询里整块换新（`#thread-list`），
  其它客户端的新会话、改名、运行状态点不再要手动刷新；组折叠状态按 `data-ws-key` 记忆（`localStorage`）；
- 组内只有空占位会话的工作区（例如刚添加、还没说话）也会出现——每个空工作区保留最新的一条空会话，有真实内容后自动收掉占位；
- 顶部「新会话」**直接建一条空会话**（落到当前工作区），不再先弹窗；**模式与工作区
  在空会话页里选**（对齐 desktop-ui 的 EmptyConversationView）：四档模式分段
  （编码/规划/创造/PM，`POST /threads/{id}/mode` → `thread.setMode`，开始后不可切）、
  工作区药丸下拉（`POST /threads/{id}/workspace` → `thread.setWorkspace`，只绑定这条
  会话、不切换项目）+「选择工作区」打开**弹窗目录选择器**（htmx 片段
  `/workspaces/picker`：盘符/常用位置切换、进入子目录、新建文件夹、「选定此工作区」；
  无 JS 时回退整页 `/workspaces?bind=<thread_id>`，「绑定到该会话」）。
  有第一条消息后这些控件消失（整页跳回时不再渲染）；
- 搜索框同时过滤会话标题与工作区**名称及完整路径**（同名目录靠路径区分），工作区命中时整组展开。

`/workspaces` 是添加/选择工作区的目录浏览器：左侧是磁盘、主目录与最近工作区，右侧浏览子目录，可新建文件夹，最后「使用此目录」把该目录设为当前工作区并打开/新建会话。

| 路由 | 作用 |
|---|---|
| `GET /workspaces?path=…` | 目录浏览器（走网关 `fs.roots` / `fs.list`，不自己读盘） |
| `POST /workspaces/select` | `workspace.add`（设当前项目 + 记入会话目录索引）→ 打开该工作区最近的会话，没有则 `thread.create` |
| `POST /workspaces/mkdir` | 在浏览位置新建目录（`fs.mkdir`），成功后进入它 |

两条实现约束：

- **工作区是会话属性**：切换不需要重连网关、也不换 agent；新会话通过 `thread.create { workspace }` 绑定目录；
- **打开历史会话前先 `thread.focus`**：快照只给"激活 / 打开的 Tab / 运行中"会话带 items，不聚焦直接渲染会得到空正文。

当前**不做移除工作区**：后端 `workspace.remove` 会连同该工作区的会话文件一起删除，Web 端先不提供这个破坏性入口。

## 运行

**生产/日常：直接跑网关**（默认 `--web-ui ssr`）：

```bash
cargo run -p agent-gateway -- --port 52353 --workspace E:/codes/your-project
# 浏览器打开 http://127.0.0.1:52353/
```

相关参数：

| 参数 | 作用 |
|---|---|
| `--web-ui ssr` | 内置 SSR 界面（默认） |
| `--web-ui desktop` | 回退到 `desktop-ui/dist` 静态产物 |
| `--web-token <t>` | 浏览器访问令牌；非回环监听时可与 `--token` 二选一 |
| `--web-plugins-dir <p>` | 前端插件目录（默认 `~/.a-da/web-plugins` → exe 旁 → 源码目录） |
| `A_DA_WEB_UI` / `A_DA_WEB_TOKEN` / `A_DA_WEB_PLUGINS_DIR` | 上述三项的环境变量形式 |

**开发/验收：单独跑界面层**（仍需要一个已启动的网关）：

```bash
cargo run -p agent-web --bin ssr-h -- \
  --host 127.0.0.1 --port 8788 \
  --gateway http://127.0.0.1:52353 \
  --token <gateway token> \
  --workspace E:/codes/your-project \
  --plugins-dir crates/agent-web/frontend-plugins
```

未显式给 `--gateway/--token/--workspace` 时会尝试读取 `~/.a-da/gateway_endpoint.json`。
发现文件里的 `workspace` 只在"目录真实存在且不是 `~/.a-da`"时采用，否则回退到当前目录
（避免把凭据目录当成会话工作区）。

🔴 **插件目录与状态分开**：停用状态写在 `<A_DA_HOME>/web-ui-state.json`，不再写插件目录旁。
插件目录按“用户目录 → exe 旁 → 当前目录 → 源码目录”查找；打包后随 `resources/bin/frontend-plugins`
分发，源码开发时用 `crates/agent-web/frontend-plugins`。改了源码插件后，重载读的是**当前插件目录**；
必要时用 `--web-plugins-dir` 显式指过去。

### 局域网访问与访问令牌

网关默认只允许绑定回环地址。要开放给局域网，必须**同时**显式确认明文并配置凭据：

```bash
ada-gateway --host 0.0.0.0 --allow-plaintext \
  --token <网关 token> \
  --workspace E:/codes/your-project
```

浏览器打开 `http://<host>:52353/`，在 `/__auth` 输入网关 token 或启动日志里的配对码；
服务端校验后换发 `SameSite=Strict` 的 HttpOnly **会话 cookie**（随机 id，网关 token 不进 cookie），
并 303 去掉 URL 里的 `?token=`。`--web-token` 是等价的静态令牌兼容项。
只开明文 HTTP，令牌会以明文传输，请只在可信网络使用。

入口防护（`src/guard.rs`）做了三件事：

- **Host 白名单**：只接受绑定的主机名 + 回环别名，挡 DNS rebinding；
- **Origin / Sec-Fetch-Site**：跨站写请求直接 403，挡 CSRF；
- **安全响应头**：`CSP`（`frame-ancestors 'none'`）、`X-Frame-Options: DENY`、`nosniff`、`Referrer-Policy: same-origin`。
  🔴 **不能用 `no-referrer`**：Chromium 在它下面会给**表单 POST**（导航请求）发 `Origin: null`，
  同源校验会把所有动作按钮一起拒掉——现象是"点新会话 / 发送 / 审批全是 403 跨站请求"，
  而 `fetch()` 却正常（照常带真实 Origin），极易误判。现在 `Origin: null` 仅当
  `Sec-Fetch-Site: same-origin`（浏览器禁改头）时按同源导航放行，其余照旧拒绝。

🔴 **工作区选择器会浏览网关所在机器的文件系统**（`/workspaces` 走 `gateway.fs.*`）。
非回环监听时必须配置浏览器凭据（网关 `--token`、配对码或 `--web-token`），
并且只发给可信用户——拿到凭据的人可以浏览宿主目录。

### 流式验收

```bash
node tools/verify-ssr-streaming.mjs
node tools/verify-ssr-approval-abort.mjs   # 审批托盘 → 批准执行 → 流式中止
```

假模型（每 120ms 吐一段）+ 真网关（**单进程内置 SSR**）+ 真 `ada-coding`，走真实的
`/threads/{id}/send` 与 `/threads/{id}/stream`，断言助手正文被拆成多次增量推送、
相邻间隔远小于旧的 2 秒整段刷新、正文单调变长、最后一段不丢。默认取 debug 网关，
可用 `A_DA_GATEWAY_EXE` 指向 `dist/ada-gateway.exe`。
脚本的临时目录都在 `target/ssr-verify/` 下（某些沙箱环境不允许子进程写系统 TEMP）。

### 工作区验收

```bash
node tools/verify-ssr-workspaces.mjs
```

真网关（单进程内置 SSR）+ 真 `ada-coding`：建两个工作区的会话，断言侧栏树分组/当前标记/跨工作区可见，
再走 `/workspaces` 浏览目录、`workspaces/select` 选新目录（无会话则建一条）、选已有工作区（打开已有会话），
最后用 `workspaces/mkdir` 新建目录。临时目录在 `target/ssr-verify-workspaces/`。

## 供应商管理

供应商管理由前端插件 **`provider-admin`** 提供，页面在 `/ext/provider-admin/providers`；
原核心入口 `/providers` 只做 303 跳到插件页，不再渲染旧管理页。插件页是 Master-Detail：
左列选供应商，右列用两个页签分别改**供应商信息**（连接参数）与**模型列表**。页签是真实链接
（`?provider=…&tab=models`），保存/新增/删除模型后回到插件页。能力与 desktop-ui 对齐：

| 能力 | 路由 | 说明 |
|---|---|---|
| 新建 | `POST /providers/new` | 建一个带默认模型的空壳，再填端点与密钥 |
| 保存连接 | `POST /providers/{id}` | 名称 / 协议 / Base URL / API Key / 代理 / 自定义请求头；勾“设为当前供应商”即同时激活 |
| 设为当前 | `POST /providers/{id}/activate` | 可带 `model` 切到指定模型 |
| 删除 | `POST /providers/{id}/delete` | 至少保留一个（后端也会拒） |
| 加模型 | `POST /providers/{id}/models` | 表格上方的「新增模型」弹窗提交到这里 |
| 改 / 删模型 | `POST /providers/{id}/models/{index}`、`.../{index}/delete` | 编辑走「编辑」弹窗；用**下标**寻址，因为模型 id 可能含 `/` |
| 获取远程模型 | `POST /providers/{id}/models/fetch` | htmx 片段；真的打端点的 `/models`，用表单里的当前值（密钥留空＝用已存的） |
| 测试连接 | `POST /providers/{id}/test` | htmx 片段；调 `config.checkProvider`，**后端目前只做参数校验、不发起真实请求**，想验可达性用“获取远程模型” |

两条实现约束：

- **每次动作都"读整条 → 改字段 → 整条写回"**：`provider.save` 是"存在即替换"，只发改动字段会把模型列表写成空；
- **API Key 不回显**：插件页只有 `已配置 + sk-…7f3a`，输入框留空＝不修改，勾“清空密钥”才真的清空；
  API Key 输入框直接提交核心，不进入 Boa。
  这是与 desktop-ui 唯一的有意差异。

## 动态前端插件

**插件只接受 TSX**：`.slang` / `.html` 模板已移除。目录结构：

```text
frontend-plugins/hello-tsx/
  plugin.json             # 清单：导航 / 页面 / 仪表盘 / 插槽
  page.tsx                # 独立页面
  dashboard.tsx           # 仪表盘卡片（可选）
  slot.tsx                # 插槽（可选）
  components/card.tsx     # 相对 import 的组件（可选）
  static/                 # 插件静态资源（可选）
  static/hello.css        # 插件自定义样式（由 manifest styles 注入）
  static/hello.js         # 插件自定义脚本（由 manifest scripts 注入，defer）
```

`plugin.json`：

```json
{
  "id": "hello-tsx",
  "name": "Hello TSX",
  "version": "0.1.0",
  "description": "演示 TSX 插件",
  "nav": [{ "label": "Hello TSX", "slug": "hello" }],
  "pages": [{ "slug": "hello", "title": "Hello TSX", "template": "page.tsx" }],
  "dashboard": "dashboard.tsx",
  "assets": "static",
  "styles": ["hello.css"],
  "scripts": ["hello.js"],
  "slots": [
    { "slot": "composer.left", "template": "slot.tsx", "order": 20 }
  ],
  "api": [
    { "method": "get", "path": "info", "template": "api/info.tsx" }
  ]
}
```

- 访问 `/extensions` 点「重载前端插件」即可重新扫描目录；新插件立即出现在导航与仪表盘，无需重启。
- 独立页面地址：`/ext/{id}/{slug}`；资源地址：`/ext/{id}/assets/{path}`。
- **自定义 CSS**：把 CSS 放进 assets 目录，在 `plugin.json` 的 `styles` 里列相对路径；
  服务端会把 `<link rel="stylesheet" href="/ext/{id}/assets/...?v=指纹">` 注入所有页面 `<head>`，
  并把 CSS 包进 `@scope ([data-plugin-scope="{id}"])`，**只作用于该插件内容**，
  不会污染应用壳或其他插件。`styles` 只接受相对路径 `.css`（不能 `..` / 绝对路径），
  CSS 变化会随插件指纹刷新缓存。例如 `{ "assets": "static", "styles": ["hello.css"] }`
  对应 `static/hello.css`。插件内容外层有 `data-plugin-scope` 包裹层
  （`display: contents`，不改变布局）；浏览器不支持 `@scope` 时整段样式被忽略（安全降级）。
  请用插件内容 class 选择器，不要依赖 `:root`/`body`；`@import`/`@charset` 放在作用域内无效，
  `@keyframes` 名称仍建议加插件前缀。
- **自定义 JS**：`scripts` 字段与 `styles` 同口径（相对 assets 目录、只接受 `.js`），服务端把
  `<script src="/ext/{id}/assets/...?v=指纹" defer>` 注入所有页面 `<head>`。SSR 出来的插槽
  （`<details>` 开合、复制、偏好记忆）需要浏览器行为时用它；HTML 白名单**不开放 `<script>` 标签**。
- **插槽（slots）**：把 TSX 挂到界面的固定位置（`chat.corner` / `composer.left` / `composer.right` / `composer.footer` / `composer.above` / `thread.tray` / `sidebar.top` / `sidebar.bottom` / `topbar.right` / `transcript.after` / `extensions.list` / `dashboard.widgets`）。插槽内容在服务端渲染进 HTML；插件加载失败只影响它自己，界面退回默认实现。
  `chat.corner` 是会话区**右上角**的悬浮点位（`.ada-corner-dock`，只在聊天页存在）；
  `thread.tray` 是托盘区的**提问段**——输出非空时替换核心提问托盘（审批/排队不受影响），数据契约见 `data.questions`。
- `composer.above` 的插件（如 Token 天气）随 SSE `parts` 刷新：投影里带**当前会话**的助手 `usage`（不是快照的 `activeThreadId`），轮次收尾后会延迟补拉一次条目，所以不会停在「晴天 0%」。点击天气胶囊展开**浮层**（分档/百分比/已用/窗口/**缓存命中**/输出/会话/模型），不跳页；完整页面仍保留入口。token 数按 `M`/`K` 缩写、精确值在 `title` 里；缓存字段缺失时显示 `—` 而不是 `0%`。浮层是插件自己的 `<details class="ada-popover">`，`chat.js` 在 `parts` 替换后按 `id` 还原展开状态，并支持点外部/Esc 关闭。
- `chat.corner` 的插件（如任务卡片）随 `parts` 的 `corner` 字段刷新：`props.data.todo` 是核心推演好的当前计划
  （`done` / `total` / `active` / `notes` / `markdown` / `steps[].status`），没有计划时是 `null`。
  同一张卡片改挂 `composer.above` 也可用（见 `frontend-plugins/task-plan/README.md`），但不要两个都声明。
- 可在扩展页逐个**停用/启用**插件（`POST /extensions/{id}/disable|enable`），停用后贡献立即消失、界面回到默认实现；`GET /web-ui/frontend-plugins` 暴露插件与插槽状态（与网关的 `/api/plugins` 不是一回事）。
  停用状态**落盘在插件目录旁的 `.ssr-h-state.json`**（只记停用的 id），重启后仍然生效；
  文件坏了按全启用处理并在下次停用时重写，不会把插件锁死在停用态。
- **插件自带 JSON API**（`api` 声明）：端点在 `/ext/{id}/api/{path}`，TSX 入口函数
  **返回什么就序列化什么**（不走 JSX runtime）。动态扩展的口子是**一条静态分发路由**，
  端点查找走注册表——`/extensions` 点重载后新端点立即生效，不需要重启。
  请求信息在 `props.api`（method/path/query/body，POST 体必须是 JSON 且 ≤256KB）；
  方法白名单 GET/POST；`data` 只到 `plugin_count`（API 面不拖会话快照）。
  CSP 是 `connect-src 'self'`：插件浏览器 JS 正好可以 fetch 自己的这些同源端点
  （见 `hello-tsx` 的 `static/hello.js` 演示）。示例：`/ext/hello-tsx/api/info`。
- 插件只接受 `.tsx` 模板；插件代码是**本地受信任内容**，不要在不受信任的目录里放插件。

### 插件实现「置底/回到最新」按钮

**纯 TSX 插件做不了完整版**：插件在服务端渲染成 HTML，没有浏览器 JS、没有事件回调，也读不到滚动位置。核心提供了一份很薄的声明式契约，插件只写 HTML：

```tsx
// plugin.json 里挂到 "transcript.after" 插槽
export function Page(_props: any) {
  return (
    <button class="ada-scroll-bottom" type="button" data-ada-scroll-bottom>
      回到最新
    </button>
  );
}
```

- `data-ada-scroll-bottom` 由 `chat.js` 事件委托绑定：点击滚到底部 + 恢复自动跟随；
- 同一批按钮按"是否接近底部"自动切换 `hidden`（核心按钮也走这套）；
- 复用核心 `.ada-scroll-bottom` 类即可获得 sticky 居中样式（插件不需要额外 CSS/JS）。
- 插槽建议用 `transcript.after`（在滚动容器内）；放 `composer.above` 会落在右下角插件悬浮区。

如果坚持"零核心脚本"，还有一个纯 HTML 的退路：在 `transcript.after` 放一个底部锚点 `<a id="chat-end"></a>`，插件按钮用 `<a href="#chat-end">`。它能让浏览器自己滚动，但**无法在底部时隐藏**、会改 URL fragment，也不负责恢复 `stickToBottom`，所以不推荐作为正式交互。

### TSX 作者约定

```tsx
import { Button, Chip } from "ssr-h:components";

export function Page(props: any) {
  return (
    <div class="card">
      <h2>{props.plugin.name}</h2>
      <Chip text={"插件 " + props.data.plugin_count} tone="brand" />
      <Button label="打开对话" href="/" />
    </div>
  );
}
```

- 内置 **TSX 组件库**（`src/tsx/components.tsx`）提供 `Button` / `IconButton` / `Chip` /
  `Notice` / `Panel` / `Disclosure` / `EmptyState` / `Icon` / `TextField` / `Select` /
  `Modal` / `Form` / `Todo` / `UserMessage` / `AssistantMessage`；
  推荐 `import { Button } from "ssr-h:components"`，也兼容直接使用全局同名组件；
- 小写标签走 **标签 / 属性名 / URL** 三重校验（属性名必须是合法标识符，否则
  `{…{"data-x\" onmouseover=\"…"}}` 这种展开属性能凭空造事件属性）；
  `hx-*` 走显式白名单，`hx-on*` 与 `hx-vals`/`hx-headers`/`hx-trigger` 的 `js:` 值一律拒绝；
  文本默认转义；函数 prop / `onX` / `dangerouslySetInnerHTML` 拒绝；
  Boa 无 fs/net/process，并有循环/递归/输出/嵌套预算（**没有内存上限**，见 [docs/TSX.md](docs/TSX.md) §5）；
- **支持 import/export**：加载/重载时 OXC 递归转译相对模块，运行时交给 Boa 的 ESM loader；
  **不需要 Bun/Node**（只支持插件目录内的相对模块，npm 依赖不支持）；
- 每次渲染新建 Boa Context（模块状态不跨请求）；release 单文件页约 5ms、
  含内置组件库的模块页约 3.4ms（组件库每次 Context 都要初始化，后续可做池化/预编译）；
- **示例插件**：`frontend-plugins/hello-tsx/`（最小页面 / 仪表盘 / 插槽）、
  `frontend-plugins/token-weather/`（按上下文用量显示天气：完整页面 + 输入框上方胶囊/浮层 +
  概览卡片，演示相对 import、`details` 浮层与 SSE `parts` 刷新；说明见插件目录内的 README）、
  `frontend-plugins/task-plan/`（任务规划卡片：`chat.corner` 右上角 / `composer.above` 两处可挂，
  演示 `props.data.todo` 数据契约、插件 CSS 与 `scripts` 注入的浏览器增强）、
  `frontend-plugins/ask-user/`（提问卡片：`thread.tray` 插槽渲染待答复提问与作答表单，
  演示 `props.data.questions` 数据契约与"插件替换核心托盘、停用回退"的降级规则）；
  完整 TSX 说明见 [docs/TSX.md](docs/TSX.md)。
- **调试用 `console.log`**：TSX 在服务端跑，输出进 **ssr-h 进程日志**（`INFO ssr_h::plugin:
  [插件id/页面] …`），浏览器 DevTools 里看不到；Boa 本身没有 `console`，这是内置的
  最小实现（单次渲染最多 20 条）。改完模板要在 `/extensions` 点「重载前端插件」才生效。


## 组件库

- **应用壳**仍用 Rust 组件（`ui/`）渲染；聊天页模板只摆放。
- **插件 TSX** 的组件库在 `src/tsx/components.tsx`：组件就是 TSX 函数，输出 `ada-*` class；
  Rust 侧只保留 `Icon` SVG 原语与 HTML 白名单，不再实现插件业务组件。
- 样式按层拆在 `static/{components,layout,widgets,shell,pages}.css`（引用顺序固定）；
  焦点态统一为品牌描边 + 3px 光环；图标唯一来源仍是 `ui/icons.rs`。
- 详见 [docs/COMPONENTS.md](docs/COMPONENTS.md)。

## 当前实现边界

- 这是**浏览器访问面**（网关默认挂载），不是 Electron 桌面壳的替代：桌面端仍加载本地 `desktop-ui/dist`；
  浏览器页面由 SSE 增量更新（断流时退回 2 秒轮询），动作完成后整体跳转；
- 已实现：只读概览、会话列表/详情、发送、中止、重试、审批、提问作答（核心托盘 + `thread.tray` 插件可接管）、
  排队（提升/删除/清空）、任务规划（task-plan 插件）、
  **供应商管理**（新建 / 编辑连接 / 删除 / 设为当前 / 测试连接 / 获取远程模型 / 模型增删改，对齐 desktop-ui）、
  插件重载与启停、**TSX 服务端插件**（`.tsx` 页面/插槽/仪表盘，OXC + Boa + 内置 TSX 组件库）；
- 核心页面壳仍是 Askama 编译期模板；TSX 只覆盖插件扩展面，不替代整页布局与数据获取；
- 侧栏会话树随 SSE `parts` / 兜底轮询增量刷新；工作区切换器与系统导航仍是整页加载快照；
- **图片输入已支持**：输入框左侧「添加图片」/ 粘贴 / 拖入输入卡 → 预览与逐张移除 → 随消息一起提交
  （data URI；PNG / JPEG / WebP / GIF，单张 ≤8 MB、最多 4 张，服务端再校验一次）；对话流里渲染缩略图，
  **点开在当前页面浮层查看**（Esc / 点遮罩关闭，不开新标签）。纯图片消息可以不写文字。
  模式/审批档位仍只读（推理等级在模型菜单里切）；
- TSX 的相对 import 由 OXC + Boa 原生加载，不需要 Bun/Node；npm 依赖仍不支持；
  渲染有 wall-clock 预算（默认 500ms）与循环/递归/输出预算，但没有内存上限；
- 浏览器鉴权：网关挂载时接受网关 token / 配对码并换发 HttpOnly 会话 cookie；
  独立运行时用 `--ui-token`。两种形态都只提供明文 HTTP，远程部署请交给反向代理终止 TLS。




