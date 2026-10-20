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
- 对话区每 3 秒通过 HTMX 拉取 `/threads/{id}/partial`，发送消息仍走服务端 WS RPC 后整页跳回。
- 输入框：Enter 发送、Shift+Enter 换行、随内容自动增高；代码块悬停有复制按钮；思考/工具折叠在 HTMX 刷新后保持展开状态；只有接近底部时才自动跟随新内容。
- 任务规划（`todo` 工具）悬浮在会话区右上角，默认收成胶囊药丸（进度 + 当前步骤），展开是步骤清单卡片；计划按 `set`/`update`/`get` 的动作推演，不是倒序取最后一张卡片。
- 对话流：工具与思考采用 desktop-ui 的单行日志式——单行显示图标、中文标签、目标对象与真实耗时，展开后是左侧导轨线里的参数/输出面板；工具输出按 Markdown 渲染（JSON 走等宽原文）。
- 侧栏支持按标题搜索会话；插件 / 供应商 / 概览 / 插件页也共用同一套侧栏外壳，不再切回旧布局。
- 图标统一为 14–16px 线性 SVG（替代早期字符符号）；工具/思考用单行日志式展示；输入框为桌面端形态（大圆角输入卡 + 底部工具行：工作区/审批档位/模型 + 近黑圆形发送键）；窄窗口（≤760px）侧栏收为抽屉，顶栏出汉堡按钮；中止等危险操作用共享确认对话框（原生 `<dialog>`）。
- 审批：`waiting_approval` 工具在输入框上方托盘展示命令/参数与等待时长，可批准/拒绝。
- 排队：当前会话的排队消息可见，可提升、删除、清空。
- 失败：`notice.level=error` 的卡片带「重试这一轮」。
- 网关断开时保留界面壳并显示「未连接」+ 重新连接，不直接 502。

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

- 组件在 **Rust 侧渲染**（`src/ui/`），模板只摆放、不再逐个元素拼 class；样式统一在 `static/components.css`（唯一一份页面级 CSS）。
- 焦点态全库统一：品牌描边 + 3px 光环；输入框聚焦时额外显示提示文案。
- 图标只在 `src/ui/icons.rs` 定义一次，模板通过注入的 sprite `<use>` 引用。
- 详见 [docs/COMPONENTS.md](docs/COMPONENTS.md)。

## 当前实现边界

- 这是 SSR/HTMX 原型，不是现有 desktop-ui 的替代：聊天发送通过服务端 WS RPC，页面按轮询/重新加载更新；
- 只实现只读概览、会话列表/详情、发送消息、中止、供应商查看、插件重载；
- 不做登录鉴权，只应绑定回环地址。
