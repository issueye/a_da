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
- 输入框：Enter 发送、Shift+Enter 换行、随内容自动增高；代码块悬停有复制按钮；思考/工具折叠按 item id 在轮询替换后保持展开状态，并还原面板内部的滚动位置（否则展开的工具每 2 秒自己滚回顶部）；只有接近底部时才自动跟随新内容。
- 任务规划（`todo` 工具）悬浮在会话区右上角，默认收成胶囊药丸（进度 + 当前步骤），展开是步骤清单卡片；计划按 `set`/`update`/`get` 的动作推演，不是倒序取最后一张卡片。
- 对话流：工具与思考采用 desktop-ui 的单行日志式——单行显示图标、中文标签、目标对象与真实耗时，展开后是左侧导轨线里的参数/输出面板；工具输出按 Markdown 渲染（JSON 走等宽原文）。
- 侧栏支持按标题搜索会话；插件 / 供应商 / 概览 / 插件页也共用同一套侧栏外壳，不再切回旧布局。
- 图标统一为 14–16px 线性 SVG（替代早期字符符号）；工具/思考用单行日志式展示；输入框为桌面端形态（大圆角输入卡 + 底部工具行：工作区/审批档位/模型 + 近黑圆形发送键）；窄窗口（≤760px）侧栏收为抽屉，顶栏出汉堡按钮；**宽屏下同一个按钮是"收起/展开侧栏"**（状态存 `localStorage`，跨刷新保持）；中止等危险操作用共享确认对话框（原生 `<dialog>`）。
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
- **安全响应头**：`CSP`（`frame-ancestors 'none'`）、`X-Frame-Options: DENY`、`nosniff`、`Referrer-Policy: same-origin`。
  🔴 **不能用 `no-referrer`**：Chromium 在它下面会给**表单 POST**（导航请求）发 `Origin: null`，
  同源校验会把所有动作按钮一起拒掉——现象是"点新会话 / 发送 / 审批全是 403 跨站请求"，
  而 `fetch()` 却正常（照常带真实 Origin），极易误判。现在 `Origin: null` 仅当
  `Sec-Fetch-Site: same-origin`（浏览器禁改头）时按同源导航放行，其余照旧拒绝。

## 供应商管理

`/providers` 是 Master-Detail：左列选供应商，右列用两个页签分别改**供应商信息**（连接参数）
与**模型列表**。页签是真实链接（`?provider=…&tab=models`）：无 JS 时整页刷新到该页签，
有 JS 时就地切换；保存/新增/删除模型后回跳到发起动作的那个页签。能力与 desktop-ui 对齐：

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
- **API Key 不回显**：页面只有 `已配置 + sk-…7f3a`，输入框留空＝不修改，勾“清空密钥”才真的清空。
  这是与 desktop-ui 唯一的有意差异。

## 动态前端插件

**插件只接受 TSX**：`.slang` / `.html` 模板已移除。目录结构：

```text
ssr-h/frontend-plugins/hello-tsx/
  plugin.json             # 清单：导航 / 页面 / 仪表盘 / 插槽
  page.tsx                # 独立页面
  dashboard.tsx           # 仪表盘卡片（可选）
  slot.tsx                # 插槽（可选）
  components/card.tsx     # 相对 import 的组件（可选）
  static/                 # 插件静态资源（可选）
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
  "slots": [
    { "slot": "composer.left", "template": "slot.tsx", "order": 20 }
  ]
}
```

- 访问 `/extensions` 点「重载前端插件」即可重新扫描目录；新插件立即出现在导航与仪表盘，无需重启。
- 独立页面地址：`/ext/{id}/{slug}`；资源地址：`/ext/{id}/assets/{path}`。
- **插槽（slots）**：把 TSX 挂到界面的固定位置（`composer.left` / `composer.right` / `composer.footer` / `composer.above` / `sidebar.top` / `sidebar.bottom` / `topbar.right` / `transcript.after` / `extensions.list` / `dashboard.widgets`）。插槽内容在服务端渲染进 HTML；插件加载失败只影响它自己，界面退回默认实现。
- 可在扩展页逐个**停用/启用**插件（`POST /extensions/{id}/disable|enable`），停用后贡献立即消失、界面回到默认实现；`GET /api/plugins` 暴露插件与插槽状态。
- 插件只接受 `.tsx` 模板；插件代码是**本地受信任内容**，不要在不受信任的目录里放插件。

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
- 小写标签走 HTML 白名单与 URL 校验；文本默认转义；函数 prop / `onX` /
  `dangerouslySetInnerHTML` 拒绝；Boa 无 fs/net/process，并有循环/递归/输出/嵌套预算；
- **支持 import/export**：加载/重载时 OXC 递归转译相对模块，运行时交给 Boa 的 ESM loader；
  **不需要 Bun/Node**（只支持插件目录内的相对模块，npm 依赖不支持）；
- 每次渲染新建 Boa Context（模块状态不跨请求）；release 单文件页约 5ms、
  含内置组件库的模块页约 3.4ms（组件库每次 Context 都要初始化，后续可做池化/预编译）；
- 示例：`frontend-plugins/hello-tsx/`；完整说明见 [docs/TSX.md](docs/TSX.md)。


## 组件库

- **应用壳**仍用 Rust 组件（`src/ui/`）渲染；聊天页模板只摆放。
- **插件 TSX** 的组件库在 `src/tsx/components.tsx`：组件就是 TSX 函数，输出 `ada-*` class；
  Rust 侧只保留 `Icon` SVG 原语与 HTML 白名单，不再实现插件业务组件。
- 样式按层拆在 `static/{components,layout,widgets,shell,pages}.css`（引用顺序固定）；
  焦点态统一为品牌描边 + 3px 光环；图标唯一来源仍是 `src/ui/icons.rs`。
- 详见 [docs/COMPONENTS.md](docs/COMPONENTS.md)。

## 当前实现边界

- 这是 SSR/HTMX 原型，不是现有 desktop-ui 的替代：聊天发送通过服务端 WS RPC，页面按轮询/重新加载更新；
- 已实现：只读概览、会话列表/详情、发送、中止、重试、审批、排队（提升/删除/清空）、任务规划、
  **供应商管理**（新建 / 编辑连接 / 删除 / 设为当前 / 测试连接 / 获取远程模型 / 模型增删改，对齐 desktop-ui）、
  插件重载与启停、**TSX 服务端插件**（`.tsx` 页面/插槽/仪表盘，OXC + Boa + 内置 TSX 组件库）；
- 核心页面壳仍是 Askama 编译期模板；TSX 只覆盖插件扩展面，不替代整页布局与数据获取；
- 侧栏会话列表仍是页面加载时的快照（顶栏运行状态由 live 更新）；新会话/其它客户端的变更需要刷新；
- 图片会显示"含 N 张图片"标记但**不渲染图片内容**；模式/审批档位/模型仍只读；
- TSX 的相对 import 由 OXC + Boa 原生加载，不需要 Bun/Node；npm 依赖仍不支持，
  且没有 wall-clock 超时（只有循环/递归/输出预算）；
- 不做登录鉴权；非回环监听必须显式 `--allow-remote` + `--ui-token`，且只提供明文 HTTP。
