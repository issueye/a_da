use crate::protocol::*;
use crate::state::{generate_snapshot_with_seq, AgentStore};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

/// 16ms 窗口节流合帧状态广播器（Snapshot Debouncer / Broadcaster）
/// 保证在大模型极高频输出 Token 时，UI 侧能获得丝滑的 60FPS 打字机动画，
/// 同时杜绝高频全量快照序列化造成的 CPU 飙升与网络拥塞。
pub struct StateBroadcaster {
    store: Arc<RwLock<AgentStore>>,
    seq: Arc<AtomicU64>,
    broadcast_tx: mpsc::UnboundedSender<String>,
    dirty: Arc<AtomicBool>,
}

impl StateBroadcaster {
    /// 创建并启动 16ms 合帧广播后台协程
    pub fn new(
        store: Arc<RwLock<AgentStore>>,
        seq: Arc<AtomicU64>,
        broadcast_tx: mpsc::UnboundedSender<String>,
    ) -> Arc<Self> {
        let broadcaster = Arc::new(Self {
            store,
            seq,
            broadcast_tx,
            dirty: Arc::new(AtomicBool::new(false)),
        });

        let b = broadcaster.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(16));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;
                if b.dirty.swap(false, Ordering::Relaxed) {
                    b.broadcast_immediate().await;
                }
            }
        });

        broadcaster
    }

    /// 标记状态有更新，等待 16ms 窗口统一合帧推流
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// 立即强制广播一次当前最新快照（跳过节流窗口，用于重要事件或轮次结束）
    pub async fn broadcast_immediate(&self) {
        self.dirty.store(false, Ordering::Relaxed);
        let current_seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let store = self.store.read().await;
        let snapshot = generate_snapshot_with_seq(&store, current_seq);
        let event = SnapshotEvent {
            seq: current_seq,
            topic: EVT_STATE_SNAPSHOT.to_string(),
            payload: snapshot,
        };
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "method": EVT_STATE_SNAPSHOT,
            "params": event
        });
        if let Ok(json_str) = serde_json::to_string(&frame) {
            let _ = self.broadcast_tx.send(json_str);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_state_broadcaster_immediate() {
        let store = Arc::new(RwLock::new(AgentStore::new("E:/test_project".to_string())));
        let seq = Arc::new(AtomicU64::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        let broadcaster = StateBroadcaster::new(store.clone(), seq.clone(), tx);
        broadcaster.broadcast_immediate().await;

        let msg = rx.recv().await.expect("未能收到广播消息");
        let val: serde_json::Value = serde_json::from_str(&msg).expect("广播消息不是合法的 JSON");
        assert_eq!(val["method"], EVT_STATE_SNAPSHOT);
        assert_eq!(val["params"]["seq"], 1);
        assert_eq!(val["params"]["topic"], EVT_STATE_SNAPSHOT);
        assert_eq!(val["params"]["payload"]["workspace"]["project"], "E:/test_project");
    }

    #[tokio::test]
    async fn test_state_broadcaster_throttle_and_debounce() {
        let store = Arc::new(RwLock::new(AgentStore::new("E:/test_throttle".to_string())));
        let seq = Arc::new(AtomicU64::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        let broadcaster = StateBroadcaster::new(store.clone(), seq.clone(), tx);

        // 模拟极高频输出（例如 10 次 mark_dirty）
        for _ in 0..10 {
            broadcaster.mark_dirty();
        }

        // 等待 40ms，让 16ms 窗口触发合帧广播
        tokio::time::sleep(Duration::from_millis(45)).await;

        // 应该成功收到合帧广播，且不会产生 10 条连续垃圾帧
        let mut count = 0;
        while let Ok(_) = rx.try_recv() {
            count += 1;
        }
        // 45ms 内，以 16ms 为周期，最多触发 1~3 次合帧广播，绝非 10 次
        assert!(count >= 1 && count <= 3, "期望收到 1~3 次合帧广播，实际收到 {} 次", count);
    }
}
