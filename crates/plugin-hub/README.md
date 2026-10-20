# a_da 插件库平台（Plugin Hub）

一个类似 GitHub 的**插件库管理平台**：用户/组织协作、创建插件仓库、维护 README 与标签、
发布带 ZIP 附件的版本、搜索/浏览、Star、下载统计，并提供 **GitHub OAuth**、**API Token** 与 **Webhook**。

- **后端**：Rust + Axum 0.8 + SQLx 0.8（SQLite）
- **前端**：Askama SSR + **本地静态** HTMX / Alpine.js / Tailwind CSS
- **结构**：MVC —— `controllers/`（HTTP 编排）、`models/`（SQLx 查询）、`views/` + `templates/`（Askama）

## 运行

```bash
# 开发
cargo run -p plugin-hub

# 发布
cargo build --release -p plugin-hub
./target/release/ada-plugin-hub
```

默认监听 `http://127.0.0.1:8787`。首次启动会自动创建 SQLite 库并执行 `migrations/`。

| 环境变量 | 默认值 | 说明 |
|---|---|---|
| `PLUGIN_HUB_ADDR` | `127.0.0.1:8787` | 监听地址 |
| `PLUGIN_HUB_DATABASE_URL` | `sqlite://data/plugin-hub.db` | SQLx SQLite URL |
| `PLUGIN_HUB_DATA_DIR` | `data/plugin-assets` | ZIP 附件根目录 |
| `PLUGIN_HUB_SECRET` | 开发默认值 | 签名 Cookie 密钥；**生产必须替换** |
| `PLUGIN_HUB_SECURE_COOKIES` | `false` | HTTPS/proxy 后设为 `true` |
| `PLUGIN_HUB_MAX_UPLOAD_MB` | `20` | 单个 ZIP 上传上限 |
| `PLUGIN_HUB_PUBLIC_URL` | `http://127.0.0.1:8787` | OAuth 回调拼接用的外部地址 |
| `PLUGIN_HUB_GITHUB_CLIENT_ID` | 无 | GitHub OAuth App Client ID |
| `PLUGIN_HUB_GITHUB_CLIENT_SECRET` | 无 | GitHub OAuth App Client Secret |
| `PLUGIN_HUB_GITHUB_API_BASE` | `https://api.github.com` | 测试/代理时可覆盖 |

> 升级提示：本次 schema 增加了组织、OAuth、Token、Webhook 表，并调整了 `plugins` 的所有者约束。
> 如果本地已有旧版 `data/plugin-hub.db`，请删除后重新启动（当前仍是开发期，未提供兼容迁移）。

## 本地静态资源

前端不依赖 CDN：

| 文件 | 说明 |
|---|---|
| `static/input.css` | Tailwind 源码 + 设计令牌 + 组件类（`@layer components`） |
| `static/app.css` | 由 Tailwind 从模板与 `src/**/*.rs` 扫描生成，**提交进仓库** |
| `static/theme.js` | 主题引导（阻塞加载）：`localStorage.a_da_theme` → 系统偏好 |
| `static/app.js` | 共享交互：导航高亮 / 提示条关闭 / 复制 / 确认对话框 |
| `static/htmx.min.js` | htmx 2.0.4 |
| `static/alpine.min.js` | Alpine.js 3.14.8 |

`app.rs` 用 `include_bytes!` 把这些文件嵌进二进制，`/static/*` 由同一进程提供；部署时不需要额外带 `static/` 目录。
错误页（`src/error.rs`）也走同一份 `app.css` / `theme.js`，所以 Tailwind 的 `content` 同时扫描 `src/**/*.rs`。

模板有改动后重新生成 Tailwind（`app.css` 是产物，必须一起提交）：

```bash
bun desktop-ui/node_modules/tailwindcss/lib/cli.js \
  -c crates/plugin-hub/tailwind.config.cjs \
  -i crates/plugin-hub/static/input.css \
  -o crates/plugin-hub/static/app.css --minify
```

## 界面与组件（设计系统）

视觉语言与桌面端 `desktop-ui/` 对齐：**zinc 中性色 + 蓝色主色**、1px 细边框、
`rounded-lg/xl/2xl` 圆角、极轻投影、紧凑字号（正文 12–14px、元信息 10–11px），
并支持**亮 / 暗双主题**。主题只切 `html.dark`，颜色通过 CSS 变量整体翻转，
模板里不要到处写 `dark:`。

### 设计令牌（`static/input.css` 顶部）

| 令牌 | 用途 |
|---|---|
| `canvas` / `surface` / `elevated` | 页面底色 / 卡片面 / 浮层（菜单、对话框） |
| `line` / `line-strong` | 分隔线与控件描边 |
| `ink` / `muted` / `faint` | 主文字 / 次要文字 / 弱提示（时间、计数） |
| `brand` / `brand-strong` / `brand-soft` / `brand-ink` | 主色、悬停色、淡底与淡底上的文字 |
| `subtle` / `subtle-ink` | 中性浅底（hover、表头、键帽） |
| `success` / `warn` / `danger`（`-soft` / `-ink`） | 状态色成对出现，暗色下自动换成透明底 |

Tailwind 里直接用语义类：`bg-surface`、`text-muted`、`border-line`、`bg-brand-soft`…

### 组件类（`@layer components`）

| 分类 | 类名 |
|---|---|
| 布局 | `app-shell`（纵向 flex + 页脚贴底）、`app-header`、`app-container`、`app-footer` |
| 品牌 / 导航 | `brand`、`brand-mark`、`brand-name`、`nav-link`、`nav-link-active` |
| 按钮 | `btn` + `btn-primary` / `btn-secondary` / `btn-ghost` / `btn-danger` / `btn-starred`，尺寸 `btn-sm` / `btn-xs` / `btn-block`，图标按钮 `icon-btn` |
| 卡片 | `card`、`card-hover`、`card-inset`、`panel-title` |
| 标题 | `page-title`、`page-sub`、`section-title`、`section-sub`、`section-head`、`link` |
| 徽章 / 标签 | `badge` + `badge-neutral/brand/success/warn/danger/mono`、`tag`、`kbd`、`avatar` |
| 表单 | `field`、`label`、`hint`、`input`、`textarea`、`select`、`checkbox-row`、`form-actions` |
| 反馈 | `alert` + `alert-success/danger/warn/info`、`empty-state`、`empty-icon`、`stat-tile` |
| 内容 | `table`、`code-block`、`code-inline`、`markdown-body` |
| 浮层 | `menu`、`menu-item`、`dialog-card`、`dialog-actions` |

### Askama 组件宏（`templates/partials/ui.html`）

页面顶部 `{% import "partials/ui.html" as ui %}`，再按需调用：

| 宏 | 说明 |
|---|---|
| `ui::icon(name, class)` | 24px 线性图标（与 desktop-ui 的 lucide 风格一致） |
| `ui::badge(text, tone)` | 状态徽章，tone：`neutral` / `brand` / `success` / `warn` / `danger` / `mono` |
| `ui::avatar(name, class)` | 首字母头像 |
| `ui::tag(tag)` | `#标签` 胶囊（链到搜索页） |
| `ui::alert(kind, title, message)` | 可关闭提示条（`success` / `danger` / `warn` / `info`） |
| `ui::empty(icon, title, hint, cta, href)` | 空态；`cta` 传空字符串表示不放按钮 |
| `ui::stat(value, label, icon)` | 统计小卡 |
| `ui::page_header(title, subtitle)` | 页面标题 + 说明 |
| `ui::plugin_card(p)` | 插件卡片（首页 / 浏览 / 作者页 / 组织页共用） |

约定：

- **叶子组件进宏，包内容的组件用类组合**：Askama 0.12 的宏没有 caller 内容块，
  需要包住任意内容的结构（卡片、面板、表单）不要硬塞进宏。
- 新增页面 = `{% extends "layout.html" %}` + `{% import "partials/ui.html" as ui %}`，
  优先复用上面的类与宏，不要再写一套 `bg-slate-*` 调色板。
- 破坏性操作在 `form` 上写 `data-confirm="确认文案"`，由 `static/app.js` 统一弹
  `<dialog>` 确认框（不用 `window.confirm`）。
- 复制按钮写 `data-copy="文本"` 或 `data-copy-target="#元素选择器"`，`app.js` 负责反馈。
- 主题切换按钮写 `data-theme-toggle`；`theme.js` 会同步 `aria-label` / `title` / `aria-pressed`。

## 路由

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/` | 首页：最近更新 + 下载最多 |
| GET | `/plugins` | 浏览/搜索（`q`、`sort`、`page`） |
| POST | `/plugins` | 创建插件（登录；可选 `org_id`） |
| GET | `/plugins/new` | 创建表单（登录） |
| GET | `/plugins/{owner}/{slug}` | 插件详情（README + 版本） |
| POST | `/plugins/{owner}/{slug}` | 更新插件（作者/组织管理员） |
| GET | `/plugins/{owner}/{slug}/edit` | 编辑表单 |
| POST | `/plugins/{owner}/{slug}/delete` | 删除插件 |
| GET/POST | `/plugins/{owner}/{slug}/releases/new` | 发布新版本（multipart） |
| GET | `/plugins/{owner}/{slug}/releases/{version}/download` | 下载 ZIP |
| POST | `/plugins/{owner}/{slug}/star` | Star 切换（HTMX 局部刷新） |
| GET | `/u/{username}` | 作者主页 |
| GET/POST | `/orgs` | 组织列表 / 创建组织 |
| GET | `/orgs/new` | 新建组织表单 |
| GET | `/orgs/{name}` | 组织主页（插件/成员/团队） |
| POST | `/orgs/{name}/members` | 添加成员（owner/admin） |
| POST | `/orgs/{name}/members/{user_id}/remove` | 移除成员（不能移除 owner） |
| POST | `/orgs/{name}/teams` | 创建团队（owner/admin） |
| POST | `/orgs/{name}/teams/{team_id}/members` | 加入团队成员 |
| GET | `/search?q=` | HTMX 搜索局部结果 |
| GET/POST | `/register`、`/login` | 注册 / 登录 |
| POST | `/logout` | 退出（CSRF） |
| GET | `/auth/github` | GitHub OAuth 开始 |
| GET | `/auth/github/callback` | GitHub OAuth 回调 |
| GET/POST | `/settings/tokens` | API Token 管理 |
| POST | `/settings/tokens/{id}/revoke` | 撤销 Token |
| GET/POST | `/settings/webhooks` | Webhook 管理 |
| POST | `/settings/webhooks/{id}/delete` | 删除 Webhook |
| GET | `/healthz` | 健康检查 |

### JSON API（API Token）

所有 `/api/v1/*` 请求使用 `Authorization: Bearer ada_pat_...`；Token 创建时只显示一次。

```bash
# 只读列表（scope=read）
curl -H "Authorization: Bearer ada_pat_..." http://127.0.0.1:8787/api/v1/plugins

# 创建插件（scope=write）
curl -X POST -H "Authorization: Bearer ada_pat_..." -H "Content-Type: application/json" \
  -d '{"slug":"demo","name":"Demo","description":"demo"}' \
  http://127.0.0.1:8787/api/v1/plugins

# 发布版本（scope=write，multipart）
curl -X POST -H "Authorization: Bearer ada_pat_..." \
  -F version=0.1.0 -F notes=first -F asset=@plugin.zip \
  http://127.0.0.1:8787/api/v1/plugins/alice/demo/releases
```

| 方法 | 路径 | 作用域 |
|---|---|---|
| GET | `/api/v1/plugins` | read |
| GET | `/api/v1/plugins/{owner}/{slug}` | read |
| POST | `/api/v1/plugins` | write |
| POST | `/api/v1/plugins/{owner}/{slug}/releases` | write |
| POST | `/api/v1/plugins/{owner}/{slug}/star` | write |

## 组织与团队

- 组织名与用户名共享命名空间（注册时互相查重），插件 URL 为 `/plugins/{owner}/{slug}`，`owner` 可以是用户或组织。
- 组织角色：`owner` / `admin` / `member`；owner/admin 可以管理成员与团队，member 可以发布插件。
- 团队是组织内的成员分组（`maintainer` / `member`），当前用于展示与后续权限扩展。
- 组织插件由组织所有；编辑/删除/发版本需要组织 owner/admin。

## GitHub OAuth

在 GitHub 创建 OAuth App，回调地址填：

```text
<PLUGIN_HUB_PUBLIC_URL>/auth/github/callback
```

然后配置 `PLUGIN_HUB_GITHUB_CLIENT_ID` / `PLUGIN_HUB_GITHUB_CLIENT_SECRET`。
登录页出现「使用 GitHub 登录」按钮；回调会读取 GitHub 用户与已验证邮箱。
OAuth 用户没有本地密码（`password_hash` 为占位标记），仍可用同一邮箱的既有账号自动关联。

## Webhook

在 `/settings/webhooks` 创建，支持事件：

- `plugin.created`
- `plugin.updated`
- `release.published`
- `star.created`（当前仅 API Star 触发）

投递为异步 POST JSON，请求头带：

```text
X-Plugin-Hub-Event: release.published
X-Plugin-Hub-Signature-256: sha256=<hex(hmac_sha256(secret, body))>
```

投递结果写入 `webhook_deliveries`，在页面「最近投递」查看。当前**不重试、不做队列**，失败只记录。

## 数据与安全

- **密码**：Argon2id 哈希；OAuth 用户使用不可解析的占位哈希。
- **会话**：签名 Cookie（HttpOnly + SameSite=Lax），不落服务端 Session 表。
- **CSRF**：登录后的写操作（创建/更新/删除/发布/Star/退出/组织/Token/Webhook）校验 `_csrf`。
- **Token**：只存 SHA-256 哈希，明文只在创建时显示一次；支持 `read` / `write` 作用域与撤销。
- **ZIP 校验**：不解压，只读 ZIP 目录；拒绝 `..`、绝对路径、反斜杠、符号链接、
  超过 500 个条目或解压后超过 50 MB 的包。
- **资产路径**：`data/plugin-assets/<owner>/<slug>/<version>.zip`，路径组件单独白名单校验。
- **README**：pulldown-cmark 渲染后经 ammonia 白名单清洗，模板用 `|safe` 输出。
- **错误页**：动态错误消息做 HTML 转义。

### 当前限制

- 单实例 SQLite + 本地文件存储，未做 S3/对象存储与多副本。
- OAuth 只接 GitHub；没有 GitLab/Google。
- 没有 Issue/PR/代码浏览/Webhook 重试队列。
- Webhook 密钥以明文存在本地 SQLite（仅 owner 页面可见）；生产建议接密钥管理。
- 登录/注册表单未加 CSRF（依赖 SameSite + 登录态变更的低风险面）；生产建议补匿名 CSRF Cookie。
- 团队目前是成员分组，尚未接入细粒度仓库权限。
- ZIP 只做结构校验，不执行插件、不做签名/供应链校验。

## 测试

```bash
cargo test -p plugin-hub -- --test-threads=1
```

覆盖：注册/登录、创建/编辑插件、组织与成员、发布/下载 ZIP、Star、搜索、静态资源、
API Token 创建与 JSON API、Webhook 创建、OAuth 未配置提示。