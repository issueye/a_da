//! Phase 0 的守门：HTTP 代理连接（`?api=1`）**不参与事件广播**，
//! 但写操作仍要让普通 UI 连接收到快照。
//!
//! 这条测试跑真的 `WsHostServer`：
//! - UI 连接：应收到首帧种子快照；
//! - API 连接：不应收到种子快照；只读请求不应触发广播；
//! - API 连接发 `config.setEffort`：dispatch 臂自己广播，UI 必须收到。

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;

use agent_base::testing::TempAppHome;
use agent_proto::{CONFIG_GET, CONFIG_SET_EFFORT, EVT_STATE_SNAPSHOT, SESSION_INITIALIZE};
use agent_rpc::server::WsHostServer;
use agent_rpc::state::AgentStore;

async fn send_json<S>(tx: &mut S, value: Value)
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Debug,
{
    tx.send(Message::Text(value.to_string().into()))
        .await
        .expect("发送帧");
}

async fn recv_json<S>(rx: &mut S, wait: Duration) -> Option<Value>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let msg = tokio::time::timeout(wait, rx.next()).await.ok()??.ok()?;
    let Message::Text(text) = msg else {
        return None;
    };
    serde_json::from_str(text.as_str()).ok()
}

#[tokio::test]
async fn test_api_connection_is_silent_but_api_writes_still_notify_ui() {
    let dir = std::env::temp_dir().join(format!("a_da_ws_api_mode_{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(&dir);
    let home = TempAppHome::at(&dir).with_config_file(dir.join("config.json"));
    let store = Arc::new(RwLock::new(AgentStore::with_home(
        "E:/test".to_string(),
        &home,
    )));

    let server = WsHostServer::bind(0, "tok".to_string(), store.clone())
        .await
        .expect("启动 WS 宿主");
    let port = server.port;

    // UI 连接：正常收种子快照
    let (ui_ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/rpc?token=tok"))
            .await
            .expect("UI 连接");
    let (mut ui_tx, mut ui_rx) = ui_ws.split();
    let seed = recv_json(&mut ui_rx, Duration::from_secs(2))
        .await
        .expect("UI 应收到种子快照");
    assert_eq!(seed["method"], EVT_STATE_SNAPSHOT);

    // API 连接：不应收种子快照
    let (api_ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/rpc?token=tok&api=1"))
            .await
            .expect("API 连接");
    let (mut api_tx, mut api_rx) = api_ws.split();
    assert!(
        recv_json(&mut api_rx, Duration::from_millis(300))
            .await
            .is_none(),
        "api=1 连接不应收到种子快照"
    );

    // 握手
    send_json(
        &mut api_tx,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": SESSION_INITIALIZE,
            "params": {
                "protocolVersion": "1.0",
                "client": { "name": "http-api-test", "capabilities": [] }
            }
        }),
    )
    .await;
    let init = recv_json(&mut api_rx, Duration::from_secs(2))
        .await
        .expect("initialize 响应");
    assert_eq!(init["id"], 1);

    // 只读：API 拿到响应，UI 不应被叫醒
    send_json(
        &mut api_tx,
        json!({ "jsonrpc": "2.0", "id": 2, "method": CONFIG_GET, "params": {} }),
    )
    .await;
    let read = recv_json(&mut api_rx, Duration::from_secs(2))
        .await
        .expect("config.get 响应");
    assert_eq!(read["id"], 2);
    assert!(
        recv_json(&mut ui_rx, Duration::from_millis(300))
            .await
            .is_none(),
        "只读请求不应触发 UI 快照广播"
    );

    // 写：dispatch 臂自己广播，UI 必须收到
    send_json(
        &mut api_tx,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": CONFIG_SET_EFFORT,
            "params": { "effort": "medium" }
        }),
    )
    .await;
    let write = recv_json(&mut api_rx, Duration::from_secs(2))
        .await
        .expect("config.setEffort 响应");
    assert_eq!(write["id"], 3);

    let notified = recv_json(&mut ui_rx, Duration::from_secs(2))
        .await
        .expect("写操作后 UI 应收到快照");
    assert_eq!(notified["method"], EVT_STATE_SNAPSHOT);
    assert_eq!(
        store.read().await.config.effort,
        agent_proto::Effort::Medium,
        "写操作必须真的生效"
    );

    let _ = ui_tx.close().await;
    let _ = api_tx.close().await;
    let _ = std::fs::remove_dir_all(&dir);
}
