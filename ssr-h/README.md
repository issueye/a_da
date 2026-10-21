# ssr-h — a_da 的 SSR Agent 前端实验版

> 独立目录、独立 crate、独立端口；**不加入根 workspace**，不影响现有 Electron / desktop-ui / gateway。

UED 设计基线、组件规范与 desktop-ui 对齐差距见 [docs/UED.md](docs/UED.md)。
组件库与插件插槽体系的架构说明见 [docs/COMPONENTS.md](docs/COMPONENTS.md)。

## 目标

- 参考 `crates/plugin-hub` 的前端框架：Axum MVC + Askama SSR + HTMX/Alpine + 本地 Tailwind CSS。
- 以 **动态前端插件** 为核心：`frontend-plugins/<id>/plugin.json` 可在运行时扫描、重载，新增导航、仪表盘卡片、独立页面与静态资源，不需要重新编译或重启 `ssr-h`。
- Agent 数据通过现有网关的 `/api/*`（HTTP）与 `/rpc`（WebSocket JSON-RPC）获取；本程序不修改任何现有后端协议。

## 主界面

- `/`：截图式 Agent 界面——左侧会话栏（新会话、导航、工作区、会话列表、前端插件）、中间对话流、底部输入框。
- `/threads/{id}`：同一界面定位到指定会话；`/overview` 保留原来的统计概览。
- 助手正文用 `pulldown-cmark` 渲染 Markdown，再经 `ammonia` 白名单清洗后输出；工具/思考折叠展示。
- 页面每 2 秒通过 HTMX 拉取 `/threads/{id}/live`，用 `hx-swap-oob` 一次更新**对话流、审批/排队托盘、任务规划、顶栏状态、输入框辅助区**；发送消息仍走服务端 WS RPC 后整页跳回。
- 输入框：Enter 发送、Shift+Enter 换行、随内容自动增高；代码块悬停有复制按钮；思考/工具折叠按 item id 在轮询替换后保持展开状态；只有接近底部时才自动跟随新内容。
- 任务规划（`todo` 工具）悬浮在会话区右上角，默认收成胶囊药丸（进度 + 当前步骤），展开是步骤清单卡片；计划按 `set`/`update`/`get` 的动作推演，不是倒序取最后一张卡片。
- 对话流：工具与思考采用 desktop-ui 的单行日志式——单行显示图标、中文标签、目标对象与真实耗时，展开后是左侧导轨线里的参数/输出面板；工具输出按 Markdown 渲染（JSON 走等宽原文）。
- 侧栏支持按标题搜索会话；插件 / 供应商 / 概览 / 插件页也共用同一套侧栏外壳，不再切回旧布局。
- 图标统一为 14–16px 线性 SVG（替代早期字符符号）；工具/思考用单行日志式展示；输入框为桌面端形态（大圆角输入卡 + 底部工具行：工作区/审批档位/模型 + 近黑圆形发送键）；窄窗口（≤760px）侧栏收为抽屉，顶栏出汉堡按钮；中止等危险操作用共享确认对话框（原生 `<dialog>`）。
- 审批：`waiting_approval` 工具在输入框上方托盘展示命令/参数与等待时长，可批准/拒绝；托盘随 live 轮询出现/消失，不需要手工刷新。
- 排队：当前会话的排队消息可见，可提升、删除、清空（提升会中断当前回答，已加确认）。
- 失败：`notice.level=error` 的卡片带「重试这一轮」；动作失败会回到会话页用 flash 说明原因，不再把整页换成裸错误页。
- 聊天页在网关断开时保留界面壳并显示「未连接」，已打开会话的 live 轮询会继续探测并在恢复后自动接回；首屏就未连接时请点「重新连接」；`/overview`、`/providers` 等二级页仍会直接显示错误页。

## 运行

先启动现有网关/桌面端（默认 `127.0.0.1:52353`），然后：

```bash
cargo run --manifest-path ssr-h/Cargo.toml
# 打开 http://127.0.0.1:8788
```

可选参数：

```bash
cargo run --manifest-path ssr-h/Cargo.toml -- \
  --host 127.0.0.1 --port 8788 \
  --gateway http://127.0.0.1:52353 \
  --token <gateway token> \
  --workspace E:/codes/your-project \
  --plugins-dir ssr-h/frontend-plugins
```

未显式给 `--gateway/--token/--workspace` 时会尝试读取 `~/.a-da/gateway_endpoint.json`。
发现文件里的 `workspace` 只在"目录真实存在且不是 `~/.a-da`"时采用，否则回退到当前目录
（避免把凭据目录当成会话工作区）。

### 局域网访问与访问令牌

默认只允许绑定回环地址。要开放给局域网，必须**同时**显式确认并设置访问令牌：

```bash
cargo run --manifest-path ssr-h/Cargo.toml -- \
  --host 0.0.0.0 --port 8788 --allow-remote \
  --ui-token <至少 12 位的 ASCII 令牌> \
  --gateway http://127.0.0.1:52353
```

进入方式：浏览器打开 `http://<host>:8788/?token=<ui-token>`，服务端会换发
`SameSite=Strict` 的 HttpOnly cookie 并 303 去掉 URL 里的 token；之后也可以手动在
`/__auth` 页面输入。只开明文 HTTP，令牌会以明文传输，请只在可信网络使用。

入口防护（`src/guard.rs`）做了三件事：

- **Host 白名单**：只接受绑定的主机名 + 回环别名，挡 DNS rebinding；
- **Origin / Sec-Fetch-Site**：跨站写请求直接 403，挡 CSRF；
- **安全响应头**：`CSP`（`frame-ancestors 'none'`）、`X-Frame-Options: DENY`、`nosniff`、`no-referrer`。

## 动态前端插件

目录结构：

```text
ssr-h/frontend-plugins/hello/
  plugin.json             # 清单：导航 / 页面 / 仪表盘 / 插槽
  composer.html           # 插槽片段（可选）
  dashboard.html          # 仪表盘卡片（可选）
  page.html               # 独立页面内容（可选）
  static/hello.js         # 插件静态资源（可选）
```

`plugin.json`：

```json
{
  "id": "hello",
  "name": "Hello Frontend Plugin",
  "version": "0.2.0",
  "description": "演示动态前端插件",
  "nav": [{ "label": "Hello 插件", "slug": "hello" }],
  "pages": [{ "slug": "hello", "title": "Hello 插件", "template": "page.html" }],
  "dashboard": "dashboard.html",
  "assets": "static",
  "slots": [
    { "slot": "composer.left", "template": "composer.html", "order": 20 }
  ]
}
```

- 访问 `/extensions` 点「重载前端插件」即可重新扫描目录；新插件立即出现在导航与仪表盘，无需重启。
- 独立页面地址：`/ext/{id}/{slug}`；资源地址：`/ext/{id}/assets/{path}`。
- **插槽（slots）**：把片段挂到界面的固定位置（`composer.left` / `composer.right` / `composer.footer` / `composer.above` / `sidebar.top` / `sidebar.bottom` / `topbar.right` / `transcript.after` / `extensions.list` / `dashboard.widgets`）。插槽内容在服务端渲染进 HTML；插件加载失败只影响它自己，界面退回默认实现。
- 可在扩展页逐个**停用/启用**插件（`POST /extensions/{id}/disable|enable`），停用后贡献立即消失、界面回到默认实现；`GET /api/plugins` 暴露插件与插槽状态。
- 插件 HTML 是**本地受信任内容**，以原样片段插入布局；不要在不受信任的目录里放插件。后续可加 iframe/沙箱。

## 组件库

- 组件在 **Rust 侧渲染**（`src/ui/`），聊天页模板只摆放；样式在 `static/components.css`。
  二级页（插件/供应商/概览）仍保留 Tailwind 时代的 `btn/card/badge` 标记，两套视觉尚未完全收口。
- 焦点态全库统一：品牌描边 + 3px 光环；输入框聚焦时额外显示提示文案。
- 组件图标在 `src/ui/icons.rs` 定义；部分模板（侧栏/顶栏/插件页）仍有内联 SVG，待后续收口到 sprite。
- 详见 [docs/COMPONENTS.md](docs/COMPONENTS.md)。

## 当前实现边界

- 这是 SSR/HTMX 原型，不是现有 desktop-ui 的替代：聊天发送通过服务端 WS RPC，页面按轮询/重新加载更新；
- 已实现：只读概览、会话列表/详情、发送、中止、重试、审批、排队（提升/删除/清空）、任务规划、供应商查看、插件重载与启停；
- 侧栏会话列表仍是页面加载时的快照（顶栏运行状态由 live 更新）；新会话/其它客户端的变更需要刷新；
- 图片会显示"含 N 张图片"标记但**不渲染图片内容**；模式/审批档位/模型仍只读；
- 不做登录鉴权；非回环监听必须显式 `--allow-remote` + `--ui-token`，且只提供明文 HTTP。
