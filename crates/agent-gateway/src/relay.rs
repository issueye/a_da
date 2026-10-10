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

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::auth::{query_param, AuthError, AuthOutcome};
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
        let ws_store = Arc::new(crate::workspaces::WorkspacesStore::new(None));
        if !default_ws_str.trim().is_empty() {
            ws_store.record(&default_ws_str, Some(&default_prod_str));
        }
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
                    let prod = match id {
                        "ada-pm" | "pm" | "pm-assistant" => "ada-pm",
                        _ => "ada-coding",
                    };
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

/// 处理一条客户端连接：先绑实例，再双向透传。
pub async fn serve_client(
    gateway: Arc<Gateway>,
    stream: TcpStream,
    requested_workspace: Option<String>,
) -> Result<(), anyhow::Error> {

    // 1. 握手时就地鉴权 + Origin 策略。
    //
    // 为什么在 `accept_hdr_async` 的回调里做而不是握手后：
    // 回调可以返回 `ErrorResponse`，于是拒绝表现为**一个 HTTP 状态码**
    // （401/403）——浏览器与 curl 都能直接看到原因，而不是"连上了又莫名其妙断开"。
    let auth = gateway.auth.clone();
    // 刻意用 `std::sync::Mutex`：这里的临界区**不含 await**（只是拷四个 Option），
    // 用异步锁反而要求跨 await 持锁。与 `Gateway` 里那个 tokio 锁不是一回事。
    let captured: Arc<std::sync::Mutex<(Option<String>, Option<String>, Option<String>, Option<String>)>> =
        Arc::new(std::sync::Mutex::new((None, None, None, None)));
    let cap = captured.clone();

    let client_ws = tokio_tungstenite::accept_hdr_async(
        stream,
        move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
              resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
            let uri = req.uri().to_string();
            let token = query_param(&uri, "token");
            let workspace = query_param(&uri, "workspace");
            let role = query_param(&uri, "role");
            let product = query_param(&uri, "product");
            let origin = req
                .headers()
                .get("origin")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            if let Err(e) = auth.check_origin(origin.as_deref()) {
                warn!("拒绝接入：{e}");
                return Err(handshake_error(403, &e.to_string()));
            }
            // 坏 token 直接拒（**不退化成 Anonymous**——那是静默降级）
            if let Err(e) = auth.authorize(token.as_deref()) {
                warn!("拒绝接入：{e}");
                return Err(handshake_error(401, &e.to_string()));
            }

            *cap.lock().expect("captured 锁") = (token, workspace, role, product);
            Ok(resp)
        },
    )
    .await?;

    let (token, uri_workspace, uri_role, uri_product) = {
        let g = captured.lock().expect("captured 锁");
        g.clone()
    };

    let outcome = gateway
        .auth
        .authorize(token.as_deref())
        .map_err(|e| anyhow::anyhow!("鉴权失败：{e}"))?;

    // 2. **未认证连接不绑定任何 agent**：它只能发现，不能触达。
    //
    // 顺序很要紧：绑定实例会**真的把 agent 进程拉起来**。
    // 先绑定再鉴权的话，一个不带 token 的连接就能让网关起进程（放大攻击面）。
    if matches!(outcome, AuthOutcome::Anonymous) {
        info!("未认证连接接入（只允许 gateway.info 发现）");
        return serve_anonymous(gateway, client_ws).await;
    }
    let AuthOutcome::Scoped(scope) = outcome else {
        unreachable!("Anonymous 已在上面返回");
    };

    // 2.5 **Agent / 控制面专用连接**：如果连接声明了 role=agent 或 role=control，
    // 不需要也不应当触发 ensure_agent 去启动实例，直接进入 Agent 控制与自注册通道。
    if uri_role.as_deref() == Some("agent") || uri_role.as_deref() == Some("control") {
        info!("Agent / 控制面专用连接已接入");
        return serve_agent_control(gateway, client_ws).await;
    }

    // 3. 工作区：URL 上的 `?workspace=` 优先，其次调用方给的默认值
    let ws = uri_workspace
        .or(requested_workspace)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| gateway.default_workspace.clone());

    // 4. 作用域检查：token 只能碰自己那几个工作区
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

    relay(gateway, client_ws, instance).await
}

/// Agent 控制与自注册通道：专用于 Agent 进程向网关自报家门与保活。
async fn serve_agent_control(
    gateway: Arc<Gateway>,
    client_ws: tokio_tungstenite::WebSocketStream<TcpStream>,
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
                "register" => {
                    let product = params.get("product").and_then(|v| v.as_str()).unwrap_or("");
                    let workspace = params.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
                    let endpoint = params.get("endpoint").and_then(|v| v.as_str()).unwrap_or("");
                    let pid = params.get("pid").and_then(|v| v.as_u64()).map(|p| p as u32);
                    if product.is_empty() || workspace.is_empty() || endpoint.is_empty() {
                        Err("gateway.register 参数缺失：需要 product, workspace, endpoint".to_string())
                    } else {
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
                        };
                        let is_new = gateway.registry.register(inst);
                        info!("Agent 成功向网关注册: {} -> {} (pid: {:?})", agent_id, endpoint, pid);
                        registered_agent_id = Some(agent_id.clone());
                        Ok(serde_json::json!({
                            "registered": true,
                            "id": agent_id,
                            "isNew": is_new,
                        }))
                    }
                }
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

/// 造一个握手期的 HTTP 错误响应。
fn handshake_error(
    status: u16,
    msg: &str,
) -> tokio_tungstenite::tungstenite::handshake::server::ErrorResponse {
    // 这个版本的 tungstenite 没有 `ErrorResponse::builder`，
    // `ErrorResponse` 就是 `http::Response<Option<String>>`，直接构造。
    let mut resp =
        tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(Some(msg.to_string()));
    *resp.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::from_u16(status)
        .unwrap_or(tokio_tungstenite::tungstenite::http::StatusCode::BAD_REQUEST);
    resp
}

/// 未认证连接：**只答 `gateway.info`**，其余一律拒绝。
///
/// 为什么不直接断开：浏览器要能先问"这是不是网关、要不要 token"。
/// 但发现响应里**不得有任何秘密**（无 token、无 agent 端点）。
async fn serve_anonymous(
    gateway: Arc<Gateway>,
    client_ws: tokio_tungstenite::WebSocketStream<TcpStream>,
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
    client_ws: tokio_tungstenite::WebSocketStream<TcpStream>,
    instance: AgentInstance,
) -> Result<(), anyhow::Error> {
    // 连到 agent 实例
    let req = instance.endpoint.clone().into_client_request()?;
    let (agent_ws, _resp) = tokio_tungstenite::connect_async(req).await?;

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

    // agent → 客户端：**原样转发**（含连上后 agent 自己推的 seq=0 种子快照）
    let tx_for_agent = client_tx.clone();
    let agent_to_client = tokio::spawn(async move {
        while let Some(msg) = agent_rx.next().await {
            match msg {
                Ok(m) => {
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
                "register" => {
                    let product = params.get("product").and_then(|v| v.as_str()).unwrap_or("");
                    let workspace = params.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
                    let endpoint = params.get("endpoint").and_then(|v| v.as_str()).unwrap_or("");
                    let pid = params.get("pid").and_then(|v| v.as_u64()).map(|p| p as u32);
                    if product.is_empty() || workspace.is_empty() || endpoint.is_empty() {
                        Err("gateway.register 参数缺失：需要 product, workspace, endpoint".to_string())
                    } else {
                        let norm_ws = normalize_workspace(workspace);
                        let id = AgentRegistry::id_for_workspace(product, &norm_ws);
                        let inst = AgentInstance {
                            id: id.clone(),
                            product: product.to_string(),
                            workspace: norm_ws.to_string_lossy().to_string(),
                            endpoint: endpoint.to_string(),
                            status: AgentStatus::Ready,
                            pid,
                            started_at: now_ms(),
                        };
                        let is_new = gw.registry.register(inst);
                        info!("Agent 成功向网关注册: {} -> {} (pid: {:?})", id, endpoint, pid);
                        Ok(serde_json::json!({
                            "registered": true,
                            "id": id,
                            "isNew": is_new,
                        }))
                    }
                }
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
