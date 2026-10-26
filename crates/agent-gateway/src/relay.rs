//! **桥接面**：客户端 ↔ 网关 ↔ agent 实例。
//!
//! # 两条通道
//!
//! ```text
//! 客户端 ──WS──► 网关 ──┬─ gateway.*   → 网关**自己答**（管理面）
//!                       └─ 其余方法    → 透传给该客户端所属的 agent 实例
//! ```
//!
//! # 透传的四条硬约束（网关的"不作为"才是正确行为）
//!
//! | # | 约束 | 为什么 |
//! |---|---|---|
//! | 1 | **不改 `id`、不改 `seq`、不改 `call_id`** | `seq` 由 runtime 生成（INV-6）；改了就断单调性 |
//! | 2 | **不缓存会话/线程/工具目录** | INV-8：一个事实一个所有者；缓存必然漂移 |
//! | 3 | **不含引擎、不执行工具** | INV-1：单一引擎 |
//! | 4 | 只持有**注册表 + 连接态** | 那是网关自己的事实 |
//!
//! # 路由策略（S5：单实例透传）
//!
//! 客户端连上来时，网关按 `workspace` 找（或拉起）一个实例，然后**整条连接绑定**到它。
//! 这样同一工作区的多个客户端落到同一个 agent（否则两份状态互相覆盖），
//! 而不同工作区天然隔离。
//!
//! 会话亲和放在**连接级**而不是帧级：帧级路由需要解析每个方法的参数去猜目标，
//! 而连接级路由只需要一次决定——更少的状态、更少的出错面。

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::ws::{CloseFrame as AxCloseFrame, Message as AxMessage, WebSocket};
use axum::extract::{OriginalUri, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::auth::{query_param, AuthError, AuthOutcome, TokenScope};
use crate::delegate::{
    delegate, DelegateError, DelegateOutcome, DelegationRegistry, DEFAULT_DELEGATION_TIMEOUT,
};
use crate::registry::{AgentInstance, AgentRegistry, AgentStatus, normalize_workspace};
use crate::supervisor::{ensure_agent, now_ms, SpawnSpec};

/// 网关自己处理的方法前缀。
///
/// 前缀之外的一律透传——**白名单**而不是黑名单：新增协议方法时网关不需要改，
/// 也不会误吞节点的方法。
pub const GATEWAY_METHOD_PREFIX: &str = "gateway.";

/// 网关管理面的方法名。
pub mod methods {
    pub const LIST_AGENTS: &str = "gateway.listAgents";
    pub const ATTACH: &str = "gateway.attach";
    pub const REGISTER: &str = "gateway.register";
    pub const STATUS: &str = "gateway.status";
    pub const DETACH: &str = "gateway.detach";
    /// **交互平台**：派活给某个 agent 并等它跑完（S6）
    pub const DELEGATE: &str = "gateway.delegate";
    /// **交互平台**：取消进行中的派活（跨网关取消，S6）
    pub const CANCEL_DELEGATION: &str = "gateway.cancelDelegation";
    /// **文件系统**：列出驱动器与根目录
    pub const FS_ROOTS: &str = "gateway.fs.roots";
    /// **文件系统**：浏览目录
    pub const FS_LIST_DIRECTORY: &str = "gateway.fs.listDirectory";
    /// **文件系统**：新建目录
    pub const FS_MAKE_DIRECTORY: &str = "gateway.fs.makeDirectory";
    /// **工作区管理**：获取最近与活跃工作区
    pub const WORKSPACES_LIST: &str = "gateway.workspaces.list";
    /// **工作区管理**：移除工作区
    pub const WORKSPACES_REMOVE: &str = "gateway.workspaces.remove";
}

/// 网关的共享状态（一个网关进程一份）。
pub struct Gateway {
    pub registry: Arc<AgentRegistry>,
    /// 拉起实例的串行锁：两个客户端同时连同一工作区时不能各起一个
    pub spawn_lock: Arc<Mutex<()>>,
    /// 网关自身 pid（传给 agent 做父进程看门狗）
    pub pid: u32,
    /// 默认产品（S5：单产品；S6 按声明/请求路由到不同产品）
    pub default_product: String,
    /// 默认工作区（客户端没指定时）
    pub default_workspace: String,
    /// 进行中的派活（S6 交互平台）
    pub delegations: Arc<DelegationRegistry>,
    /// 接入鉴权（S7）。默认 [`crate::auth::AuthConfig::open`]（不要求 token，只该在回环上用）。
    pub auth: Arc<crate::auth::AuthConfig>,
    /// 工作区管理（持久化最近工作区列表）
    pub workspaces: Arc<crate::workspaces::WorkspacesStore>,
}

impl Gateway {
    pub fn new(default_product: impl Into<String>, default_workspace: impl Into<String>) -> Self {
        let default_prod_str = default_product.into();
        let default_ws_str = default_workspace.into();
        // 🔴 构造期**不**往工作区存储里记东西：`WorkspacesStore::new(None)` 指用户真实的
        // `~/.a-da/gateway_workspaces.json`，在这里落一次盘就等于"谁 new 一个 Gateway 谁写用户文件"
        // ——测试注入临时存储也来不及（构造时已经写过了）。生产路径由组合根显式记一次
        // （见 `agent-gateway/src/main.rs`），测试则注入自己的存储。
        let ws_store = Arc::new(crate::workspaces::WorkspacesStore::new(None));
        Self {
            registry: Arc::new(AgentRegistry::new()),
            spawn_lock: Arc::new(Mutex::new(())),
            pid: std::process::id(),
            default_product: default_prod_str,
            default_workspace: default_ws_str,
            delegations: Arc::new(DelegationRegistry::new()),
            auth: Arc::new(crate::auth::AuthConfig::open()),
            workspaces: ws_store,
        }
    }

    /// 换成带 token 的鉴权配置（S7）。链式，便于组合根一行装好。
    pub fn with_auth(mut self, auth: crate::auth::AuthConfig) -> Self {
        self.auth = Arc::new(auth);
        self
    }

    /// 已经是共享引用时直接装上（Web UI 登录校验与网关 API 共用同一份凭据表）。
    pub fn with_auth_arc(mut self, auth: Arc<crate::auth::AuthConfig>) -> Self {
        self.auth = auth;
        self
    }

    /// 换成指定的工作区存储（**测试必须传**，否则会写用户真实的
    /// `~/.a-da/gateway_workspaces.json`——历史上 e2e 测试就是这么把一堆
    /// 临时目录记进用户工作区列表的）。
    pub fn with_workspaces(mut self, store: Arc<crate::workspaces::WorkspacesStore>) -> Self {
        self.workspaces = store;
        self
    }

    /// 起一个后台**回收协程**：定时清理已退出条目、回收空闲实例、超上限按 LRU 驱逐。
    ///
    /// 没有它，访问过多少个工作区就会常驻多少个 `ada-coding` 进程。
    pub fn spawn_reaper(self: &Arc<Self>, policy: crate::supervisor::ReapPolicy) -> tokio::task::JoinHandle<()> {
        let gateway = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(policy.interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let (pruned, reaped, evicted) = crate::supervisor::reap_once(&gateway.registry, &policy);
                if pruned + reaped + evicted > 0 {
                    info!(
                        "实例回收：清理死条目 {}、回收空闲 {}、超限驱逐 {}（当前受管 {}）",
                        pruned,
                        reaped,
                        evicted,
                        gateway.registry.len()
                    );
                }
            }
        })
    }

    /// **发现**（`gateway.info`）：无需 token，**不含任何秘密**。
    ///
    /// 刻意不返回 agent 端点与 token——发现通道是**未认证**的，
    /// 往里放端点等于把内部拓扑送给任何能连上端口的人。
    pub fn info_result(&self) -> serde_json::Value {
        serde_json::json!({
            "protocolVersion": "1.0",
            "product": self.default_product,
            "agentCount": self.registry.len(),
            "authRequired": self.auth.requires_token(),
            "allowedOrigins": self.auth.allowed_origins(),
            "capabilities": crate::auth::gateway_capabilities(),
            "note": "发现响应不含 token 与 agent 端点；拿 token 请走部署侧（配置/配对码）",
        })
    }

    /// **交互平台**：派活。
    ///
    /// 返回 `Ok(outcome)` 或**如实的拒绝原因**——深度超限、连不上、目标拒绝、被取消
    /// 都要能被调用方区分（否则 PM agent 只能报"失败了"）。
    pub async fn delegate_result(
        &self,
        agent_id: Option<&str>,
        workspace: Option<&str>,
        task: &str,
        depth: u32,
        // `Some(tid)` = 多轮续跑该线程
        thread_id: Option<String>,
        delegation_id: &str,
        parent_thread_id: Option<String>,
        subagent_id: Option<String>,
    ) -> Result<DelegateOutcome, DelegateError> {
        if task.trim().is_empty() {
            return Err(DelegateError::NoResult {
                reason: "gateway.delegate 需要一个非空 task".to_string(),
            });
        }

        // 目标解析：显式 agentId 优先；否则按工作区找（没有就拉起）
        let instance = match agent_id {
            Some(id) if !id.trim().is_empty() => {
                if let Some(inst) = self.registry.get(id) {
                    inst
                } else {
                    // **只有一个产品**：PM 已降级为 `ada-coding` 的模式预设，
                    // 所以历史 id（`ada-pm` / `pm` / `pm-assistant`）也一律拉起
                    // 同一个产品——"这条会话是 PM 模式"由会话的 mode 表达，不再由产品表达。
                    let prod = "ada-coding";
                    let ws = workspace
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| self.default_workspace.clone());
                    let spec = SpawnSpec::new(prod, &ws);
                    ensure_agent(&spec, &self.registry, &self.spawn_lock, self.pid)
                        .await
                        .map_err(|e| DelegateError::Connect(e.to_string()))?
                }
            }
            _ => {
                let ws = workspace
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.default_workspace.clone());
                let spec = SpawnSpec::new(self.default_product.clone(), &ws);
                ensure_agent(&spec, &self.registry, &self.spawn_lock, self.pid)
                    .await
                    .map_err(|e| DelegateError::Connect(e.to_string()))?
            }
        };

        // 取消标志**先注册再派活**：客户端可能刚发请求就取消，晚了会丢（丢唤醒是经典坑）
        let cancel = self.delegations.open(delegation_id).await;

        delegate(
            &instance,
            task,
            delegation_id,
            depth,
            thread_id,
            cancel,
            self.delegations.clone(),
            DEFAULT_DELEGATION_TIMEOUT,
            parent_thread_id,
            subagent_id,
        )
        .await
    }

    /// 管理面：列出实例（**已剥 token**）。
    pub fn list_agents_result(&self) -> serde_json::Value {
        let agents: Vec<serde_json::Value> =
            self.registry.list().iter().map(|i| i.redacted()).collect();
        serde_json::json!({ "agents": agents, "count": agents.len() })
    }

    /// 管理面：网关自身状态。
    pub fn status_result(&self) -> serde_json::Value {
        serde_json::json!({
            "pid": self.pid,
            "product": self.default_product,
            "defaultWorkspace": self.default_workspace,
            "agentCount": self.registry.len(),
            "routableCount": self.registry.routable().len(),
            "methodPrefix": GATEWAY_METHOD_PREFIX,
        })
    }

    /// 管理面：确保某工作区有实例，返回它的 id 与端点（**不含 token**）。
    pub async fn attach_result(&self, workspace: Option<&str>) -> Result<serde_json::Value, String> {
        let ws = workspace
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.default_workspace.clone());
        if ws.trim().is_empty() {
            return Err("gateway.attach 需要一个 workspace（网关未配置默认工作区）".to_string());
        }
        self.workspaces.record(&ws, Some(&self.default_product));
        let spec = SpawnSpec::new(self.default_product.clone(), &ws);
        let inst = ensure_agent(&spec, &self.registry, &self.spawn_lock, self.pid)
            .await
            .map_err(|e| e.to_string())?;
        Ok(inst.redacted())
    }

    /// 管理面：获取磁盘驱动器与根目录列表
    pub fn fs_roots_result(&self) -> serde_json::Value {
        let recent: Vec<String> = self
            .workspaces
            .get_data()
            .workspaces
            .into_iter()
            .map(|w| w.workspace)
            .collect();
        let roots = crate::fs_service::list_roots(&recent, None);
        serde_json::to_value(&roots).unwrap_or_else(|_| serde_json::json!([]))
    }

    /// 管理面：浏览目录树与文件列表
    pub fn fs_list_directory_result(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let directories_only = params
            .get("directoriesOnly")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let show_hidden = params
            .get("showHidden")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|l| l as usize);

        let listing = crate::fs_service::list_directory(
            path,
            directories_only,
            show_hidden,
            limit,
            None,
        )?;
        serde_json::to_value(listing).map_err(|e| e.to_string())
    }

    /// 管理面：在指定目录下新建文件夹
    pub fn fs_make_directory_result(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let raw_path = params
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let parent_param = params
            .get("parentPath")
            .or_else(|| params.get("parent"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name_param = params
            .get("folderName")
            .or_else(|| params.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let (parent, name) = if !parent_param.is_empty() && !name_param.is_empty() {
            (parent_param.to_string(), name_param.to_string())
        } else if !raw_path.is_empty() {
            let p = std::path::Path::new(raw_path);
            let n = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let parent_dir = p.parent().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            (parent_dir, n)
        } else {
            (parent_param.to_string(), name_param.to_string())
        };

        let created_path = crate::fs_service::make_directory(&parent, &name, None)?;
        Ok(serde_json::json!({
            "path": created_path,
            "created": true,
        }))
    }

    /// 管理面：获取最近工作区列表
    pub fn workspaces_list_result(&self) -> serde_json::Value {
        let data = self.workspaces.get_data();
        serde_json::to_value(data).unwrap_or_else(|_| serde_json::json!({ "workspaces": [] }))
    }

    /// 管理面：移除最近工作区
    pub fn workspaces_remove_result(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let ws = params
            .get("workspace")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if ws.is_empty() {
            return Err("gateway.workspaces.remove 需要 workspace 参数".to_string());
        }
        let removed = self.workspaces.remove(ws);
        Ok(serde_json::json!({
            "removed": removed,
            "workspace": ws,
        }))
    }
}

/// axum 路由的共享状态。
///
/// 只装网关自己的东西：页面由 `agent-web` 的 SSR 子 service 提供（2026-10：
/// React 桌面前端 `desktop-ui/` 已移除，不再有“静态前端产物托管”这个字段）。
#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<Gateway>,
    pub default_workspace: String,
}

/// 已升级连接的**适配层**：对外保持 `tungstenite::Message` 语义。
///
/// 为什么不在 relay 内部直接换成 axum 的消息类型：这条链条的另一头（连 agent 实例）
/// 本来就是 `tokio_tungstenite`，而且两个方向都是**原样转发**（不解析、不改写）。
/// 两端用同一种消息类型，转发路径才不需要来回转换；转换只发生在这层薄薄的边界上。
pub struct ClientSocket {
    inner: WebSocket,
}

impl ClientSocket {
    pub fn new(inner: WebSocket) -> Self {
        Self { inner }
    }
}

fn ax_to_wire(msg: AxMessage) -> Message {
    match msg {
        AxMessage::Text(t) => Message::Text(t.as_str().to_string().into()),
        AxMessage::Binary(b) => Message::Binary(b),
        AxMessage::Ping(p) => Message::Ping(p),
        AxMessage::Pong(p) => Message::Pong(p),
        AxMessage::Close(c) => Message::Close(c.map(|f| CloseFrame {
            code: CloseCode::from(f.code),
            reason: f.reason.as_str().to_string().into(),
        })),
    }
}

fn wire_to_ax(msg: Message) -> AxMessage {
    match msg {
        Message::Text(t) => AxMessage::Text(t.as_str().to_string().into()),
        Message::Binary(b) => AxMessage::Binary(b.into()),
        Message::Ping(p) => AxMessage::Ping(p.into()),
        Message::Pong(p) => AxMessage::Pong(p.into()),
        Message::Close(c) => AxMessage::Close(c.map(|f| AxCloseFrame {
            code: u16::from(f.code),
            reason: f.reason.as_str().to_string().into(),
        })),
        // 低层帧（只有 `Message::Frame` 构造 API 会产生，relay 不走这条路）：
        // 转成空二进制而不是 panic——对端应当看到"这条帧没内容"，而不是连接被炸掉。
        Message::Frame(_) => AxMessage::Binary(Vec::new().into()),
    }
}

impl Stream for ClientSocket {
    type Item = Result<Message, axum::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(msg))) => Poll::Ready(Some(Ok(ax_to_wire(msg)))),
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Sink<Message> for ClientSocket {
    type Error = axum::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.get_mut().inner).start_send(wire_to_ax(item))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}

/// `/rpc` 的 handler：**先鉴权，再升级**。
///
/// 与旧实现（`accept_hdr_async` 的握手回调）的区别只有位置：拒绝现在是一个普通的
/// HTTP 状态码（403/401），浏览器和 curl 都能直接看到原因，也不必为了拒绝去造一个空响应。
/// 鉴权顺序保持不变，而且都在 `on_upgrade` **之前**——先升级再鉴权会让一个不带 token 的
/// 连接也能触发 `ensure_agent`（真的把 agent 进程拉起来）。
pub async fn ws_handler(
    State(state): State<AppState>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    // 不是升级请求：这是浏览器地址栏直接点了 `/rpc`，给 426 比 400 更能说明问题
    let Ok(ws) = ws else {
        return (
            StatusCode::UPGRADE_REQUIRED,
            "该路径是 WebSocket 端点（/rpc）；浏览页面请访问 /",
        )
            .into_response();
    };

    let gateway = state.gateway.clone();
    let uri_text = uri.to_string();
    let token = query_param(&uri_text, "token");
    let workspace = query_param(&uri_text, "workspace");
    let role = query_param(&uri_text, "role");
    let product = query_param(&uri_text, "product");

    let origin = headers.get("origin").and_then(|v| v.to_str().ok());
    if let Err(e) = gateway.auth.check_origin(origin) {
        warn!("拒绝接入：{e}");
        return (StatusCode::FORBIDDEN, e.to_string()).into_response();
    }
    // 坏 token 直接拒（**不退化成 Anonymous**——那是静默降级）
    let outcome = match gateway.auth.authorize(token.as_deref()) {
        Ok(outcome) => outcome,
        Err(e) => {
            warn!("拒绝接入：{e}");
            return (StatusCode::UNAUTHORIZED, e.to_string()).into_response();
        }
    };

    let default_workspace = state.default_workspace.clone();
    ws.on_upgrade(move |socket| async move {
        let socket = ClientSocket::new(socket);
        let result = match outcome {
            // **未认证连接不绑定任何 agent**：它只能发现，不能触达。
            AuthOutcome::Anonymous => {
                info!("未认证连接接入（只允许 gateway.info 发现）");
                serve_anonymous(gateway, socket).await
            }
            AuthOutcome::Scoped(scope) => {
                // **Agent / 控制面专用连接**：不触发 ensure_agent，直接进自注册通道
                if matches!(role.as_deref(), Some("agent") | Some("control")) {
                    info!("Agent / 控制面专用连接已接入");
                    serve_agent_control(gateway, socket).await
                } else {
                    bind_and_relay(gateway, socket, workspace, product, default_workspace, scope)
                        .await
                }
            }
        };
        if let Err(e) = result {
            warn!("客户端连接结束：{e}");
        }
    })
}

/// 绑定实例并把这条连接交给它（原 `serve_client` 的后半段）。
async fn bind_and_relay(
    gateway: Arc<Gateway>,
    socket: ClientSocket,
    uri_workspace: Option<String>,
    uri_product: Option<String>,
    default_workspace: String,
    scope: TokenScope,
) -> Result<(), anyhow::Error> {
    // 工作区：URL 上的 `?workspace=` 优先，其次网关默认值
    let ws = uri_workspace
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(default_workspace);

    // 作用域检查：token 只能碰自己那几个工作区
    if !scope.allows(&ws) {
        return Err(anyhow::anyhow!(
            "{}",
            AuthError::WorkspaceNotInScope {
                workspace: ws.clone()
            }
        ));
    }

    let target_product = uri_product.unwrap_or_else(|| gateway.default_product.clone());
    gateway.workspaces.record(&ws, Some(&target_product));
    let spec = SpawnSpec::new(target_product, &ws);
    let instance = ensure_agent(&spec, &gateway.registry, &gateway.spawn_lock, gateway.pid)
        .await
        .map_err(|e| anyhow::anyhow!("绑定 agent 失败：{e}"))?;
    info!("客户端已接入网关，绑定实例 {}", instance.id);

    relay(gateway, socket, instance).await
}

/// `gateway.register` 的**唯一**实现。
///
/// 两个入口都需要它：控制面（`serve_agent_control`，agent 进程自注册）与管理面
/// （`relay`，客户端代注册）。此前两份各抄一遍，差异只有变量名——而且**不等价**：
/// 只有控制面那份把 `agent_id` 记了下来用于断连清理，管理面那份不记，
/// 于是从管理面注册的实例断连后没有清理路径，只能等 15 分钟空闲回收（`ReapPolicy`）。
///
/// 返回 `(agent_id, 响应体)`：`agent_id` 必须由调用方持有到连接结束，断开时标记离线。
fn handle_register(
    gateway: &Gateway,
    params: &serde_json::Value,
) -> Result<(String, serde_json::Value), String> {
    let product = params.get("product").and_then(|v| v.as_str()).unwrap_or("");
    let workspace = params.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
    let endpoint = params.get("endpoint").and_then(|v| v.as_str()).unwrap_or("");
    let pid = params.get("pid").and_then(|v| v.as_u64()).map(|p| p as u32);

    // 自述服务的产品清单（可选）：一个实例可能同时服务 coding 与 pm
    let products: Vec<String> = params
        .get("products")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<String>>()
        })
        .filter(|list| !list.is_empty())
        .unwrap_or_else(|| vec![product.to_string()]);

    if product.is_empty() || workspace.is_empty() || endpoint.is_empty() {
        return Err("gateway.register 参数缺失：需要 product, workspace, endpoint".to_string());
    }

    let norm_ws = normalize_workspace(workspace);
    let agent_id = AgentRegistry::id_for_workspace(product, &norm_ws);
    let inst = AgentInstance {
        id: agent_id.clone(),
        product: product.to_string(),
        workspace: norm_ws.to_string_lossy().to_string(),
        endpoint: endpoint.to_string(),
        status: AgentStatus::Ready,
        pid,
        started_at: now_ms(),
        products,
        last_active_ms: now_ms(),
    };
    let is_new = gateway.registry.register(inst);
    info!("Agent 成功向网关注册: {} -> {} (pid: {:?})", agent_id, endpoint, pid);

    let resp = serde_json::json!({
        "registered": true,
        "id": agent_id,
        "isNew": is_new,
    });
    Ok((agent_id, resp))
}

/// 连接断开时把"本连接注册的实例"标记离线。
///
/// 用 `Drop` 而不是在循环末尾手动调：`relay()` 里有多处 `?` 提前返回，
/// 手动收尾必然漏一条路径（`ConnGuard` 用的是同一个理由）。
struct RegisteredAgentGuard {
    registry: Arc<AgentRegistry>,
    id: Option<String>,
}

impl Drop for RegisteredAgentGuard {
    fn drop(&mut self) {
        if let Some(id) = &self.id {
            warn!("连接已关闭，标记注册实例离线: {id}");
            self.registry.set_status(id, AgentStatus::Stopped);
        }
    }
}

/// Agent 控制与自注册通道：专用于 Agent 进程向网关自报家门与保活。
async fn serve_agent_control(
    gateway: Arc<Gateway>,
    client_ws: ClientSocket,
) -> Result<(), anyhow::Error> {
    let (mut tx, mut rx) = client_ws.split();
    let mut registered_agent_id: Option<String> = None;

    while let Some(msg) = rx.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                warn!("读取 Agent 控制帧失败：{e}");
                break;
            }
        };
        if msg.is_close() {
            break;
        }
        let Message::Text(text) = msg else { continue };
        let Ok(req) = serde_json::from_str::<agent_proto::JsonRpcRequest>(&text) else {
            continue;
        };

        let id = req.id.clone();
        let params = req.params.clone().unwrap_or(serde_json::Value::Null);

        let out: Result<serde_json::Value, String> = if let Some(method) = req.method.strip_prefix(GATEWAY_METHOD_PREFIX) {
            match method {
                "register" => handle_register(&gateway, &params).map(|(agent_id, resp)| {
                    registered_agent_id = Some(agent_id);
                    resp
                }),
                "info" => Ok(gateway.info_result()),
                "listAgents" => Ok(gateway.list_agents_result()),
                "status" => Ok(gateway.status_result()),
                other => Err(format!("Agent 控制连接不支持方法：{GATEWAY_METHOD_PREFIX}{other}")),
            }
        } else {
            Err(format!("Agent 控制连接仅接受 gateway.* 方法，收到: {}", req.method))
        };

        let frame = match out {
            Ok(result) => agent_proto::JsonRpcResponse::<serde_json::Value>::success(id, result),
            Err(msg) => agent_proto::JsonRpcResponse::<serde_json::Value>::error(
                id,
                agent_proto::ProtocolError::new(
                    agent_proto::RpcErrorCode::InvalidRequest.code(),
                    msg,
                    None,
                ),
            ),
        };
        tx.send(Message::Text(serde_json::to_string(&frame)?.into())).await?;
    }

    if let Some(agent_id) = registered_agent_id {
        warn!("Agent 控制连接已关闭，标记实例离线: {}", agent_id);
        gateway.registry.set_status(&agent_id, AgentStatus::Stopped);
    }

    Ok(())
}

/// 未认证连接：**只答 `gateway.info`**，其余一律拒绝。
///
/// 为什么不直接断开：浏览器要能先问"这是不是网关、要不要 token"。
/// 但发现响应里**不得有任何秘密**（无 token、无 agent 端点）。
async fn serve_anonymous(
    gateway: Arc<Gateway>,
    client_ws: ClientSocket,
) -> Result<(), anyhow::Error> {
    let (mut tx, mut rx) = client_ws.split();
    while let Some(msg) = rx.next().await {
        let msg = msg?;
        let Message::Text(text) = msg else { continue };
        let Ok(req) = serde_json::from_str::<agent_proto::JsonRpcRequest>(&text) else {
            continue;
        };
        // `gateway.pair` 也走匿名通道：**浏览器此刻还没有 token**，
        // 配对码就是它换取 token 的那一步。
        let frame = if req.method == "gateway.info" {
            agent_proto::JsonRpcResponse::<serde_json::Value>::success(
                req.id,
                gateway.info_result(),
            )
        } else if req.method == "gateway.pair" {
            let code = req
                .params
                .as_ref()
                .and_then(|p| p.get("code"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match gateway.auth.redeem_pairing_code(code) {
                Ok((token, scope)) => {
                    info!("配对成功：已发放一个 token（作用域 {}）", scope.describe());
                    agent_proto::JsonRpcResponse::<serde_json::Value>::success(
                        req.id,
                        serde_json::json!({
                            "token": token,
                            "expiresInSec": crate::auth::PAIRING_TTL.as_secs(),
                            "scope": scope.describe(),
                            "note": "把 token 带在后续连接的 `?token=` 上",
                        }),
                    )
                }
                Err(e) => agent_proto::JsonRpcResponse::<serde_json::Value>::error(
                    req.id,
                    agent_proto::ProtocolError::new(
                        agent_proto::RpcErrorCode::InvalidRequest.code(),
                        e.to_string(),
                        None,
                    ),
                ),
            }
        } else {
            agent_proto::JsonRpcResponse::<serde_json::Value>::error(
                req.id,
                agent_proto::ProtocolError::new(
                    agent_proto::RpcErrorCode::InvalidRequest.code(),
                    format!(
                        "未认证连接只能调用 `gateway.info` 或 `gateway.pair`（当前：`{}`）；\
                         已有 token 请带在 URL 的 `?token=…` 上",
                        req.method
                    ),
                    None,
                ),
            )
        };
        tx.send(Message::Text(serde_json::to_string(&frame)?.into()))
            .await?;
    }
    Ok(())
}

/// 双向透传。
async fn relay(
    gateway: Arc<Gateway>,
    client_ws: ClientSocket,
    instance: AgentInstance,
) -> Result<(), anyhow::Error> {
    // 🔴 记账：这条实例现在挂着几个客户端。有连接就不许回收
    // （用户可能正对着一条长回答发呆，掐掉会莫名其妙掉线）。
    // 用 guard 保证**任何**返回路径（含 `?` 提前返回与 panic 展开）都会减回来。
    gateway.registry.conn_inc(&instance.id);
    // 长连接计数是回收器的唯一依据：有客户端连着就不回收。
    // 同一个守卫也被进程内 Web UI 的事件订阅复用（见 `web.rs`）。
    let _conn_guard = gateway.registry.open_connection(instance.id.clone());

    // 本连接通过 `gateway.register` 注册的实例：断开时标记离线（与控制面同一语义）
    let mut registered = RegisteredAgentGuard {
        registry: gateway.registry.clone(),
        id: None,
    };

    // 连到 agent 实例：端点可能是回环 TCP，也可能是本地管道
    // （Windows 命名管道 / Unix socket）——形态由 `link` 解析，帧与鉴权完全一致。
    let endpoint = crate::link::AgentEndpoint::parse(&instance.endpoint)
        .map_err(|e| anyhow::anyhow!(e))?;
    let agent_ws = endpoint.connect().await?;

    let (mut client_sink, mut client_rx) = client_ws.split();
    let (mut agent_tx, mut agent_rx) = agent_ws.split();

    // 客户端侧**单写者**：agent 转发与管理面响应都往这条通道投递，
    // 由一个写协程独占 sink。这样两路写入不会互相打断帧
    // （与 `agent-rpc` 的 `WsHostServer` 同一手法）。
    let (client_tx, mut client_out_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    let write_task = tokio::spawn(async move {
        while let Some(msg) = client_out_rx.recv().await {
            if client_sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // agent → 客户端：**原样转发**（含连上后 agent 自己推的 seq=0 种子快照）。
    // 同时 `touch`：只要 agent 还在吐帧（长跑的一轮对话），这条实例就不算空闲。
    let tx_for_agent = client_tx.clone();
    let reg_for_agent = gateway.registry.clone();
    let id_for_agent = instance.id.clone();
    let agent_to_client = tokio::spawn(async move {
        while let Some(msg) = agent_rx.next().await {
            match msg {
                Ok(m) => {
                    reg_for_agent.touch(&id_for_agent);
                    let is_close = m.is_close();
                    if tx_for_agent.send(m).is_err() {
                        break;
                    }
                    if is_close {
                        break;
                    }
                }
                Err(e) => {
                    warn!("读取 agent 帧失败：{e}");
                    break;
                }
            }
        }
    });

    // 客户端 → 网关/agent
    let gw = gateway.clone();
    while let Some(msg) = client_rx.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                warn!("读取客户端帧失败：{e}");
                break;
            }
        };
        if msg.is_close() {
            break;
        }

        // 客户端还有话说 → 这条实例是活跃的（空闲回收据此判据）
        gateway.registry.touch(&instance.id);

        let Message::Text(text) = msg else {
            // 二进制帧：协议只有文本帧，如实拒绝而不是静默丢弃
            let err = agent_proto::JsonRpcResponse::<()>::error(
                None,
                agent_proto::ProtocolError::new(
                    agent_proto::RpcErrorCode::InvalidRequest.code(),
                    "网关只接受文本帧",
                    None,
                ),
            );
            let _ = client_tx.send(Message::Text(serde_json::to_string(&err)?.into()));
            continue;
        };

        // 解析到"方法名"这一层就够——不改内容，只决定谁答
        let parsed: Result<agent_proto::JsonRpcRequest, _> = serde_json::from_str(&text);
        let req = match parsed {
            Ok(r) => r,
            Err(_) => {
                let err = agent_proto::JsonRpcResponse::<()>::error(
                    None,
                    agent_proto::ProtocolError::new(
                        agent_proto::RpcErrorCode::ParseError.code(),
                        "不是合法的 JSON-RPC 请求",
                        None,
                    ),
                );
                let _ = client_tx.send(Message::Text(serde_json::to_string(&err)?.into()));
                continue;
            }
        };

        if let Some(method) = req.method.strip_prefix(GATEWAY_METHOD_PREFIX) {
            // ── 管理面 / 交互平台：网关自己答 ──
            let id = req.id.clone();
            let params = req.params.clone().unwrap_or(serde_json::Value::Null);

            // `gateway.delegate` 是**长任务**：必须在独立任务里跑，绝不能在这个循环里 await。
            //
            // 原因：取消（`gateway.cancelDelegation`）是**同一连接上的另一帧**。
            // 若循环被派活阻塞住，那一帧永远读不到——取消就永远到不了。
            // 所以这里 spawn 出去，结果**稍后**用同一个 `id` 发回（JSON-RPC 允许延迟响应）。
            if method == "delegate" {
                let task = params
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let agent_id = params
                    .get("agentId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let workspace = params
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                // 深度：**缺省即拒绝**（0），不默认成 1——拿不到依据时不放开
                let depth = params.get("depth").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                // 多轮：带 threadId 则续跑已有线程
                let thread_id = params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let delegation_id = params
                    .get("delegationId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());

                let parent_thread_id = params
                    .get("parentThreadId")
                    .or_else(|| params.get("parentId"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let subagent_id = params
                    .get("subagentId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| agent_id.clone());

                let gw_task = gw.clone();
                let tx_task = client_tx.clone();
                tokio::spawn(async move {
                    let out = gw_task
                        .delegate_result(
                            agent_id.as_deref(),
                            workspace.as_deref(),
                            &task,
                            depth,
                            thread_id,
                            &delegation_id,
                            parent_thread_id,
                            subagent_id,
                        )
                        .await;
                    let frame = match out {
                        Ok(DelegateOutcome { agent_id, thread_id, summary, details, .. }) => {
                            agent_proto::JsonRpcResponse::<serde_json::Value>::success(
                                id,
                                serde_json::json!({
                                    "ok": true,
                                    "delegationId": delegation_id,
                                    "agentId": agent_id,
                                    "threadId": thread_id,
                                    "summary": summary,
                                    "details": details,
                                }),
                            )
                        }
                        Err(e) => {
                            let code = match &e {
                                DelegateError::DepthExceeded { .. } => {
                                    agent_proto::RpcErrorCode::InvalidRequest.code()
                                }
                                _ => agent_proto::RpcErrorCode::InternalError.code(),
                            };
                            agent_proto::JsonRpcResponse::<serde_json::Value>::error(
                                id,
                                agent_proto::ProtocolError::new(code, e.to_string(), None),
                            )
                        }
                    };
                    let _ = tx_task.send(Message::Text(serde_json::to_string(&frame).unwrap_or_default().into()));
                });
                continue;
            }

            let out: Result<serde_json::Value, String> = match method {
                "register" => handle_register(&gw, &params).map(|(agent_id, resp)| {
                    // 与 `serve_agent_control` 同等对待：断连时由 guard 标记离线
                    registered.id = Some(agent_id);
                    resp
                }),
                "info" => Ok(gw.info_result()),
                "listAgents" => Ok(gw.list_agents_result()),
                "status" => Ok(gw.status_result()),
                "attach" => {
                    let ws_param = params.get("workspace").and_then(|v| v.as_str());
                    gw.attach_result(ws_param).await
                }
                "fs.roots" => Ok(gw.fs_roots_result()),
                "fs.listDirectory" => gw.fs_list_directory_result(&params),
                "fs.makeDirectory" => gw.fs_make_directory_result(&params),
                "workspaces.list" => Ok(gw.workspaces_list_result()),
                "workspaces.remove" => gw.workspaces_remove_result(&params),
                "cancelDelegation" => {
                    let did = params
                        .get("delegationId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if did.is_empty() {
                        Err("gateway.cancelDelegation 需要 delegationId".to_string())
                    } else if gw.delegations.cancel(did).await {
                        Ok(serde_json::json!({
                            "cancelled": true,
                            "delegationId": did,
                            "note": "取消标志已置；派活任务会在下一帧把 thread.abort 打到目标",
                        }))
                    } else {
                        // 刻意不区分"已完成"与"id 写错"：网关不缓存已完成派活的历史
                        // （那会是"网关持有会话状态"的开端）
                        Ok(serde_json::json!({
                            "cancelled": false,
                            "delegationId": did,
                            "note": "没有这个进行中的派活（已完成或 id 不存在）",
                        }))
                    }
                }
                "detach" => {
                    let agent_id = params.get("agentId").and_then(|v| v.as_str()).unwrap_or("");
                    match gw.registry.remove(agent_id) {
                        Some(gone) => Ok(serde_json::json!({
                            "detached": true,
                            "agentId": gone.id,
                            "note": "仅从注册表移除；进程由它的父进程看门狗回收",
                        })),
                        None => Err(format!("没有这个 agent 实例：{agent_id}")),
                    }
                }
                other => Err(format!("网关没有这个方法：{GATEWAY_METHOD_PREFIX}{other}")),
            };

            let frame = match out {
                Ok(result) => agent_proto::JsonRpcResponse::<serde_json::Value>::success(id, result),
                Err(msg) => agent_proto::JsonRpcResponse::<serde_json::Value>::error(
                    id,
                    agent_proto::ProtocolError::new(
                        agent_proto::RpcErrorCode::MethodNotFound.code(),
                        msg,
                        None,
                    ),
                ),
            };
            let _ = client_tx.send(Message::Text(serde_json::to_string(&frame)?.into()));
            continue;
        }

        // ── 其余：**原样透传**（不改 id / seq / call_id）──
        if agent_tx.send(Message::Text(text)).await.is_err() {
            warn!("转发到 agent 失败（实例 {}）", instance.id);
            break;
        }
    }

    agent_to_client.abort();
    drop(client_tx);
    let _ = write_task.await;
    Ok(())
}
