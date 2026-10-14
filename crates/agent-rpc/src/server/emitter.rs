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
    /// 上次**发出去**的 seq（增量帧的 `baseSeq`，客户端据此判断有没有缺帧）。
    last_sent_seq: Arc<AtomicU64>,
    /// 已发给这条连接的 item 版本：`(threadId,itemId) → 指纹`。
    ///
    /// 增量帧的全部意义就在这张表：指纹没变就不推（见 `crate::state::patch::diff_items`）。
    versions: Arc<std::sync::Mutex<std::collections::BTreeMap<crate::state::patch::ItemKey, u64>>>,
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
            last_sent_seq: Arc::new(AtomicU64::new(0)),
            versions: Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
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

    /// 当前**已发出的状态版本**（最后一帧的 `seq`）。
    ///
    /// 每一份**全量快照**——首帧种子、`session.snapshot` 的响应——都必须带上它。
    /// 客户端把快照的 `seq` 记为 `lastSnapshotSeq`，而下一帧增量帧的 `baseSeq` 正是这个值；
    /// 写死成常量就会让两者的比对永远失败，客户端的"缺帧自愈"变成每帧一次的请求/响应死循环
    /// （实测 376 次/秒，渲染进程满核、内存持续上涨）。
    pub fn last_sent_seq(&self) -> u64 {
        self.last_sent_seq.load(Ordering::Relaxed)
    }

    /// 标记状态有更新，等待合帧窗口统一推流
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// 立即强制广播一次（跳过节流窗口，用于重要事件或轮次结束）。
    ///
    /// 两条路：
    /// - 客户端声明了 `state.patch` → 发**增量帧** `evt.state.patch`（只带变过的 item）；
    /// - 否则 → 发**全量快照** `evt.state.snapshot`（首帧、自愈、重连、老客户端都走这条）。
    pub async fn broadcast_immediate(&self) {
        self.dirty.store(false, Ordering::Relaxed);
        let current_seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let snapshot = {
            let store = self.store.read().await;
            generate_snapshot_with_seq(&store, current_seq)
        };
        let base_seq = self.last_sent_seq.swap(current_seq, Ordering::Relaxed);

        let payload = if self.patch_enabled() {
            // 本帧要投影的全部 item（只数带 items 的会话）
            let current: Vec<(String, agent_proto::Item)> = snapshot
                .threads
                .iter()
                .filter(|t| !t.items.is_empty())
                .flat_map(|t| t.items.iter().map(move |it| (t.id.clone(), it.clone())))
                .collect();

            let delta = {
                let mut versions = self.versions.lock().expect("版本表锁被毒化");
                let d = crate::state::patch::diff_items(&versions, &current);
                crate::state::patch::advance_versions(&mut versions, &current);
                d
            };

            // `meta` 与快照**同形**，只是各会话不带 items——客户端 applyPatch 正是这么消费的：
            // 先用 meta 覆盖线程元数据与小字段，再按 upserted/removed 合并 item。
            let mut meta = serde_json::to_value(&snapshot).unwrap_or(serde_json::Value::Null);
            if let Some(threads) = meta.get_mut("threads").and_then(|t| t.as_array_mut()) {
                for t in threads.iter_mut() {
                    if let Some(obj) = t.as_object_mut() {
                        obj.insert("items".to_string(), serde_json::json!([]));
                    }
                }
            }

            serde_json::json!({
                "jsonrpc": "2.0",
                "method": EVT_STATE_PATCH,
                "params": {
                    "seq": current_seq,
                    "baseSeq": base_seq,
                    "meta": meta,
                    "upserted": delta
                        .upserted
                        .iter()
                        .map(|(tid, it)| serde_json::json!({ "threadId": tid, "item": it }))
                        .collect::<Vec<_>>(),
                    "removed": delta
                        .removed
                        .iter()
                        .map(|(tid, iid)| serde_json::json!({ "threadId": tid, "itemId": iid }))
                        .collect::<Vec<_>>(),
                }
            })
        } else {
            let event = SnapshotEvent {
                seq: current_seq,
                topic: EVT_STATE_SNAPSHOT.to_string(),
                payload: snapshot,
            };
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": EVT_STATE_SNAPSHOT,
                "params": event
            })
        };

        if let Ok(json_str) = serde_json::to_string(&payload) {
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
            // 造足够多的卡片让快照超过门槛。
            //
            // ⚠️ **不能再靠"一条超大输出"**：投影侧对单个字段有限幅
            // （`state::snapshot::SNAPSHOT_FIELD_LIMIT`，32 KB），一条 257 KB 的输出
            // 进快照只剩 32 KB，整帧到不了 256 KB 的门槛，下面那条断言就再也无法成立。
            // 真实的大帧形态本来就是"卡片多"（长会话 800 条），所以这里按多张卡造。
            let mut s = store.write().await;
            let tid = s.create_thread(Some("大帧".to_string()), None);
            let big_output = "x".repeat(40 * 1024);
            for i in 0..20 {
                let call_id = format!("call_big_{i}");
                s.start_tool_call(&tid, &call_id, "run_command", "{}");
                s.finish_tool_call(
                    &tid,
                    &call_id,
                    true,
                    Some(big_output.clone()),
                    Some(5),
                    Some(0),
                    Some(5),
                    None,
                );
            }
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

    /// 🔴 增量帧的验收断言（docs/state-patch-plan.md）：**没变过的 item 不许出现在 `upserted`**。
    ///
    /// 这是整个增量方案省字节的地方：一条长会话 822 个 item、全量帧 1.85 MB，
    /// 而稳态下一帧只有"正在流式的那一个 item + 小 meta"——从 MB 级降到几十 KB。
    #[tokio::test]
    async fn test_patch_mode_sends_only_changed_items() {
        let store = Arc::new(RwLock::new(AgentStore::new("E:/test_patch_mode".to_string())));
        let seq = Arc::new(AtomicU64::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let broadcaster = StateBroadcaster::new(store.clone(), seq, tx);

        let mut tid = String::new();
        {
            let mut s = store.write().await;
            tid = s.create_thread(Some("增量".to_string()), None);
            s.start_tool_call(&tid, "call_x", "run_command", "{}");
            s.finish_tool_call(
                &tid,
                "call_x",
                true,
                Some("输出".to_string()),
                Some(1),
                Some(0),
                Some(1),
                None,
            );
        }
        // 断言只看**本用例这条线程**：`AgentStore::new` 会把用户已有的会话一起读进来
        // （本机实测有合成出来的 800 item 长会话），拿全局计数断言会随环境漂移。
        let mine = |v: &serde_json::Value| -> usize {
            v["params"]["upserted"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|u| u["threadId"].as_str() == Some(tid.as_str()))
                        .count()
                })
                .unwrap_or(0)
        };

        // 1. 没声明能力 → 全量快照（老客户端/首帧路径不受影响）
        broadcaster.broadcast_immediate().await;
        let full = rx.recv().await.expect("应收到一帧");
        assert!(full.contains("evt.state.snapshot"), "未开增量应发全量快照");

        // 2. 声明能力后第一帧：版本表是空的，等价于把现有 item 全推一次
        broadcaster.enable_patch_mode();
        broadcaster.broadcast_immediate().await;
        let first: serde_json::Value =
            serde_json::from_str(&rx.recv().await.expect("应收到增量帧")).expect("增量帧应是 JSON");
        assert_eq!(
            first["method"].as_str(),
            Some("evt.state.patch"),
            "开了增量应发 evt.state.patch"
        );
        assert_eq!(mine(&first), 1, "首帧应把本条线程的 item 推一次");

        // 3. 状态没变 → 本条线程的 upserted 必须为空（这就是省下来的字节）
        broadcaster.broadcast_immediate().await;
        let steady: serde_json::Value =
            serde_json::from_str(&rx.recv().await.expect("应收到稳态帧")).expect("稳态帧应是 JSON");
        assert_eq!(
            mine(&steady),
            0,
            "没变过的 item 不许出现在 upserted（别的线程有变化不算）"
        );
        // `baseSeq` 必须等于上一帧的 seq，客户端据此判断有没有缺帧
        assert_eq!(
            steady["params"]["baseSeq"].as_u64(),
            steady["params"]["seq"].as_u64().map(|s| s - 1),
            "baseSeq 必须是上一帧的 seq"
        );
    }

    /// 📏 **收益实测**（钉进 CI）：800 个 item 的长会话下，
    /// 全量帧 vs 增量稳态帧的字节数——这就是"MB 级 → 几十 KB"那句话的依据。
    #[tokio::test]
    async fn test_patch_frame_is_much_smaller_than_full_snapshot() {
        let store = Arc::new(RwLock::new(AgentStore::new("E:/test_patch_size".to_string())));
        let seq = Arc::new(AtomicU64::new(0));
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let broadcaster = StateBroadcaster::new(store.clone(), seq, tx);

        {
            let mut s = store.write().await;
            let tid = s.create_thread(Some("大会话".to_string()), None);
            for i in 0..800 {
                let cid = format!("call_{i}");
                s.start_tool_call(&tid, &cid, "run_command", "{}");
                s.finish_tool_call(
                    &tid,
                    &cid,
                    true,
                    Some(format!("输出 {i}：{}", "x".repeat(2048))),
                    Some(1),
                    Some(0),
                    Some(1),
                    None,
                );
            }
        }

        // 全量帧（当前线上行为）
        broadcaster.broadcast_immediate().await;
        let full = rx.recv().await.expect("全量帧");
        // 增量首帧：版本表为空 → 等价于把所有 item 推一次
        broadcaster.enable_patch_mode();
        broadcaster.broadcast_immediate().await;
        let first = rx.recv().await.expect("增量首帧");
        // 增量稳态帧：没有任何变化
        broadcaster.broadcast_immediate().await;
        let steady = rx.recv().await.expect("增量稳态帧");

        println!(
            "📏 800 item：全量帧 {} KB / 增量首帧 {} KB / 增量稳态帧 {} KB",
            full.len() / 1024,
            first.len() / 1024,
            steady.len() / 1024
        );
        assert!(
            steady.len() * 10 < full.len(),
            "增量稳态帧应当比全量小一个数量级（全量 {} 字节 / 稳态 {} 字节）",
            full.len(),
            steady.len()
        );
    }
}
