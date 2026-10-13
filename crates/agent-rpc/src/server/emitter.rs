use agent_proto::*;
use crate::state::{generate_snapshot_with_seq, AgentStore};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

/// 16ms 窗口节流合帧状态广播器（Snapshot Debouncer / Broadcaster）
/// 保证在大模型极高频输出 Token 时，UI 侧能获得丝滑的 60FPS 打字机动画，
/// 同时杜绝高频全量快照序列化造成的 CPU 飙升与网络拥塞。
///
/// 🔴 **大快照要自动降频**：合帧是"全量快照"，大小取决于会话里有多少卡片。
/// 实测一条长会话（822 个 item）单帧 **1.85 MB**——按 16ms 推就是 ~115 MB/s 的
/// 序列化 + IPC + 前端 JSON 解析，渲染进程内存直接冲到 GB 级（实测 1.5 GB）。
/// 所以这里按**上一帧的实际字节数**自适应：帧越大，间隔越长（帧数与 CPU 随之下降），
/// 而轮次结束 / 审批等关键事件仍走 `broadcast_immediate`，界面不会停在旧状态。
pub struct StateBroadcaster {
    store: Arc<RwLock<AgentStore>>,
    seq: Arc<AtomicU64>,
    broadcast_tx: mpsc::UnboundedSender<String>,
    dirty: Arc<AtomicBool>,
    /// 上一帧的字节数（自适应间隔的依据）
    last_frame_bytes: Arc<std::sync::atomic::AtomicUsize>,
    /// 客户端是否声明了 state.patch 能力（见 enable_patch_mode）。
    ///
    /// **默认关闭**：没声明的连接继续收全量快照——这是『宿主先落地、客户端后跟上』
    /// 的安全绳，也是出问题时的回滚路径。
    patch_enabled: Arc<AtomicBool>,
}

/// 合帧的基础间隔（60FPS 打字机）
pub const FRAME_INTERVAL_SMALL: Duration = Duration::from_millis(16);
/// 帧超过 [`FRAME_BIG_BYTES`] 时的间隔：帧越大越贵，宁可 10FPS 也不要卡死/吃满内存
pub const FRAME_INTERVAL_BIG: Duration = Duration::from_millis(100);
/// "大帧"的门槛
pub const FRAME_BIG_BYTES: usize = 256 * 1024;

/// 按上一帧大小挑下一帧的间隔（纯函数，便于守门测试）。
pub fn next_frame_interval(last_frame_bytes: usize) -> Duration {
    if last_frame_bytes >= FRAME_BIG_BYTES {
        FRAME_INTERVAL_BIG
    } else {
        FRAME_INTERVAL_SMALL
    }
}

impl StateBroadcaster {
    /// 创建并启动合帧广播后台协程
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
            last_frame_bytes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            patch_enabled: Arc::new(AtomicBool::new(false)),
        });

        let b = broadcaster.clone();
        tokio::spawn(async move {
            loop {
                // 自适应：小帧 16ms（60FPS 打字机），大帧 100ms（别把内存打爆）
                let interval = next_frame_interval(
                    b.last_frame_bytes.load(Ordering::Relaxed),
                );
                tokio::time::sleep(interval).await;
                if b.dirty.swap(false, Ordering::Relaxed) {
                    b.broadcast_immediate().await;
                }
            }
        });

        broadcaster
    }

    /// 客户端在 session.initialize 里声明了 state.patch → 后续帧可走增量。
    pub fn enable_patch_mode(&self) {
        self.patch_enabled.store(true, Ordering::Relaxed);
    }

    /// 当前这条连接是否走增量帧（全量快照永远保留给首帧/自愈/未声明能力的连接）。
    pub fn patch_enabled(&self) -> bool {
        self.patch_enabled.load(Ordering::Relaxed)
    }

    /// 标记状态有更新，等待合帧窗口统一推流
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
            // 记下这一帧多大：下一帧的间隔据此自适应（见 `next_frame_interval`）
            self.last_frame_bytes.store(json_str.len(), Ordering::Relaxed);
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

        // 等待 60ms，让合帧窗口触发广播
        tokio::time::sleep(Duration::from_millis(60)).await;

        // 应该成功收到合帧广播，且不会产生 10 条连续垃圾帧
        let mut count = 0;
        while let Ok(_) = rx.try_recv() {
            count += 1;
        }
        // 60ms 内，以 16ms 为周期，最多触发 1~4 次合帧广播，绝非 10 次
        assert!(count >= 1 && count <= 4, "期望收到 1~4 次合帧广播，实际收到 {} 次", count);
    }

    /// 🔴 大帧必须自动降频：全量快照的大小由会话卡片数决定，
    /// 实测一条长会话单帧 1.85 MB——按 60FPS 推就是 ~115 MB/s 的序列化 +
    /// IPC + 前端解析，渲染进程内存会被打到 GB 级（实测 1.5 GB）。
    #[tokio::test]
    async fn test_big_frame_slows_the_cadence() {
        assert_eq!(
            next_frame_interval(10 * 1024),
            FRAME_INTERVAL_SMALL,
            "小帧保持 60FPS 打字机"
        );
        assert_eq!(
            next_frame_interval(FRAME_BIG_BYTES),
            FRAME_INTERVAL_BIG,
            "刚好到门槛就该降频"
        );
        assert_eq!(
            next_frame_interval(1_895_000),
            FRAME_INTERVAL_BIG,
            "1.85 MB 的实测大帧必须走慢档"
        );

        // 真的广播一次大帧后，广播器必须记住它的大小（下一帧据此变慢）
        let store = Arc::new(RwLock::new(AgentStore::new("E:/test_big_frame".to_string())));
        let seq = Arc::new(AtomicU64::new(0));
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        let broadcaster = StateBroadcaster::new(store.clone(), seq, tx);
        {
            // 造一条足够大的 item，让快照超过门槛
            let mut s = store.write().await;
            let tid = s.create_thread(Some("大帧".to_string()), None);
            s.start_tool_call(&tid, "call_big", "run_command", "{}");
            s.finish_tool_call(
                &tid,
                "call_big",
                true,
                Some("x".repeat(FRAME_BIG_BYTES + 1024)),
                Some(5),
                Some(0),
                Some(5),
                None,
            );
        }
        broadcaster.broadcast_immediate().await;
        let last = broadcaster.last_frame_bytes.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            last >= FRAME_BIG_BYTES,
            "广播器必须记下真实帧大小（实际 {last}）"
        );
        assert_eq!(
            next_frame_interval(last),
            FRAME_INTERVAL_BIG,
            "大帧之后必须切到慢档"
        );
    }
}
