# ssr-h 合并进网关开发计划

| 项 | 值 |
|---|---|
| 日期 | 2026-10-25 |
| 输入 | [ssr-h/README.md](../../ssr-h/README.md)、`ssr-h/docs/{STATUS,UED,COMPONENTS,TSX}.md`、[gateway/README.md](README.md)、[protocol/README.md](../protocol/README.md)、本轮代码核查 |
| 性质 | **已落地（2026-10-25）**：M0–M4 全部完成；批次与任务卡保留为实施记录 |
| 目标 | 一个 `ada-gateway` 进程 + 一个端口 = 整站；SSR 成为网关的浏览器界面；删除 ssr-h 对网关的自反代 |
| 约束 | 不改多轮引擎与现有 JSON-RPC 方法；网关仍不跑引擎（INV-1）、不缓存会话状态（INV-8）；Electron 默认加载 SSR，`--ui=desktop` 保留 React/Vite 回退 |
| 节奏 | 一任务一 commit（`refactor(web)` / `feat(gateway)` / `docs(gateway)`），一批一出场，每批可独立回退 |
| 实施进度 | **已完成**：`crates/agent-web` 已并入 workspace；网关默认 `--web-ui ssr`；单进程验收脚本全绿；`verify-docs` 基线清零 |

> 注：§2 里的 `ssr-h/...` 是**合并前**的路径；当前落点见 §11 与 `crates/agent-web`。

## 0. 一页结论

ssr-h 已经是一个功能完整、可用的 SSR 前端（对话流、审批、提问、排队、工作区树、供应商管理、TSX 插件），
但它现在是**第二个进程、第二个端口、第二套鉴权**，并且所有数据都要绕一圈：
浏览器 → ssr-h → 网关 HTTP/WS → agent。合并的目标不是重写界面，而是把这条链路改成：

```text
浏览器 ──同源──▶ ada-gateway（SSR 路由 + /api + /rpc）──▶ 每工作区一个 agent
                    │
                    └── 进程内直接调用，不再 HTTP/WS 回环到自己
```

做法是**新增一个 UI 投影层 crate `agent-web`**，把 ssr-h 的代码平移进去，暴露一个可挂载的
`Router`；在 `agent-web` 里定义一个只有四个原语的 `WebBackend` 端口，网关提供一个进程内实现。
ssr-h 的页面与处理器基本不改，`AgentClient` 的方法签名也不改——改的只是"它底下怎么到达网关"。

三件事必须同时成立，合并才算成功：

1. **同源单端口**：`/`、`/threads/*`、`/static/*`、`/ext/*`、SSE、`/api/*`、`/rpc` 全在一个 axum Router 上；
2. **不再自反代**：一元调用走网关内部函数，事件订阅直连本工作区的 agent；网关 token 永不出现在浏览器；
3. **桌面端也可直接用**：Electron 等网关就绪后加载 `http://127.0.0.1:<port>/`（默认 SSR，
   窗口保持无边框，由 preload 在 SSR 顶栏注入窗口控制）；`--ui=desktop` 回退本地 React/Vite。
   网关仍只被当作一个 HTTP/WS 服务。

## 1. 目标与非目标

### 1.1 目标

- 删除 ssr-h 的独立监听与独立端口：不再需要 `ssr-h --gateway ... --token ...` 去连网关。
- 删除 ssr-h 到网关的自反代：查询与命令走进程内调用；状态事件直连 agent 端点。
- 保留 ssr-h 的全部现有能力与交互（见 §2.1），不降级为"只读预览"。
- 浏览器访问与 API 同源：SSE、表单 POST、`/rpc`、`/api/*` 共享一个 origin，不再有跨端口 CORS/Origin 问题。
- 保持网关的两条硬边界：不跑多轮循环（INV-1）、不缓存线程/消息/工具状态（INV-8）。
- 合并后仍可单独构建/运行 UI 层做开发与验收（过渡期保留 standalone 入口）。

### 1.2 非目标

- **不删除 Electron 的 desktop-ui**：SSR 是默认界面，React 界面作为 `--ui=desktop` 回退保留。
- **不把 plugin-hub 拉进来**：插件库平台是独立 Axum 服务，合并没有收益。
- **不重写 SSR 为 React/SPA**，也不把 Askama 模板改成运行时文件。
- **不新增 JSON-RPC 方法**：合并只动宿主与路由；若最终不得不加，按 §6.1 的四处同步执行。
- **不改 Electron 的网关监督逻辑**；界面加载路径只增加 `--ui=ssr|desktop` 选择（默认 SSR）。
- **不在本计划里统一 provider/plugin 的领域模型**：ssr-h 仍通过既有方法与 agent 交互。

## 2. 现状

### 2.1 ssr-h 是什么

| 维度 | 现状 | 证据 |
|---|---|---|
| 形态 | binary-only 独立 crate，自带 `[workspace]` 与 `Cargo.lock`，**不在根 workspace** | `ssr-h/Cargo.toml:1-9`、根 `Cargo.toml:1-26` |
| 体量 | `src` 约 1.95 万行（聊天 3.9k、插件注册表 1.6k、TSX 引擎 1.5k、处理器 4.5k、UI 组件 3.9k） | 本轮统计 |
| 入口 | 自己 bind、自己 `axum::serve`，默认 `127.0.0.1:8788` | `ssr-h/src/main.rs:180-189` |
| 路由 | 一个 `Router`，页面 + 表单动作 + SSE + 静态资源 + `/ext/*` + 自己的一套 `/api/plugins` | `ssr-h/src/main.rs:193-315` |
| 到网关 | 查询走 `/api/*`（reqwest），命令走每次新建的短 WS，事件订阅走一条长 WS | `ssr-h/src/agent.rs:19-43`、`:327-378`、`:388-430` |
| 鉴权 | 独立 `--ui-token` + `ssr_h_token` cookie；Host 白名单、Origin 校验、CSP | `ssr-h/src/guard.rs:36-65`、`:103-142`、`:283-311`、`:351-364` |
| UI | Askama 编译期模板 + `include_bytes!` 静态资源 + Boa/OXC 跑的 TSX 前端插件 | `ssr-h/src/templates.rs:186-340`、`ssr-h/src/main.rs:374-483`、`ssr-h/src/tsx/mod.rs` |
| 插件状态 | 停用列表写在插件目录旁的 `.ssr-h-state.json` | `ssr-h/src/plugins/registry.rs:346-385` |
| 验收 | 两个真进程脚本，各自起网关 + ssr-h | `tools/verify-ssr-streaming.mjs`、`tools/verify-ssr-workspaces.mjs` |

### 2.2 网关是什么

| 维度 | 现状 | 证据 |
|---|---|---|
| 形态 | 根 workspace 成员，只依赖 `agent-proto`（check J 守着） | `crates/agent-gateway/Cargo.toml`、`src/lib.rs:17-25` |
| 路由 | `/rpc` + `nest("/api")` + `fallback(static_handler)` | `crates/agent-gateway/src/router.rs:30-36` |
| HTTP API | 71 条一元路由（agent 方法 + `gateway.*` 管理面），body 上限 4 MiB | `crates/agent-gateway/src/http_api.rs:51-55`、`:142-942` |
| 网页访问 | `/` 托管 `desktop-ui/dist`（可 `--ui-dir` 指定），静态资源不要 token | `crates/agent-gateway/src/static_ui.rs:1-30`、`docs/gateway/README.md:127-197` |
| 鉴权 | WS query token / HTTP Bearer / 工作区作用域 / Origin / 配对码；**没有 cookie** | `crates/agent-gateway/src/auth.rs:98-220`、`:303-320` |
| 实例模型 | 一工作区一进程；`ensure_agent` 复用/拉起；空闲 15 分钟回收，最多 6 个 | `crates/agent-gateway/src/supervisor.rs:407-466`、`:472-524` |
| 连接计数 | `AgentRegistry::connections` 只由 `relay` 增减；有连接就不回收 | `crates/agent-gateway/src/registry.rs:212-256`、`:317-335` |

### 2.3 合并要解决的九个具体冲突

| # | 冲突 | 现状证据 | 处理方向 |
|---|---|---|---|
| C1 | 两个监听/两个 `axum::serve` | `ssr-h/src/main.rs:180-189` vs `agent-gateway/src/main.rs:119-141` | 删 ssr-h 的 listener，只保留网关一份 |
| C2 | 两个 Router、两个 AppState | `ssr-h/src/main.rs:193-315` vs `agent-gateway/src/router.rs:30-36` | `agent-web` 暴露子 Router，网关合并 |
| C3 | `/` 归属冲突 | SSR 聊天页 vs desktop-ui 首页 | `--web-ui` 取 `ssr` 或 `desktop`（见 D2） |
| C4 | `GET /api/plugins` 撞名 | SSR 前端插件快照 vs 网关 `plugin.list` | SSR 自省端点改名并移出 `/api`（见 T07） |
| C5 | fallback 只有一个 | SSR `not_found` vs static SPA 兜底 | 按 `--web-ui` 选择唯一 fallback |
| C6 | 两套鉴权 | SSR cookie+Host+Origin vs 网关 token+scope+Origin | guard 只包 SSR 路由；凭据分阶段统一（D3） |
| C7 | guard 绑定固定端口 | `Access::for_host(host, port, ...)` 要求 Host 端口完全一致 | 绑定后取真实端口再构造 `Access`（兼容 `--port 0` 与顺延） |
| C8 | 模板/静态路径是编译期相对的 | Askama 默认相对 crate 根；`include_bytes!` 相对源文件 | 随 crate 搬家并修正路径（T01） |
| C9 | 自反代与回收器脱节 | SSR 直连 agent 时不经过 `relay`，`connections` 不增加 | 事件订阅必须持有公共连接守卫（T05） |

## 3. 目标形态

### 3.1 拓扑

```text
Electron ── http://127.0.0.1:<port>/ ─────┐
                                          ├──▶ ada-gateway（一个进程、一个端口）
浏览器 ──────────── 同源 HTML/SSE ────────┘        │
                                                   ├─ SSR 页面处理器（agent-web）
                                                   ├─ /api/*（一元调用）
                                                   ├─ /rpc（WS 桥）
                                                   └─ 注册表 / 监督 / 委派
                                                            │ 每工作区一个
                                                            ▼
                                                       ada-coding × N
```

桌面端不经过网关 `/`，所以 SSR 接管 `/` 不会改变桌面壳；浏览器访问则从 React 首页变成 SSR 首页
（需要 React 时仍可用 `--web-ui desktop` 启动）。

### 3.2 crate 边界

新增根 workspace 成员 `crates/agent-web`（包名 `agent-web`，产品代号仍叫 ssr-h）：

```text
crates/agent-web/
  Cargo.toml          # 依赖：agent-proto（只取常量）+ axum/askama/... + boa/oxc
  src/                # 由 ssr-h/src 平移：handlers / plugins / tsx / ui / guard / agent / ...
  templates/          # Askama 编译期模板
  static/             # 内嵌静态资源
  tests/              # ui_scale.rs 等
  frontend-plugins/   # 示例插件（打包时可外置）
```

依赖方向：

```text
agent-gateway ──▶ agent-web ──▶ agent-proto
      │
      └──▶ agent-proto
```

- `agent-web` **不得**依赖 `agent-node`、`agent-base`、`agent-runtime`、`agent-toolkit`、`agent-rpc`；
  它只消费 JSON 形状与 `agent-proto` 的方法常量。
- `agent-gateway` 只额外依赖 `agent-web`，不依赖任何引擎 crate。
- `verify-wiring` 的 check J 扩展为**同时扫描 `agent-web` 的清单与源码**（`tools/xtask/src/gates.rs:1302-1420`），
  防止 UI 层悄悄把节点/引擎拉进来。

### 3.3 `WebBackend`：四个原语，两个实现

`agent-web` 不直接依赖 `Gateway`（否则与 `agent-gateway → agent-web` 形成环），而是定义端口：

```rust
// 伪代码：省略错误类型与 async_trait 细节
pub trait WebBackend: Send + Sync {
    /// 一元查询/命令：路径 + query + body，语义与网关 `/api/*` 一致。
    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value>;
    async fn post(&self, path: &str, body: Value) -> Result<Value>;
    /// 需要 WS 的命令（thread.send / approval.decide / ...）。
    async fn rpc(&self, method: &str, params: Value) -> Result<Value>;
    /// 订阅 `evt.state.snapshot` / `evt.state.patch`；连接断开时返回。
    async fn subscribe(&self, tx: mpsc::Sender<Value>) -> Result<()>;
}
```

- `HttpBackend`：现有 `AgentClient` 的 reqwest + 短 WS + 长 WS 逻辑，原样搬进实现。
  独立运行时用它，两个实机验收脚本也用它。
- `GatewayBackend`：放在 `agent-gateway`（适配器属于宿主），实现四个原语：
  - `get/post` → 网关抽出的 `Gateway::call_api(workspace, path, query, body)`；
  - `rpc` → `Gateway::call_rpc(workspace, method, params)`（内部 `ensure_agent` + `call_agent`）；
  - `subscribe` → `ensure_agent` 后通过 `link::AgentEndpoint::connect` 直连该 agent，
    `session.initialize` 声明 `capabilities: ["state.patch"]`，只转发两个 `evt.state.*` 主题。
- `AgentClient` 的公开方法签名保持不变，只把字段从 `http+base+token` 换成 `Arc<dyn WebBackend>`。
  这样所有处理器一行不用改，风险集中在四个原语的等价性上。

### 3.4 路由与鉴权职责

`--web-ui ssr` 时（最终默认）：

| 请求 | 去向 | 鉴权 |
|---|---|---|
| `GET /`、`/threads/*`、`/overview`、`/providers*`、`/workspaces*`、`/extensions` | SSR 处理器 | SSR cookie + Host/Origin |
| `GET /threads/{id}/stream` | SSE | 同上 |
| `GET /static/*` | 内嵌静态资源 | 免 token（只含 JS/CSS） |
| `GET/POST /ext/{id}/...` | TSX 插件页/资源/API | 同上 |
| `GET /web-ui/frontend-plugins` | SSR 前端插件自省（原 `/api/plugins`） | 同上 |
| `GET /rpc` | 网关 WS 桥 | 网关 token / Origin |
| `/api/*` | 网关一元 API | 网关 Bearer/query token |
| 其他 | SSR 404 页 | — |

`--web-ui desktop` 时保持今天的表：`/` 与静态资源由 `static_ui` 托管，SSR 路由不挂载。

**中间件分层**：`guard`（Host/Origin/CSP/安全响应头）只包 SSR 子 Router；`/api`、`/rpc`、
`/static` 不套 SSR 的 CSP 与 `X-Frame-Options`。`CompressionLayer` / `TraceLayer` 可以全局。
`POST /threads/{id}/send` 的 32 MiB body 限制只挂这一条路由。

### 3.5 为什么不是"把源码直接并进 `agent-gateway`"

| 方案 | 优点 | 问题 |
|---|---|---|
| **A. 新增 `agent-web`（推荐）** | 管理面与 UI 投影面分离；可单独测试/运行；网关 crate 仍可读 | 多一个 workspace 成员与一条依赖边 |
| B. 直接并入网关 crate | 少一个 crate | 网关从 7.5k 行涨到 27k 行，Askama/Boa/OXC 全塞进管理面；check J 的"只依赖线协议"更难解释 |
| C. 保留 ssr-h 独立 crate，网关依赖它 | 改动最小 | ssr-h 已有自己的 `[workspace]`/`Cargo.lock`，要么破坏独立构建，要么无法作为库被依赖；仍要解决 AppState/Router 冲突 |

选 A。它把"UI 是投影层"这件事落到 crate 边界上，也让 M0 的搬家可以纯机械完成、不影响行为。

## 4. 关键设计决策

| # | 决策 | 选择（推荐） | 备选 | 理由 / 影响 |
|---|---|---|---|---|
| D1 | 代码归属 | 新增 `crates/agent-web` | 并入网关；保留独立 crate | 见 §3.5 |
| D2 | 根路径归属 | `--web-ui` 取 `ssr` 或 `desktop`，**最终默认 `ssr`**（Electron 也默认加载 SSR） | SSR 挂 `/ssr`；两者同时挂 | SSR 页面里全是根绝对 URL；挂子路径要改模板/表单/SSE/JS，成本远高于开关。desktop-ui 的 Vite 产物又写死 `/assets/*`，同样不适合挂子路径 |
| D3 | 浏览器凭据 | 过渡期沿用独立 `--web-token`；M3 起接受网关 token/配对码并换发 HttpOnly 会话 cookie | 立即统一；不做鉴权 | 进程内调用不需要 token，浏览器只需要"能不能看这个 UI"；分阶段可先合并结构再收口凭据 |
| D4 | standalone 二进制 | M0–M2 保留为开发/验收入口，不作为发布产物；M4 再决定删或加 feature | 立即删除 | 保留一条"不依赖网关进程"的回归路径，便于对比两个后端的行为 |
| D5 | 网关依赖声明 | `agent-gateway` 增加 `agent-web`；文档改为"依赖线协议 + UI 投影层" | 把 UI 代码塞进网关 | 需要同步改 `lib.rs` 边界说明与 check J 的扫描范围，不能只改代码 |
| D6 | 前端插件自省端点 | 从 `/api/plugins` 改为 `/web-ui/frontend-plugins` | 保留 `/api/plugins` | 避免与网关 `plugin.list` 撞名；移出 `/api` 嵌套，避免 axum 路由优先级歧义 |
| D7 | 插件目录与状态 | 插件目录 `~/.a-da/web-plugins`（可 `--web-plugins-dir` 覆盖）；停用状态 `~/.a-da/web-ui-state.json` | 继续放 exe 旁 | 打包安装目录可能只读；状态属于用户配置，应与凭据目录同级 |

## 5. 批次与任务卡

每张卡都要能单独 commit、单独回退。**M0 不改变任何运行时行为**，这是整个计划的基线。

### M0 结构搬家（不改行为）

**T01 新建 `crates/agent-web` 并平移代码**

- 新建 `crates/agent-web/Cargo.toml`，把 `ssr-h/Cargo.toml` 的依赖整体搬过来（去掉 `clap` 与
  `reqwest`/`tokio-tungstenite` 在 M1 之后再决定是否保留给 `HttpBackend`）。
- 平移 `ssr-h/src`、`ssr-h/templates`、`ssr-h/static`、`ssr-h/tests`、`ssr-h/frontend-plugins`。
- 根 `Cargo.toml` 增加成员与 `agent-web = { path = "crates/agent-web" }`；
  把 `oxc_ast` 补进 `[workspace.dependencies]`（ssr-h 直接用它）。
- 修正 Askama 模板根（默认即 crate 根，搬到 `crates/agent-web/templates` 后无需额外配置）、
  `include_bytes!` 的相对路径、logo 资源路径。
- 删除 `ssr-h/Cargo.toml`、`ssr-h/Cargo.lock` 与目录；`git mv` 保留历史。
- 更新 `tests/ui_scale.rs` 里按 `CARGO_MANIFEST_DIR` 推导仓库布局的路径（`ssr-h/tests/ui_scale.rs:21-27`）。

验收：`cargo build -p agent-web`、`cargo test -p agent-web -- --test-threads=1`；
`cargo xtask verify --fast` 不红（此时网关尚未依赖它）。

回退：整个 M0 是一个纯移动 commit，`git revert` 即可。

**T02 引入 `WebBackend`，保留 `HttpBackend`**

- 在 `agent-web` 定义 §3.3 的 trait 与 `HttpBackend`（现有 `AgentClient` 的 reqwest/WS 逻辑）。
- `AgentClient` 改为 `struct AgentClient { backend: Arc<dyn WebBackend>, workspace: String }`，
  所有公开方法保留原签名；内部只做参数组装。
- 保留一个 `ssr-h` 开发二进制（可以暂时留在 `agent-web` 的 `[[bin]]`），
  用 `HttpBackend` 连一个已经跑起来的网关。

验收：两个实机脚本（`tools/verify-ssr-streaming.mjs`、`tools/verify-ssr-workspaces.mjs`）**不改脚本**
仍能通过——证明搬家与后端抽象没有改变行为。

回退：`HttpBackend` 就是旧逻辑，删掉 trait 即可回到 T01。

**T03 把 `agent-web` 改成可挂载**

- 把 `main.rs` 里的 `app(state)` 抽成 `pub fn router(state: WebState) -> Router`，
  不再自己 bind/serve；`WebState` 包含 `Arc<dyn WebBackend>`、`Arc<PluginRegistry>`、`Access`。
- `Access` 由宿主编排构造：`agent-web` 只提供 `Access::for_host(host, port, token)`，
  端口由调用方在**绑定之后**传入。
- 增加一个 `pub fn access_guard_layer(access: SharedAccess)`，供宿主只给 SSR 路由套。
- 处理 `rewrite_plugin_redirect` 与 `CompressionLayer`/`TraceLayer` 的层次：把 SSR 专属层收进
  `agent-web::router`，全局层留给网关。

验收：`agent-web` 的单测覆盖"router 可被 `oneshot` 调用"（参考网关 `router.rs` 的测试写法）。

### M1 进程内挂载（可开关）

**T04 网关增加 `--web-ui` 与 SSR 挂载**

- 新增参数：`--web-ui`（`ssr` 或 `desktop`；环境变量 `A_DA_WEB_UI`）、`--web-token`
  （或 `A_DA_WEB_TOKEN`）、`--web-plugins-dir`。
- `AppState` 增加 `web: Option<Arc<WebState>>`；保留 `router(state)` 原签名给现有测试，
  新增 `router_with_web(state, web)`，生产走后者。
- `--web-ui ssr`：合并 SSR 子 Router，`/`、SSR 路由、SSE、`/static/*`、`/ext/*`、
  `/web-ui/frontend-plugins` 生效，fallback 用 SSR 404。
- `--web-ui desktop`：保持今天的 `router(state)`，SSR 完全不挂。
- READY 行增加 `webUi` 字段（不含任何 token）。

验收：`cargo test -p agent-gateway` 全绿；新增 `web_ui_e2e.rs` 的首批用例
（`/` 返回 SSR 聊天页、`/rpc` 仍 426/101、`/api` 仍按 token 鉴权）。

回退：`--web-ui desktop` 就是旧行为；代码路径也在同一个 Router 里，没有第二个进程。

**T05 `GatewayBackend`：进程内一元调用与事件订阅**

- 从 `http_api` 抽出两个可复用入口：
  - `Gateway::call_api(workspace, path, query, body)`：按现有 `ROUTES` 表语义做一元调用；
  - `Gateway::call_rpc(workspace, method, params)`：`ensure_agent` → `call_agent`；
  - 网关管理面：直接调 `Gateway` 的 `fs_*` / `workspaces_*`（只保留 SSR 真正用到的几个）。
- 把 `relay.rs:798-806` 的 `ConnGuard` 提到 `registry.rs`，变成公共
  `AgentRegistry::open_connection(id) -> ConnectionGuard`；`relay` 改用它。
- `GatewayBackend::subscribe`：
  1. `ensure_agent` 拿到实例；
  2. 持有 `ConnectionGuard`，并每转发一帧 `registry.touch(id)`；
  3. `AgentEndpoint::parse` + `connect`（遵守 `tcp` / `pipe` / `unix` 三种端点）；
  4. `session.initialize` 声明 `state.patch`；
  5. 只转发 `evt.state.snapshot` / `evt.state.patch`。
- 显式记录：`ConnectionGuard` 是**长连接不被回收器误杀**的唯一依据；
  没有它，浏览器标签页开着 15 分钟后 agent 会被回收，表现是"对话突然不动"。

验收：
- 后端等价性单测：同一组方法/参数，`HttpBackend` 与 `GatewayBackend` 返回的 JSON 形状一致；
- 回收器单测：`subscribe` 持有期间 `reap_once` 不回收，drop 后可回收；
- 真机：`verify-ssr-streaming.mjs` 改走进程内后端（M2 完成脚本改造）。

回退：`GatewayBackend` 是独立实现，切回 `HttpBackend` 即可（保留 `--web-backend http` 调试开关，
M4 删除）。

**T06 鉴权与中间件作用域**

- `guard` 只包 SSR 子 Router；`/api`、`/rpc`、`/static` 不继承 SSR 的 CSP / `X-Frame-Options`。
- `Access` 用**绑定后的真实端口**构造；`Host` 校验仍要求端口一致。
- 非回环监听时，`--web-ui ssr` 要求 `--web-token`（或环境变量）非空且 ≥12 位；
  与网关 `--token` 的检查并列，两个都失败即拒绝启动。
- 反向代理场景（Host 是域名、端口 443）留给 M3：先用 `--web-public-url` / `--web-allow-host`
  显式声明，避免把 Host 校验做成"只能直连"。

验收：`web_ui_e2e.rs` 覆盖
"跨站 POST → 403"、"Host 端口不对 → 403"、"API 请求不会被 SSR CSP 污染"、
"静态资源免 token"。

**T07 前端插件自省端点与目录**

- `GET /api/plugins` → `GET /web-ui/frontend-plugins`；同步 `static/components.js` 的轮询地址、
  `docs/` 与 README。
- `AgentClient::plugins()` 改为调 `plugin.list`（agent 方法），不再请求本进程的 HTTP 路由。
- 插件目录解析顺序：`--web-plugins-dir` → `A_DA_WEB_PLUGINS_DIR` → `<A_DA_HOME>/web-plugins`
  → exe 旁 `frontend-plugins` → 当前目录 `frontend-plugins`；
  开发时最后回退到 crate 内的 `frontend-plugins`。
- 停用状态写入 `<A_DA_HOME>/web-ui-state.json`；读取时兼容旧的插件目录内 `.ssr-h-state.json`
  （一次性迁移或忽略，二选一，需在实现时定）。

验收：插件页重载/启停/资源 `@scope` 注入全部通过；目录只读时启动不失败（状态走用户目录）。

**T08 打包与资源**

- SSR 的模板与静态资源是编译期内嵌，不需要额外拷贝；只有 `frontend-plugins` 需要随包分发。
- `electron-builder.json` 增加 `frontend-plugins` 到 `resources/bin/`（若决定随包）；
  `findUiDir`/`--ui-dir` 逻辑不变（仍服务 `desktop` 模式）。
- `dist/` 不再产出第二个 `ssr-h.exe`；`cargo xtask ship` 的产物清单同步。

验收：`bun run electron:pack` 后，`resources/bin/` 里能找到插件目录；桌面端启动、
浏览器 `http://127.0.0.1:52353/` 能打开 SSR 首页。

### M2 默认切换与脚本改造

**T09 Electron 与浏览器入口**

- Electron 默认 `--ui=ssr`：等网关就绪后加载 `http://127.0.0.1:<port>/`，带 token 时用
  `?token=` 自动登录；窗口保持无边框，由 preload 在 SSR 顶栏注入最小化/最大化/关闭。
- `--ui=desktop`（`A_DA_ELECTRON_UI=desktop` / `bun run electron:dev:desktop`）保留原 React/Vite
  界面与自绘标题栏；`desktop-ui` 的「浏览器打开」链接仍指向 `/`。

**T10 验收脚本改单进程**

- `verify-ssr-streaming.mjs`：只起 `ada-gateway --web-ui ssr --web-token ... --port <free>`，
  等 `A_DA_GATEWAY_READY`，直接访问 `/threads/{id}/send` 与 `/threads/{id}/stream`。
- `verify-ssr-workspaces.mjs`：同上，去掉 `--gateway`、第二端口与 `ssr-h listening on` 的等待。
- 新增 `tools/verify-gateway-web.mjs`：一个端口上同时验证
  `/`（SSR）、`/api/*`（Bearer）、`/rpc`（token）、`/static/*`、`/web-ui/frontend-plugins`。

**T11 默认值切换**

- 当 M0–M2 全部出场门通过后，把 `--web-ui` 默认值从 `desktop` 改为 `ssr`；
  保留 `desktop` 作为至少一个版本的显式回退。
- 更新 README 的启动示例、`docs/gateway/README.md` §2.5、`docs/protocol/README.md` 路由表。

### M3 凭据统一（待拍板后执行）

**T12 `/__auth` 接受网关凭据**

- `/__auth` 除 `--web-token` 外，接受网关 `--token` 或一次性配对码（`AuthConfig` 已有的
  `authorize` / `redeem_pairing_code`）。
- 校验通过后换发一个随机 session id 的 HttpOnly cookie（`SameSite=Strict`），
  服务端内存表映射 session → 工作区作用域；不把网关 token 写进 cookie。
- 受限 token 的工作区作用域要传进 `WebState`，越权的工作区操作返回 403。
- `--web-token` 降级为兼容项，文档标注"下一个大版本移除"。

验收：`web_ui_e2e.rs` 覆盖 token 登录、配对码登录、作用域外工作区被拒、cookie 过期后重新登录。

### M4 清尾

**T13 独立二进制与旧参数**

- 决定 standalone 二进制的去留：删除，或保留 `--features standalone` 仅供开发。
- 删除 `--web-backend http` 调试开关（若 M1 引入）。
- 清理不再使用的依赖（`reqwest` / `tokio-tungstenite` 若只剩 standalone）。

**T14 文档与门禁收尾**

- 把 ssr-h 的 `docs/*` 按功能域拆到 `docs/gateway/` 或 `crates/agent-web/README.md`；
  原 `ssr-h/docs` 删除或保留为 `crates/agent-web/docs`。
- **不要**在 markdown 里直接写归档时代的源码目录路径（含反引号也算）；
  用"`agent-web` 的 `ui` 子模块"表述。当前 ssr-h 的若干文档已经命中这条门禁，
  搬家时顺手改写，或给 `crates/agent-web/docs` 加一条**写明理由**的白名单。
  2026-10-25 已清零：SSR 文档里的路径改为相对 `ui/` 表述，`verify-docs` 无违约。
- 更新 §6.2 列出的所有索引与状态文档。

## 6. 协议与文档同步

### 6.1 协议：本次不新增 JSON-RPC 方法

合并只增加 HTML 表单路由、SSE 端点与一个自省 JSON 端点，**不进入 `spec/proto/*.json`**。
因此 §16 的四处同步（spec → `methods.rs` → dispatch → `client-ts/methods.ts`）本次不触发。

若实现中确实需要新方法（例如"Web UI 会话"），必须：
1. `spec/proto/*.json` 登记；
2. `crates/agent-proto/src/methods.rs` 加常量与 `ALL_METHODS`；
3. `crates/agent-rpc/src/server/dispatch/` 加分支；
4. `client-ts/methods.ts` 同步；
5. `docs/protocol/README.md` 补用法；`cargo xtask verify-wiring` 与 `cargo test -p agent-proto` 守着。

### 6.2 文档清单

| 文件 | 要改什么 |
|---|---|
| `docs/protocol/README.md` | §1.2 路由表（SSR 页面/SSE/静态/`/web-ui`）；§2 鉴权（cookie 与 token 的关系）；§3 端点表不变；§10 真源表补 `agent-web` |
| `docs/gateway/README.md` | §0/§1 拓扑与角色矩阵；§2.5 静态托管改为"SSR 或 desktop-ui 二选一"；新增一节写 `agent-web` 与 `WebBackend` |
| `docs/quality/feature-catalog.md` | §十/§十一 增加 SSR 网页界面；§十二 说明 HTML/SSE 路由不计入 JSON-RPC；§十三 增加新验收脚本 |
| `docs/quality/unfinished-features.md` | 网关退出不回收子进程（`:295-304`）与 HTTP Phase 3（`:306-317`）补一句与 SSE 的关系；新增合并遗留项 |
| `README.md` | 系统图、目录表、启动示例、已知限制 |
| `AGENTS.md` | 代码结构增加 `agent-web`；开发命令增加 `--web-ui`；坑位补 `ConnGuard` 与 `/api/plugins` 撞名 |
| `docs/README.md` | 登记本文件；网关条目加 SSR 说明 |
| `docs/plans/README.md` | 加一行指向本文件（"现行计划"） |

### 6.3 `verify-docs` 注意事项

`cargo xtask verify-docs` 会扫全仓 markdown 里的 8 个归档布局字面量
（`tools/xtask/src/gates.rs:1508-1638`：TS 时代的 agent/UI 源码目录、旧 GPUIX 仓库路径、
旧前端入口文件名、两条已退役的 Bun 命令、旧窗口骨架文件名、旧 platform 目录）。
本文件与迁移后的文档都不要写出这些原文；需要指代 UI 目录时写"`agent-web` 的 `ui` 子模块"。

## 7. 测试与出场门

### 7.1 每批共同门

```bash
CARGO_BUILD_JOBS=2 cargo build --workspace
CARGO_BUILD_JOBS=2 cargo test --workspace -- --test-threads=1
bun run typecheck                 # desktop-ui 未改，但合并期间必须保持绿
cargo xtask verify --fast
```

### 7.2 新增测试

| 测试 | 落点 | 断言 |
|---|---|---|
| SSR 路由 e2e | `crates/agent-gateway/tests/web_ui_e2e.rs` | `/` 返回聊天页；`/threads` 可访问；`/static/*` 免 token；未知路径返回 SSR 404 |
| API/RPC 不受影响 | 同上 | `/api/gateway/info` 公开；其余 `/api` 仍 401；`/rpc` 仍 426/101 |
| guard 作用域 | 同上 | 跨站 POST 403；Host 端口不符 403；`/api` 响应没有 SSR 的 CSP |
| 后端等价性 | `crates/agent-gateway` 单测 | 同一组方法/参数，`HttpBackend` 与 `GatewayBackend` 结果形状一致 |
| 回收器与长连接 | `crates/agent-gateway` 单测 | `subscribe` 持有期间不被回收；drop 后可回收 |
| 插件目录/状态 | `crates/agent-web` 单测 | 只读目录可启动；状态写用户目录；旧状态文件兼容 |
| 设计系统 | `crates/agent-web/tests/ui_scale.rs` | 搬家后路径断言仍成立 |

### 7.3 实机验收

- `node tools/verify-ssr-streaming.mjs`（T10 改造后单进程）
- `node tools/verify-ssr-approval-abort.mjs`（审批批准 → 工具真正执行 → 流式中止）
- `node tools/verify-ssr-workspaces.mjs`（同上）
- `node tools/verify-gateway-web.mjs`（新增）
- `pwsh -NoProfile -File tools/verify-desktop-shell.ps1`（桌面壳行为不变；锁屏时自动改用
  `tools/verify-desktop-shell-headless.mjs` 的 CDP 无头验收：窗口控制 / 状态同步 / 发消息 /
  退出清理，真实命中渲染层与原生窗口状态）
- `bun tools/verify-full-access-paths.mjs`（路径权限未被 UI 层改变）

### 7.4 最终出场门

1. `cargo xtask verify --fast` 全绿；
2. `cargo test --workspace -- --test-threads=1` 全绿；
3. `bun run typecheck` 全绿；
4. 四个实机脚本全过；
5. 浏览器打开 `http://127.0.0.1:52353/` 能完成"新建会话 → 发消息 → 看到流式回答 → 审批 → 中止"；
6. Electron 桌面端启动、发消息、关闭窗口、退出清理均正常；默认加载 SSR 页面
   （`node tools/verify-electron-ssr.mjs`；`--ui=desktop` 的 React 壳仍由
   `verify-desktop-shell.ps1` / `verify-desktop-shell-headless.mjs` 覆盖）。

## 8. 风险与回退

| 风险 | 表现 | 处置 |
|---|---|---|
| Boa/OXC 进入网关进程 | 二进制变大、编译变慢；插件渲染 panic 可能影响宿主 | 渲染继续走独立线程 + `catch_unwind`；保留渲染预算；必要时把 TSX 插件做成可选 feature |
| 阻塞渲染占满运行时 | 多个插件页同时渲染时网关响应变慢 | 继续 `spawn_blocking`，限制并发；不把渲染放在 async 线程 |
| guard 误伤 API | `/api` 响应带上 CSP/`X-Frame-Options`，或浏览器预检被 403 | guard 只包 SSR 子 Router；用 e2e 断言 API 响应头 |
| 端口 0 / 顺延 | Host 校验按 52353 拒绝实际端口 | `Access` 用绑定后的真实端口构造 |
| SSE 长连接被回收 | 标签页开着十几分钟后回答突然停 | `GatewayBackend::subscribe` 持有 `ConnectionGuard`，并逐帧 `touch`；单测守着 |
| `/api` 路由优先级 | SSR 自省端点被 `nest("/api")` 吞掉 | 自省端点移到 `/web-ui/*`，不参与 `/api` 嵌套 |
| desktop-ui 子路径 | 挂 `/desktop` 后 `/assets/*` 404 | 不做子路径，用 `--web-ui` 二选一 |
| 反向代理 | Host 是域名/443，Host 校验拒绝 | M3 增加 `--web-public-url` / `--web-allow-host`；文档写清 |
| 文档门禁 | 迁移后的 markdown 含归档布局字面量 | 改写表述；必要时加白名单并写明理由 |
| 回退 | 合并后浏览器界面异常 | `--web-ui desktop` 回到 React 首页；`HttpBackend` + standalone 二进制提供第二条回归路径 |

## 9. 工期汇总

| 批次 | 内容 | 估时 |
|---|---|---|
| M0 | T01–T03 结构搬家、后端抽象、可挂载 Router | 3.5 人日 |
| M1 | T04–T08 进程内挂载、连接守卫、鉴权作用域、插件端点、打包 | 5 人日 |
| M2 | T09–T11 桌面入口、脚本改造、默认切换 | 2 人日 |
| M3 | T12 凭据统一（待拍板） | 2 人日 |
| M4 | T13–T14 清尾与文档门禁 | 1 人日 |
| **合计** | | **约 13.5 人日** |

## 10. 待拍板

1. **凭据是否统一**（D3/M3）：统一后浏览器只需一套凭据，但要处理 token 作用域与配对码；
   不统一则长期保留 `--web-token`。
2. **默认切换时点**（T11）：M2 完成即切，还是再观察一个版本。
3. **standalone 二进制**：M4 删除，还是保留为 `--features standalone` 的开发工具。
4. **插件目录位置**（D7）：`~/.a-da/web-plugins`，还是随包分发只读目录 + 用户目录叠加。
5. **反向代理支持**：是否本期就做 `--web-public-url` / `--web-allow-host`，还是留给部署文档。
6. **`/web-ui/frontend-plugins` 的协议地位**：仅内部页面使用，还是登记为稳定的 Web 端点。

## 11. 实施记录（2026-10-25）

| 项 | 落点 |
|---|---|
| SSR crate | `crates/agent-web`（`AgentClient` + `WebBackend` / `HttpBackend`；`router()` 可挂载） |
| 网关进程内后端 | `crates/agent-gateway/src/web.rs`（`GatewayBackend` + `GatewayLogin` + `build_web_state`） |
| 路由 | `router_with_web` 用 `fallback_service` 挂 SSR；`/rpc`、`/api/*` 保持网关自己的鉴权与路由 |
| 连接守卫 | `AgentRegistry::open_connection` / `ConnectionGuard`（`relay` 与 SSR 事件订阅共用） |
| 凭据 | `/__auth` 接受网关 token / 配对码，换发 HttpOnly 会话 cookie；`--web-token` 保留兼容 |
| 插件 | 自省端点 `/web-ui/frontend-plugins`；停用状态 `~/.a-da/web-ui-state.json` |
| 默认值 | 网关 `--web-ui` 默认 `ssr`；Electron `--ui` 默认 `ssr`，`desktop` 保留回退 |
| Electron | 等网关就绪后加载 `http://127.0.0.1:<port>/`，带 token 时 `?token=` 自动登录；窗口保持无边框，preload 在 SSR 顶栏注入窗口控制；`--ui=desktop` 回退 React |
| 验收 | `cargo test -p agent-web`、`cargo test -p agent-gateway`、`node tools/verify-gateway-web.mjs`、`node tools/verify-ssr-streaming.mjs`、`node tools/verify-ssr-approval-abort.mjs`、`node tools/verify-ssr-workspaces.mjs`、`node tools/verify-electron-ssr.mjs`、`pwsh -NoProfile -File tools/verify-desktop-shell.ps1`（`--ui=desktop`；锁屏时走 `verify-desktop-shell-headless.mjs`）、`cargo xtask verify-docs` |

## 附录 A：文件落点对照

| 现在 | 合并后 |
|---|---|
| `ssr-h/src/main.rs` 的 Router/监听 | 网关 `router_with_web`；`agent-web` 只留 `router(state)` |
| `ssr-h/src/agent.rs` 的 reqwest/WS | `agent-web::backend::{WebBackend, HttpBackend}` |
| `ssr-h/src/guard.rs` | `agent-web` 原样保留，改由宿主编排 `Access` |
| `ssr-h/src/handlers/*` | `agent-web` 原样保留 |
| `ssr-h/src/plugins/*`、`src/tsx/*` | `agent-web` 原样保留 |
| `ssr-h/templates/`、`ssr-h/static/` | `crates/agent-web/templates/`、`crates/agent-web/static/` |
| `ssr-h/frontend-plugins/` | `crates/agent-web/frontend-plugins/`（打包可外置） |
| `ssr-h/tests/ui_scale.rs` | `crates/agent-web/tests/ui_scale.rs`（路径断言更新） |
| `ssr-h/Cargo.toml` + `Cargo.lock` | 删除；根 workspace 增加 `crates/agent-web` |
| `tools/verify-ssr-*.mjs` | 改为单进程单端口；新增 `verify-gateway-web.mjs` |
