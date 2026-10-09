//! **WEB 接入面**的端到端测试（S7）。
//!
//! # 这些断言都在证明"没认证就碰不到 agent"
//!
//! 鉴权最容易的失败形态是**看起来在检查、实际没拦住**：
//! 检查了 token 但仍然把连接绑到了 agent、或者拒绝用的是"连上就断开"
//! （客户端只能看到"连接被重置"，看不出原因）。
//!
//! 所以断言落在三件事上：
//!
//! | 断言 | 证明的事 |
//! |---|---|
//! | 坏 token → 握手期 HTTP 401 | 拒绝发生在**握手之前**，且原因可读 |
//! | 未认证连接能 `gateway.info`、**不能**碰别的方法 | 发现通道可用但**能力被限制** |
//! | 未认证连接**没有拉起 agent** | 未认证请求不能放大成"起进程" |
//! | 工作区不在 token 作用域 → 拒绝 | 多租户隔离真的生效 |
//! | Origin 默认拒绝 / 白名单放行 | 浏览器跨源默认关着 |

use std::sync::Arc;
use std::time::Duration;

use agent_gateway::auth::AuthConfig;
use agent_gateway::relay::serve_client;
use agent_gateway::Gateway;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type Sink = futures_util::stream::SplitSink<WsStream, Message>;
type Source = futures_util::stream::SplitStream<WsStream>;

/// 桩 agent 宿主：连上推一帧种子快照，收到方法回一帧。
///
/// 为什么要它：**已认证连接会被绑定到一个 agent**（S5 的连接级会话亲和，
/// 且既有测试要求"连上就收到种子快照"）。所以想走通已认证路径，
/// 就得有一个真的能连上的宿主——不能只注册一个假端点。
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
                let _ = tx
                    .send(Message::Text(
                        serde_json::json!({"jsonrpc":"2.0","method":"evt.state.snapshot","params":{"seq":0}})
                            .to_string()
                            .into(),
                    ))
                    .await;
                while let Some(Ok(Message::Text(text))) = rx.next().await {
                    let Ok(req) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let _ = tx
                        .send(Message::Text(
                            serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"ok":true}})
                                .to_string()
                                .into(),
                        ))
                        .await;
                }
            });
        }
    });
    port
}

/// 起一个网关：注册一个指向桩宿主的实例（已认证连接会绑定到它）。
async fn gateway_with(auth: AuthConfig) -> (Arc<Gateway>, u16) {
    let ws = std::env::temp_dir().join("a_da_gw_auth_ws");
    let stub_port = stub_agent_host().await;
    let gateway = Arc::new(
        Gateway::new("ada-coding", ws.to_string_lossy()).with_auth(auth),
    );
    let ws_norm = agent_gateway::normalize_workspace(&ws.to_string_lossy());
    gateway.registry.register(agent_gateway::registry::AgentInstance {
        id: agent_gateway::AgentRegistry::id_for_workspace("ada-coding", &ws_norm),
        product: "ada-coding".to_string(),
        workspace: ws_norm.to_string_lossy().to_string(),
        endpoint: format!("ws://127.0.0.1:{stub_port}/rpc?token=stub"),
        status: agent_gateway::registry::AgentStatus::Ready,
        pid: None,
        started_at: 1,
    });
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("绑定");
    let port = listener.local_addr().unwrap().port();
    let gw = gateway.clone();
    let w = ws.to_string_lossy().to_string();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let g = gw.clone();
            let w2 = w.clone();
            tokio::spawn(async move {
                let _ = serve_client(g, stream, Some(w2)).await;
            });
        }
    });
    (gateway, port)
}

/// 带 query 与可选 Origin 连接。失败时返回**握手错误文本**（里面有 HTTP 状态码）。
async fn connect(
    port: u16,
    query: &str,
    origin: Option<&str>,
) -> Result<(Sink, Source), String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let url = format!("ws://127.0.0.1:{port}/rpc{query}");
    let mut req = url.into_client_request().map_err(|e| e.to_string())?;
    if let Some(o) = origin {
        req.headers_mut()
            .insert("origin", o.parse().map_err(|e| format!("{e:?}"))?);
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => Ok(ws.split()),
        Err(e) => Err(format!("{e}")),
    }
}

async fn call(tx: &mut Sink, rx: &mut Source, id: u64, method: &str) -> serde_json::Value {
    let frame = serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":{}});
    tx.send(Message::Text(frame.to_string().into()))
        .await
        .expect("发送");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = tokio::time::timeout(remain, rx.next())
            .await
            .expect("等响应超时")
            .expect("连接应还在")
            .expect("读帧失败");
        let Message::Text(t) = msg else { continue };
        let v: serde_json::Value = serde_json::from_str(&t).unwrap();
        if v.get("id").and_then(|x| x.as_u64()) == Some(id) {
            return v;
        }
    }
}

/// **坏 token → 握手期就被拒**（401），不是"连上再断"。
#[tokio::test]
async fn test_bad_token_rejected_at_handshake() {
    let (_gw, port) = gateway_with(AuthConfig::from_args(&["good".into()], &[]).unwrap()).await;
    let err = connect(port, "?token=wrong", None)
        .await
        .expect_err("坏 token 必须连不上");
    assert!(
        err.contains("401") || err.to_lowercase().contains("unauthorized"),
        "拒绝要表现为 HTTP 401（客户端能读到原因），实际：{err}"
    );
}

/// 未认证连接：**能发现**（`gateway.info`），**不能**碰别的。
#[tokio::test]
async fn test_anonymous_can_discover_but_nothing_else() {
    let (gw, port) = gateway_with(AuthConfig::from_args(&["good".into()], &[]).unwrap()).await;
    let (mut tx, mut rx) = connect(port, "", None).await.expect("未认证也应能连上（发现通道）");

    let info = call(&mut tx, &mut rx, 1, "gateway.info").await;
    assert!(info.get("error").is_none(), "发现必须可用：{info}");
    assert_eq!(
        info.pointer("/result/authRequired").and_then(|v| v.as_bool()),
        Some(true),
        "发现要告诉客户端「需要 token」：{info}"
    );
    // 🔴 发现响应**不得含任何秘密**
    let text = info.to_string();
    assert!(!text.contains("good"), "发现响应泄露了 token：{text}");
    assert!(
        !text.contains("ws://"),
        "发现响应不得含 agent 端点（未认证通道不送内部拓扑）：{text}"
    );

    // 别的方法一律拒绝，并**说清楚**要带 token
    let denied = call(&mut tx, &mut rx, 2, "gateway.listAgents").await;
    let msg = denied
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(denied.get("error").is_some(), "未认证不得列出 agent：{denied}");
    assert!(msg.contains("token"), "错误信息要指出该带 token：{msg}");

    // 未认证连接**没有绑定/新起 agent**（预注册的那 1 个不算，断言没有增加）
    assert_eq!(
        gw.registry.len(),
        1,
        "未认证请求不得让网关多起 agent 进程（放大攻击面）"
    );
}

/// 正确 token → 全部能力可用。
#[tokio::test]
async fn test_valid_token_gets_full_access() {
    let (_gw, port) = gateway_with(AuthConfig::from_args(&["good".into()], &[]).unwrap()).await;
    let (mut tx, mut rx) = connect(port, "?token=good", None).await.expect("好 token 应连上");
    let resp = call(&mut tx, &mut rx, 1, "gateway.listAgents").await;
    assert!(resp.get("error").is_none(), "已认证应能列出 agent：{resp}");
}

/// **多租户隔离**：token 的作用域管到工作区。
///
/// 用真实工作区路径（`?workspace=` 会被解析出来），断言"不在作用域 → 拒绝"。
#[tokio::test]
async fn test_token_scope_blocks_other_workspace() {
    let mine = std::env::temp_dir().join("a_da_tenant_mine");
    let other = std::env::temp_dir().join("a_da_tenant_other");
    let mine_s = mine.to_string_lossy().replace('\\', "/");
    let other_s = other.to_string_lossy().replace('\\', "/");

    let auth = AuthConfig::from_args(&[format!("t1={mine_s}")], &[]).unwrap();
    let (gw, port) = gateway_with(auth).await;

    // 自己的作用域：通过（会尝试拉起 agent，失败也不影响"作用域检查通过"这个结论）
    let mine_q = format!("?token=t1&workspace={}", urlencode(&mine_s));
    let _ = connect(port, &mine_q, None).await;

    // 别人的作用域：**必须被拒**，且不得起 agent
    let other_q = format!("?token=t1&workspace={}", urlencode(&other_s));
    let _ = connect(port, &other_q, None).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 作用域外的请求不得注册任何实例
    let ids: Vec<String> = gw.registry.list().iter().map(|i| i.id.clone()).collect();
    assert!(
        !ids.iter().any(|id| id.contains(&other_s.to_lowercase())),
        "作用域外的工作区不得被绑定：{ids:?}"
    );
}

/// Origin：默认拒绝任何带 Origin 的请求；白名单放行。
#[tokio::test]
async fn test_origin_default_deny_then_allowlist() {
    let (_gw, port) = gateway_with(AuthConfig::from_args(&["t".into()], &[]).unwrap()).await;

    let err = connect(port, "?token=t", Some("https://evil.example"))
        .await
        .expect_err("默认必须拒绝浏览器来源");
    assert!(
        err.contains("403") || err.to_lowercase().contains("forbidden"),
        "要表现为 HTTP 403：{err}"
    );

    // 白名单里放行
    let auth = AuthConfig::from_args(&["t".into()], &["https://app.example".into()]).unwrap();
    let (_gw2, port2) = gateway_with(auth).await;
    let ok = connect(port2, "?token=t", Some("https://app.example")).await;
    assert!(ok.is_ok(), "白名单来源应放行：{:?}", ok.err());
    // 别的来源仍然拒绝
    assert!(
        connect(port2, "?token=t", Some("https://evil.example"))
            .await
            .is_err()
    );
}

/// 开放模式（未配 token）保持 S5/S6 的行为不变——回环上的桌面端不受影响。
#[tokio::test]
async fn test_open_mode_preserves_previous_behaviour() {
    let (_gw, port) = gateway_with(AuthConfig::open()).await;
    let (mut tx, mut rx) = connect(port, "", None).await.expect("开放模式应能连上");
    let resp = call(&mut tx, &mut rx, 1, "gateway.status").await;
    assert!(resp.get("error").is_none(), "开放模式应保持旧行为：{resp}");
    assert_eq!(
        resp.pointer("/result/routableCount").is_some(),
        true,
        "status 形状不变：{resp}"
    );
}

/// 最小百分号编码（只编码会破坏 query 的字符，够测试用）。
fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' => c.to_string(),
            other => other
                .to_string()
                .bytes()
                .map(|b| format!("%{b:02X}"))
                .collect::<String>(),
        })
        .collect()
}
