use agent_proto::*;
use crate::server::dispatch::Dispatcher;
use crate::state::{generate_snapshot, AgentStore};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

pub struct WsHostServer {
    pub port: u16,
    pub token: String,
    store: Arc<RwLock<AgentStore>>,
    dispatcher: Arc<Dispatcher>,
    broadcaster: Arc<crate::server::emitter::StateBroadcaster>,
}

impl WsHostServer {
    /// 绑定并启动宿主（不注入真引擎 → 走 legacy 主循环，与切换前行为一致）。
    pub async fn bind(
        bind_port: u16,
        token: String,
        store: Arc<RwLock<AgentStore>>,
    ) -> Result<Arc<Self>, anyhow::Error> {
        Self::bind_with_engine(bind_port, token, store, None).await
    }

    /// W3-T2：绑定并注入**真引擎**（`AgentRuntime`）。
    ///
    /// 注入之后走哪条由 `A_DA_ENGINE` 决定（默认 `legacy`，保证可回滚）；
    /// 设 `A_DA_ENGINE=runtime` 才真的切到 `AgentRuntime::run_turn`。
    ///
    /// `injection` 同时带来引擎审批闸门用的 waiter 表——**必须用它**，
    /// 否则 UI 的批准落到另一张表上，闸门会静默等到超时（见 [`EngineInjection`]）。
    pub async fn bind_with_engine(
        bind_port: u16,
        token: String,
        store: Arc<RwLock<AgentStore>>,
        injection: Option<crate::server::dispatch::EngineInjection>,
    ) -> Result<Arc<Self>, anyhow::Error> {
        Self::bind_with_engines(bind_port, token, store, injection, None).await
    }

    /// 绑定并同时注入多个真引擎（如 `ada-coding` 主引擎与 `pm-assistant` PM 引擎）。
    pub async fn bind_with_engines(
        bind_port: u16,
        token: String,
        store: Arc<RwLock<AgentStore>>,
        coding_injection: Option<crate::server::dispatch::EngineInjection>,
        pm_injection: Option<crate::server::dispatch::EngineInjection>,
    ) -> Result<Arc<Self>, anyhow::Error> {
        let addr = SocketAddr::from(([127, 0, 0, 1], bind_port));
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        let actual_port = local_addr.port();

        info!("WebSocket 服务端已在 127.0.0.1:{} 成功绑定", actual_port);

        let (broadcast_tx, mut broadcast_rx) = mpsc::unbounded_channel::<String>();
        let client_senders = Arc::new(RwLock::new(Vec::<mpsc::UnboundedSender<Message>>::new()));

        // 广播转发后台协程
        let senders_clone = client_senders.clone();
        tokio::spawn(async move {
            while let Some(msg_str) = broadcast_rx.recv().await {
                let mut senders = senders_clone.write().await;
                senders.retain(|tx| {
                    tx.send(Message::Text(msg_str.clone().into())).is_ok()
                });
            }
        });

        let session_mgr = Arc::new(agent_node::session::SessionManager::new(None));
        let checkpoint_mgr = Arc::new(agent_node::checkpoint::CheckpointManager::new(None));
        let subagent_mgr = Arc::new(agent_node::subagents::SubagentManager::new());
        // 审批 waiter 表：优先使用 coding 引擎的审批表（UI 决策必须落到同一张表）
        let approval_mgr = match (&coding_injection, &pm_injection) {
            (Some(i), _) => i.approval_mgr.clone(),
            (None, Some(i)) => i.approval_mgr.clone(),
            (None, None) => Arc::new(agent_node::approval::ApprovalManager::new()),
        };
        let plugin_mgr = Arc::new(agent_node::plugins::PluginManager::new());
        let skill_mgr = Arc::new(agent_node::skills::SkillManager::new());
        let seq = Arc::new(AtomicU64::new(0));
        let broadcaster = crate::server::emitter::StateBroadcaster::new(
            store.clone(),
            seq.clone(),
            broadcast_tx.clone(),
        );
        let dispatcher = Arc::new(
            Dispatcher::new(
                store.clone(),
                session_mgr,
                checkpoint_mgr,
                subagent_mgr,
                approval_mgr,
                plugin_mgr,
                skill_mgr,
                Some(broadcaster.clone()),
            )
            // 宿主注入主引擎与 PM 引擎
            .pipe_engine(coding_injection)
            .pipe_pm_engine(pm_injection),
        );

        let server = Arc::new(Self {
            port: actual_port,
            token: token.clone(),
            store: store.clone(),
            dispatcher: dispatcher.clone(),
            broadcaster,
        });

        // 接收外部连接后台协程
        let server_clone = server.clone();
        tokio::spawn(async move {
            while let Ok((stream, peer_addr)) = listener.accept().await {
                let srv = server_clone.clone();
                let senders = client_senders.clone();
                tokio::spawn(async move {
                    if let Err(e) = srv.handle_connection(stream, peer_addr, senders).await {
                        warn!("客户端连接断开或异常 ({}): {}", peer_addr, e);
                    }
                });
            }
        });

        Ok(server)
    }

    /// 广播全量快照
    pub async fn broadcast_snapshot(&self) {
        self.broadcaster.broadcast_immediate().await;
    }

    async fn handle_connection(
        &self,
        stream: TcpStream,
        peer_addr: SocketAddr,
        client_senders: Arc<RwLock<Vec<mpsc::UnboundedSender<Message>>>>,
    ) -> Result<(), anyhow::Error> {
        let expected_token = self.token.clone();
        let mut token_valid = false;

        // WebSocket 握手门禁校验
        let callback = |req: &Request, resp: Response| {
            let uri = req.uri();
            let query = uri.query().unwrap_or("");
            for pair in query.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == "token" && v == expected_token {
                        token_valid = true;
                        break;
                    }
                }
            }
            if !token_valid {
                let status = tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
                let unauth_resp = Response::builder()
                    .status(status)
                    .body(Some("Unauthorized".to_string()))
                    .unwrap();
                return Err(unauth_resp);
            }
            Ok(resp)
        };

        let ws_stream = tokio_tungstenite::accept_hdr_async(stream, callback).await?;
        info!("客户端连接已通过令牌验证并建立 WebSocket 通道: {}", peer_addr);

        let (mut ws_sink, mut ws_stream) = ws_stream.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

        // 注册到全局广播列表
        {
            let mut senders = client_senders.write().await;
            senders.push(tx.clone());
        }

        // 推送首帧种子快照 (seq = 0，对齐协议 §1.3)
        {
            let store = self.store.read().await;
            let snapshot = generate_snapshot(&store);
            let seed_event = SnapshotEvent {
                seq: 0,
                topic: EVT_STATE_SNAPSHOT.to_string(),
                payload: snapshot,
            };
            let seed_frame = serde_json::json!({
                "jsonrpc": "2.0",
                "method": EVT_STATE_SNAPSHOT,
                "params": seed_event
            });
            let _ = tx.send(Message::Text(serde_json::to_string(&seed_frame)?.into()));
        }

        // 出站写入协程
        let write_task = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if ws_sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        let mut initialized = false;

        // 入站帧处理循环
        while let Some(msg_res) = ws_stream.next().await {
            let msg = match msg_res {
                Ok(m) => m,
                Err(e) => {
                    warn!("读取帧错误: {}", e);
                    break;
                }
            };

            if msg.is_close() {
                break;
            }

            if let Message::Text(text) = msg {
                let req: JsonRpcRequest = match serde_json::from_str(&text) {
                    Ok(r) => r,
                    Err(_) => {
                        let err_resp = JsonRpcResponse::<()>::error(
                            None,
                            ProtocolError::new(RpcErrorCode::ParseError.code(), "不是合法的 JSON", None),
                        );
                        let _ = tx.send(Message::Text(serde_json::to_string(&err_resp)?.into()));
                        continue;
                    }
                };

                // 握手前只接受 session.initialize（协议 §1.2）
                if !initialized && req.method != SESSION_INITIALIZE {
                    let err_resp = JsonRpcResponse::<()>::error(
                        req.id,
                        ProtocolError::new(
                            AppErrorCode::Unauthorized.code(),
                            "握手前只接受 session.initialize",
                            Some(serde_json::json!({ "what": req.method })),
                        ),
                    );
                    let _ = tx.send(Message::Text(serde_json::to_string(&err_resp)?.into()));
                    continue;
                }

                let is_init_method = req.method == SESSION_INITIALIZE;
                let params = req.params.unwrap_or(serde_json::Value::Null);
                let dispatch_res = self.dispatcher.dispatch(&req.method, params).await;

                match dispatch_res {
                    Ok(result) => {
                        if is_init_method {
                            initialized = true;
                        }
                        let resp = JsonRpcResponse::success(req.id, result);
                        let _ = tx.send(Message::Text(serde_json::to_string(&resp)?.into()));
                        // 触发一次快照对齐
                        self.broadcast_snapshot().await;
                    }
                    Err(err) => {
                        let resp = JsonRpcResponse::<()>::error(req.id, err);
                        let _ = tx.send(Message::Text(serde_json::to_string(&resp)?.into()));
                    }
                }
            }
        }

        write_task.abort();
        Ok(())
    }
}
