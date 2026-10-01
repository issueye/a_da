//! 原生 WebSocket 桥接器
//!
//! 为 Meta Hermes 轻量引擎提供原生非阻塞 WebSocket 客户端能力，
//! 负责在 JS 虚拟 DOM 界面与 Rust 后端之间建立高并发低延迟的双向全双工通道。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;
use tracing::{error, info};

pub struct WsConnection {
    pub state: Arc<AtomicU8>, // 0: Connecting, 1: Open, 2: Closing, 3: Closed
    pub send_tx: mpsc::UnboundedSender<String>,
    pub recv_queue: Arc<Mutex<VecDeque<String>>>,
}

static WS_TABLE: OnceLock<Mutex<HashMap<u32, WsConnection>>> = OnceLock::new();
static NEXT_WS_ID: AtomicU32 = AtomicU32::new(1);

fn get_table() -> &'static Mutex<HashMap<u32, WsConnection>> {
    WS_TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 建立到指定 URL 的原生 WebSocket 连接
pub fn ws_connect(url: &str) -> u32 {
    let id = NEXT_WS_ID.fetch_add(1, Ordering::SeqCst);
    let state = Arc::new(AtomicU8::new(0)); // Connecting
    let recv_queue = Arc::new(Mutex::new(VecDeque::new()));
    let (send_tx, mut send_rx) = mpsc::unbounded_channel::<String>();

    let conn = WsConnection {
        state: Arc::clone(&state),
        send_tx,
        recv_queue: Arc::clone(&recv_queue),
    };

    get_table().lock().unwrap().insert(id, conn);

    let url_str = url.to_string();
    let state_clone = Arc::clone(&state);
    let recv_clone = Arc::clone(&recv_queue);

    std::thread::Builder::new()
        .name(format!("ada-ws-client-{}", id))
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(r) => r,
                Err(e) => {
                    error!("[NativeWS] 创建 tokio runtime 失败: {}", e);
                    state_clone.store(3, Ordering::SeqCst);
                    return;
                }
            };

            rt.block_on(async move {
                use futures_util::{SinkExt, StreamExt};
                use tokio_tungstenite::connect_async;
                use tokio_tungstenite::tungstenite::Message;

                info!("[NativeWS] 正在连接 WebSocket: {}", url_str);
                match connect_async(&url_str).await {
                    Ok((ws_stream, _)) => {
                        info!("[NativeWS] WebSocket 连接成功: {}", url_str);
                        state_clone.store(1, Ordering::SeqCst); // Open
                        let (mut write, mut read) = ws_stream.split();

                        let send_fut = async {
                            while let Some(msg) = send_rx.recv().await {
                                if let Err(e) = write.send(Message::Text(msg.into())).await {
                                    error!("[NativeWS] 发送消息失败: {}", e);
                                    break;
                                }
                            }
                            let _ = write.close().await;
                        };

                        let recv_fut = async {
                            while let Some(res) = read.next().await {
                                match res {
                                    Ok(Message::Text(text)) => {
                                        if let Ok(mut q) = recv_clone.lock() {
                                            q.push_back(text.to_string());
                                        }
                                    }
                                    Ok(Message::Close(_)) => break,
                                    Err(e) => {
                                        error!("[NativeWS] 读取消息异常: {}", e);
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        };

                        tokio::select! {
                            _ = send_fut => {},
                            _ = recv_fut => {},
                        }
                    }
                    Err(e) => {
                        error!("[NativeWS] 连接失败 {}: {}", url_str, e);
                    }
                }

                state_clone.store(3, Ordering::SeqCst); // Closed
                info!("[NativeWS] WebSocket 连接已关闭: {}", url_str);
            });
        })
        .expect("启动 NativeWS 线程失败");

    id
}

/// 发送文本消息
pub fn ws_send(id: u32, text: &str) -> bool {
    if let Some(conn) = get_table().lock().unwrap().get(&id) {
        conn.send_tx.send(text.to_string()).is_ok()
    } else {
        false
    }
}

/// 轮询连接状态及接收消息队列
/// 返回 JSON 格式：`{"state": N, "messages": [...]}`
pub fn ws_poll(id: u32) -> String {
    if let Some(conn) = get_table().lock().unwrap().get(&id) {
        let state = conn.state.load(Ordering::SeqCst);
        let mut messages = Vec::new();
        if let Ok(mut q) = conn.recv_queue.lock() {
            while let Some(msg) = q.pop_front() {
                messages.push(msg);
            }
        }

        let obj = serde_json::json!({
            "state": state,
            "messages": messages,
        });
        obj.to_string()
    } else {
        r#"{"state":3,"messages":[]}"#.to_string()
    }
}

/// 主动关闭连接并从表中清理
pub fn ws_close(id: u32) {
    if let Some(conn) = get_table().lock().unwrap().remove(&id) {
        conn.state.store(2, Ordering::SeqCst); // Closing
    }
}
