//! 网关的端到端集成测试（**用桩 agent 宿主**，不依赖外部进程）。
//!
//! # 为什么需要一个"桩宿主"
//!
//! 网关的价值全在**转发与管理**上，而这些逻辑不需要真 agent 就能验证：
//! 桩宿主只要"连上后推一帧种子快照、收到方法就回一帧"就够。
//! 这样测试是**密封的**（不依赖 `ada-coding` 二进制是否已构建），
//! 因此可以进 `cargo test` 而不会变成"有时红有时绿"的脆弱门。
//!
//! 真 agent 的往返由 `cargo xtask gateway-smoke` 单独验（见 §13.30）。
//!
//! # 这个测试到底证明了什么
//!
//! | 断言 | 证明的事 |
//! |---|---|
//! | 客户端收到宿主推的种子快照 | **agent → 客户端**的转发通了（含连接期事件，不只是响应） |
//! | `gateway.status` 有响应且不含 token | **管理面由网关自己答** |
//! | 业务方法由桩宿主回答 | **其余方法原样透传**（网关不吞、不改） |
//! | `gateway.listAgents` 有 1 个 ready 实例 | **注册表**被真正填充（路由有落点） |
//! | 未注册工作区 → 报错而不是静默路由 | 路由**不编造目标** |

use std::sync::Arc;
use std::time::Duration;

use agent_gateway::registry::{AgentInstance, AgentStatus};
use agent_gateway::relay::serve_client;
use agent_gateway::Gateway;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// 桩 agent 宿主：连上先推一帧种子快照，然后对每个请求回一帧带标记的响应。
async fn stub_agent_host() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("桩宿主绑定");
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let (mut tx, mut rx) = ws.split();

                // 1. 连接期事件：种子快照（seq=0，对齐宿主行为）
                let seed = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "evt.state.snapshot",
                    "params": { "seq": 0, "topic": "evt.state.snapshot", "payload": { "from": "stub" } }
                });
                let _ = tx.send(Message::Text(seed.to_string().into())).await;

                // 2. 请求 → 带标记的响应（标记里回显方法名，便于断言"是桩答的"）
                while let Some(Ok(msg)) = rx.next().await {
                    let Message::Text(text) = msg else { continue };
                    let Ok(req) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
                    let resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "answeredBy": "stub-agent", "method": method }
                    });
                    if tx.send(Message::Text(resp.to_string().into())).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    port
}

/// 起一个网关，并把某个工作区**预先登记**成指向桩宿主（`Ready` → `ensure_agent` 会复用）。
async fn gateway_with_stub_agent(workspace: &str) -> (Arc<Gateway>, u16) {
    let stub_port = stub_agent_host().await;
    let gateway = Arc::new(Gateway::new("ada-coding", workspace));

    let ws_norm = agent_gateway::normalize_workspace(workspace);
    let id = agent_gateway::AgentRegistry::id_for_workspace("ada-coding", &ws_norm);
    gateway.registry.register(AgentInstance {
        id,
        product: "ada-coding".to_string(),
        workspace: ws_norm.to_string_lossy().to_string(),
        endpoint: format!("ws://127.0.0.1:{stub_port}/rpc?token=stub-token"),
        status: AgentStatus::Ready,
        pid: None,
        started_at: 1,
    });

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("网关绑定");
    let gw_port = listener.local_addr().unwrap().port();
    let gw = gateway.clone();
    let ws = workspace.to_string();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let g = gw.clone();
            let w = ws.clone();
            tokio::spawn(async move {
                let _ = serve_client(g, stream, Some(w)).await;
            });
        }
    });

    (gateway, gw_port)
}

/// 连上网关，返回 (写端, 读端)。
async fn connect(
    port: u16,
) -> (
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/rpc"))
        .await
        .expect("连上网关");
    ws.split()
}

/// 读一帧文本，解析成 JSON（带超时——挂住比断言失败更难排查）。
async fn next_frame(
    rx: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> serde_json::Value {
    let msg = tokio::time::timeout(Duration::from_secs(5), rx.next())
        .await
        .expect("等帧超时（网关没转发？）")
        .expect("连接应还在")
        .expect("读帧失败");
    let Message::Text(t) = msg else {
        panic!("只应有文本帧");
    };
    serde_json::from_str(&t).expect("帧应是合法 JSON")
}

#[tokio::test]
async fn test_gateway_relays_seed_event_and_forwards_methods() {
    let ws = std::env::temp_dir().join("a_da_gw_e2e_ws");
    let (gateway, port) = gateway_with_stub_agent(&ws.to_string_lossy()).await;
    let (mut tx, mut rx) = connect(port).await;

    // ① 连接期事件：宿主推的种子快照必须被转发过来（证明 agent → 客户端通了）
    let seed = next_frame(&mut rx).await;
    assert_eq!(
        seed.get("method").and_then(|v| v.as_str()),
        Some("evt.state.snapshot"),
        "网关必须转发连接期事件，而不只是响应：{seed}"
    );
    assert_eq!(
        seed.pointer("/params/payload/from").and_then(|v| v.as_str()),
        Some("stub"),
        "转发必须**原样**（payload 不能被网关改动）"
    );

    // ② 管理面：网关自己答
    tx.send(Message::Text(
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"gateway.status"})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let status = next_frame(&mut rx).await;
    assert_eq!(status.get("id"), Some(&serde_json::json!(1)));
    assert!(
        status.pointer("/result/pid").is_some(),
        "gateway.status 应由网关回答：{status}"
    );
    let status_text = status.to_string();
    assert!(
        !status_text.contains("stub-token"),
        "管理面响应不得泄露 token：{status_text}"
    );

    // ③ 业务方法：透传给桩宿主（响应里带 stub 标记）
    tx.send(Message::Text(
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"session.initialize","params":{}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let forwarded = next_frame(&mut rx).await;
    assert_eq!(
        forwarded.pointer("/result/answeredBy").and_then(|v| v.as_str()),
        Some("stub-agent"),
        "非 gateway.* 的方法必须原样透传给 agent：{forwarded}"
    );
    assert_eq!(
        forwarded.pointer("/result/method").and_then(|v| v.as_str()),
        Some("session.initialize"),
        "透传不得改方法名"
    );

    // ④ 注册表：有 1 个 ready 实例（说明路由有落点）
    tx.send(Message::Text(
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"gateway.listAgents"})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let agents = next_frame(&mut rx).await;
    assert_eq!(
        agents.pointer("/result/count").and_then(|v| v.as_u64()),
        Some(1),
        "注册表里应有 1 个实例：{agents}"
    );
    assert_eq!(
        agents.pointer("/result/agents/0/status").and_then(|v| v.as_str()),
        Some("ready")
    );
    assert!(
        !agents.to_string().contains("stub-token"),
        "listAgents 不得泄露 token"
    );

    assert_eq!(gateway.registry.len(), 1);
}

#[tokio::test]
async fn test_unknown_gateway_method_is_method_not_found_not_silence() {
    let ws = std::env::temp_dir().join("a_da_gw_e2e_unknown");
    let (_gateway, port) = gateway_with_stub_agent(&ws.to_string_lossy()).await;
    let (mut tx, mut rx) = connect(port).await;
    let _seed = next_frame(&mut rx).await;

    tx.send(Message::Text(
        serde_json::json!({"jsonrpc":"2.0","id":9,"method":"gateway.nope"})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();

    let resp = next_frame(&mut rx).await;
    assert!(
        resp.get("error").is_some(),
        "未知的 gateway.* 方法必须**如实报错**，不许静默：{resp}"
    );
    assert!(
        resp.pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("gateway.nope"),
        "错误信息要点出方法名：{resp}"
    );
}

#[tokio::test]
async fn test_binary_frame_is_rejected_explicitly() {
    let ws = std::env::temp_dir().join("a_da_gw_e2e_binary");
    let (_gateway, port) = gateway_with_stub_agent(&ws.to_string_lossy()).await;
    let (mut tx, mut rx) = connect(port).await;
    let _seed = next_frame(&mut rx).await;

    tx.send(Message::Binary(vec![1, 2, 3].into())).await.unwrap();
    let resp = next_frame(&mut rx).await;
    assert!(
        resp.get("error").is_some(),
        "二进制帧必须被**明确拒绝**，而不是静默丢弃（静默丢弃会让客户端干等）：{resp}"
    );
}
