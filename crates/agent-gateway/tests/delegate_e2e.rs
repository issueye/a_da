//! **交互平台**的端到端测试（桩 agent 宿主，密封、不依赖外部进程）。
//!
//! # 这个测试证明的是"跨进程真的发生了"，不是"我们发了帧"
//!
//! 派活与取消都要跨两层（客户端 → 网关 → 目标 agent），最容易出的问题是
//! **假接线**：取消"发了"但没到目标、派活"回了"但目标其实没跑。
//! 所以断言落在**目标侧可观察的事实**上：
//!
//! | 断言 | 证明的事 |
//! |---|---|
//! | 派活返回桩宿主的 assistant 文本 | 网关**真的驱动目标跑了一轮**并读回了结果 |
//! | 桩宿主收到了 `thread.send` | 派活不是网关自己编的答案 |
//! | 取消后桩宿主收到了 `thread.abort` | **取消跨网关真的穿透到目标** |
//! | 深度 0 / 超限被拒 | 拿不到依据时**倒向拒绝**，不是默认放行 |

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_gateway::registry::{AgentInstance, AgentStatus};
use agent_gateway::relay::serve_client;
use agent_gateway::Gateway;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// 桩宿主的可观察记录。
#[derive(Default)]
struct StubLog {
    /// 收到过 `thread.send` 的文本
    sends: Mutex<Vec<String>>,
    /// 收到过 `thread.abort` 的线程 id
    aborts: Mutex<Vec<String>>,
    /// 被要求"这一轮跑多久"（模拟真实 agent 的耗时）
    turn_delay_ms: u64,
    /// 是否已经跑完一轮（决定快照里线程是否还在 running）
    finished: AtomicBool,
}

/// 桩 agent 宿主：够真地模拟"建线程 → 发消息 → 广播快照（running → done）"。
async fn stub_agent_host(log: Arc<StubLog>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("桩宿主绑定");
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let (mut tx, mut rx) = ws.split();
                let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

                // 单写者
                let writer = tokio::spawn(async move {
                    while let Some(s) = out_rx.recv().await {
                        if tx.send(Message::Text(s.into())).await.is_err() {
                            break;
                        }
                    }
                });

                // 连接期：种子快照（空线程）
                let _ = out_tx.send(snapshot(&[], &[], None));

                let mut thread_id: Option<String> = None;
                while let Some(Ok(msg)) = rx.next().await {
                    let Message::Text(text) = msg else { continue };
                    let Ok(req) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
                    match method {
                        "session.initialize" => {
                            let _ = out_tx.send(
                                serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"sessionId":"s"}})
                                    .to_string(),
                            );
                        }
                        "thread.create" => {
                            let tid = "t-stub".to_string();
                            thread_id = Some(tid.clone());
                            let _ = out_tx.send(
                                serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"id":tid}})
                                    .to_string(),
                            );
                        }
                        "thread.send" => {
                            let t = req
                                .pointer("/params/text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            log.sends.lock().unwrap().push(t);
                            let _ = out_tx.send(
                                serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"ok":true}})
                                    .to_string(),
                            );
                            // 开跑：推一帧 running
                            let tid = thread_id.clone().unwrap_or_else(|| "t-stub".into());
                            let _ = out_tx.send(snapshot(&[tid.clone()], &[tid.clone()], None));
                            // 延迟后完成：推一帧 done + assistant 文本
                            let delay = log.turn_delay_ms;
                            let log2 = log.clone();
                            let tx2 = out_tx.clone();
                            let tid2 = tid.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(delay)).await;
                                log2.finished.store(true, Ordering::SeqCst);
                                let _ = tx2.send(snapshot(
                                    &[],
                                    &[tid2.clone()],
                                    Some("桩宿主完成的任务结果"),
                                ));
                            });
                        }
                        "thread.abort" => {
                            let tid = req
                                .pointer("/params/threadId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            log.aborts.lock().unwrap().push(tid.clone());
                            let _ = out_tx.send(
                                serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"ok":true}})
                                    .to_string(),
                            );
                            // 中止后线程不再 running
                            let _ = out_tx.send(snapshot(&[], &[tid], None));
                        }
                        _ => {
                            let _ = out_tx.send(
                                serde_json::json!({"jsonrpc":"2.0","id":id,"result":{}})
                                    .to_string(),
                            );
                        }
                    }
                }
                drop(out_tx);
                let _ = writer.await;
            });
        }
    });

    port
}

/// 造一帧 `evt.state.snapshot`。
fn snapshot(running: &[String], threads: &[String], assistant: Option<&str>) -> String {
    let items: Vec<serde_json::Value> = match assistant {
        Some(t) => vec![serde_json::json!({"kind":"assistant","id":"a1","at":1,"text":t})],
        None => vec![],
    };
    let threads_json: Vec<serde_json::Value> = threads
        .iter()
        .map(|id| serde_json::json!({"id": id, "title": "t", "createdAt": 1, "workspace": "E:/ws", "items": items}))
        .collect();
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "evt.state.snapshot",
        "params": {
            "seq": 1,
            "threads": threads_json,
            "activeThreadId": "",
            "runningThreadIds": running,
            "waitingThreadIds": [],
        }
    })
    .to_string()
}

async fn gateway_with_stub(log: Arc<StubLog>) -> (Arc<Gateway>, u16) {
    let stub_port = stub_agent_host(log).await;
    let ws = std::env::temp_dir().join("a_da_gw_delegate_ws");
    let gateway = Arc::new(Gateway::new("ada-coding", ws.to_string_lossy()));

    let ws_norm = agent_gateway::normalize_workspace(&ws.to_string_lossy());
    let id = agent_gateway::AgentRegistry::id_for_workspace("ada-coding", &ws_norm);
    gateway.registry.register(AgentInstance {
        id,
        product: "ada-coding".to_string(),
        workspace: ws_norm.to_string_lossy().to_string(),
        endpoint: format!("ws://127.0.0.1:{stub_port}/rpc?token=stub"),
        status: AgentStatus::Ready,
        pid: None,
        started_at: 1,
    });

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("网关绑定");
    let gw_port = listener.local_addr().unwrap().port();
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
    (gateway, gw_port)
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type Sink = futures_util::stream::SplitSink<WsStream, Message>;
type Source = futures_util::stream::SplitStream<WsStream>;
type Client = (Sink, Source);

async fn connect(port: u16) -> Client {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/rpc"))
        .await
        .expect("连上网关");
    ws.split()
}

async fn send(tx: &mut Sink, v: serde_json::Value) {
    tx.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// 读到 `id` 匹配的那一帧（跳过事件帧）。
async fn recv_id(rx: &mut Source, id: u64) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
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

/// **派活**：网关驱动目标跑一轮，把 assistant 文本带回来。
#[tokio::test]
async fn test_delegate_drives_remote_turn_and_returns_summary() {
    let log = Arc::new(StubLog { turn_delay_ms: 50, ..Default::default() });
    let (_gw, port) = gateway_with_stub(log.clone()).await;
    let (mut tx, mut rx) = connect(port).await;

    send(
        &mut tx,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 7, "method": "gateway.delegate",
            "params": { "task": "把 README 里的错别字改掉", "depth": 1 }
        }),
    )
    .await;

    let resp = recv_id(&mut rx, 7).await;
    assert!(
        resp.get("error").is_none(),
        "派活不该报错：{resp}"
    );
    assert_eq!(
        resp.pointer("/result/summary").and_then(|v| v.as_str()),
        Some("桩宿主完成的任务结果"),
        "必须带回**目标**的回复，而不是网关自己编的：{resp}"
    );
    let agent_id = resp
        .pointer("/result/details/agentId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        agent_id.starts_with("ada-coding::"),
        "details 里要有 agentId（可归因）：{agent_id}"
    );

    // 目标侧可观察事实：它真的收到了任务文本
    let sends = log.sends.lock().unwrap().clone();
    assert_eq!(sends, vec!["把 README 里的错别字改掉".to_string()], "任务必须原样到达目标");
}

/// **取消跨网关**：取消必须穿透到目标，而不是"我们发了一帧取消"。
#[tokio::test]
async fn test_cancel_delegation_reaches_the_target_agent() {
    // 让目标那一轮跑很久，好在中途取消
    let log = Arc::new(StubLog { turn_delay_ms: 30_000, ..Default::default() });
    let (_gw, port) = gateway_with_stub(log.clone()).await;
    let (mut tx, mut rx) = connect(port).await;

    send(
        &mut tx,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 11, "method": "gateway.delegate",
            "params": { "task": "跑一个很长的任务", "depth": 1, "delegationId": "d-cancel" }
        }),
    )
    .await;

    // 等目标真的收到任务（否则取消可能先于派活注册）
    for _ in 0..100 {
        if !log.sends.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!log.sends.lock().unwrap().is_empty(), "派活应已到达目标");

    // 取消
    send(
        &mut tx,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 12, "method": "gateway.cancelDelegation",
            "params": { "delegationId": "d-cancel" }
        }),
    )
    .await;
    let cancel_resp = recv_id(&mut rx, 12).await;
    assert_eq!(
        cancel_resp.pointer("/result/cancelled").and_then(|v| v.as_bool()),
        Some(true),
        "取消应被受理：{cancel_resp}"
    );

    // 派活那一帧应回"被取消"
    let delegate_resp = recv_id(&mut rx, 11).await;
    assert!(
        delegate_resp.get("error").is_some(),
        "被取消的派活必须如实报错：{delegate_resp}"
    );
    assert!(
        delegate_resp
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("取消"),
        "错误信息要点明是取消：{delegate_resp}"
    );

    // 🔴 关键断言：目标**真的**收到了 thread.abort
    // （有界轮询：网关已等过受理，但"目标侧记录"与"响应到达客户端"之间仍有调度间隙）
    for _ in 0..100 {
        if !log.aborts.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let aborts = log.aborts.lock().unwrap().clone();
    assert_eq!(
        aborts,
        vec!["t-stub".to_string()],
        "取消必须穿透到目标（thread.abort），而不是停在网关"
    );
}

/// 深度缺省即拒绝：拿不到依据时不放开（`FailDirection::Closed` 同原则）。
#[tokio::test]
async fn test_delegate_without_depth_is_rejected() {
    let log = Arc::new(StubLog { turn_delay_ms: 10, ..Default::default() });
    let (_gw, port) = gateway_with_stub(log.clone()).await;
    let (mut tx, mut rx) = connect(port).await;

    send(
        &mut tx,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 21, "method": "gateway.delegate",
            "params": { "task": "x" }   // 没有 depth
        }),
    )
    .await;

    let resp = recv_id(&mut rx, 21).await;
    assert!(resp.get("error").is_some(), "缺 depth 必须被拒绝：{resp}");
    assert!(
        resp.pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("深度"),
        "错误信息要说明是深度问题：{resp}"
    );
    assert!(
        log.sends.lock().unwrap().is_empty(),
        "被拒绝的派活**绝不能**已经打到目标"
    );
}

/// 取消一个不存在的派活 → 如实回 `cancelled:false`，不静默、不报错。
#[tokio::test]
async fn test_cancel_unknown_delegation_reports_false() {
    let log = Arc::new(StubLog { turn_delay_ms: 10, ..Default::default() });
    let (_gw, port) = gateway_with_stub(log).await;
    let (mut tx, mut rx) = connect(port).await;

    send(
        &mut tx,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 31, "method": "gateway.cancelDelegation",
            "params": { "delegationId": "does-not-exist" }
        }),
    )
    .await;

    let resp = recv_id(&mut rx, 31).await;
    assert_eq!(
        resp.pointer("/result/cancelled").and_then(|v| v.as_bool()),
        Some(false),
        "不存在的派活要如实说 false：{resp}"
    );
}
