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

use crate::registry::{AgentInstance, AgentRegistry};
use crate::supervisor::{ensure_agent, SpawnSpec};

/// 网关自己处理的方法前缀。
///
/// 前缀之外的一律透传——**白名单**而不是黑名单：新增协议方法时网关不需要改，
/// 也不会误吞节点的方法。
pub const GATEWAY_METHOD_PREFIX: &str = "gateway.";

/// 网关管理面的方法名。
pub mod methods {
    pub const LIST_AGENTS: &str = "gateway.listAgents";
    pub const ATTACH: &str = "gateway.attach";
    pub const STATUS: &str = "gateway.status";
    pub const DETACH: &str = "gateway.detach";
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
}

impl Gateway {
    pub fn new(default_product: impl Into<String>, default_workspace: impl Into<String>) -> Self {
        Self {
            registry: Arc::new(AgentRegistry::new()),
            spawn_lock: Arc::new(Mutex::new(())),
            pid: std::process::id(),
            default_product: default_product.into(),
            default_workspace: default_workspace.into(),
        }
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
        let spec = SpawnSpec::new(self.default_product.clone(), &ws);
        let inst = ensure_agent(&spec, &self.registry, &self.spawn_lock, self.pid)
            .await
            .map_err(|e| e.to_string())?;
        Ok(inst.redacted())
    }
}

/// 处理一条客户端连接：先绑实例，再双向透传。
pub async fn serve_client(
    gateway: Arc<Gateway>,
    stream: TcpStream,
    requested_workspace: Option<String>,
) -> Result<(), anyhow::Error> {
    // 1. 绑定实例（此时才真正拉起 agent）
    let ws = requested_workspace
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| gateway.default_workspace.clone());
    let spec = SpawnSpec::new(gateway.default_product.clone(), &ws);
    let instance = ensure_agent(&spec, &gateway.registry, &gateway.spawn_lock, gateway.pid)
        .await
        .map_err(|e| anyhow::anyhow!("绑定 agent 失败：{e}"))?;

    // 2. 与客户端完成 WS 握手（**网关不做 token 校验**：那是上游的事，
    //    网关自己的鉴权在 S7 随远程接入一起做——现在只跑回环）
    let client_ws = tokio_tungstenite::accept_async(stream).await?;
    info!("客户端已接入网关，绑定实例 {}", instance.id);

    relay(gateway, client_ws, instance).await
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
            // ── 管理面：网关自己答 ──
            let id = req.id.clone();
            let params = req.params.clone().unwrap_or(serde_json::Value::Null);
            let out: Result<serde_json::Value, String> = match method {
                "listAgents" => Ok(gw.list_agents_result()),
                "status" => Ok(gw.status_result()),
                "attach" => {
                    let ws_param = params.get("workspace").and_then(|v| v.as_str());
                    gw.attach_result(ws_param).await
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
