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
| `static/app.css` | 由 Tailwind 从 `templates/**/*.html` 扫描生成 |
| `static/htmx.min.js` | htmx 2.0.4 |
| `static/alpine.min.js` | Alpine.js 3.14.8 |

`app.rs` 用 `include_bytes!` 把这三个文件嵌进二进制，`/static/*` 由同一进程提供；部署时不需要额外带 `static/` 目录。

模板有改动后重新生成 Tailwind：

```bash
bun desktop-ui/node_modules/tailwindcss/lib/cli.js \
  -c crates/plugin-hub/tailwind.config.cjs \
  -i crates/plugin-hub/static/input.css \
  -o crates/plugin-hub/static/app.css --minify
```

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