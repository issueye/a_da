//! `EventSink` 端口的真实实现（`agent_base::ports::EventSink`）。
//!
//! 设计口径（`docs/agent-base-wiring-plan.md` §5 W1-T5）：
//! 引擎产出的领域事件**只有这一条出口**（INV-6）。当前传输层（`WsHostServer`）
//! 的粒度是**粗快照**（16ms 合帧的 `evt.state.snapshot`），细粒度 `evt.item.*`
//! 属计划附录 B 的 M6。所以这一版的真实 sink 做三件事：
//!
//! 1. **保序入队**：把领域事件按到达顺序记下来（供审计、回放与断言使用）；
//! 2. **`seq` 单调校验**：同一线程的 `seq` 必须严格递增，回退/重复**如实记账**，
//!    绝不静默吞掉（INV-6 的"缺口即重同步"要靠这个账本才成立）；
//! 3. **驱动传输**：`mark_dirty()` 让 16ms 合帧广播器把最新状态推给界面。
//!
//! `emit` 是**同步**方法（端口契约如此），因此这里只用 `std::sync::Mutex`
//! 做极短临界区，**不跨 await 持锁**，也不阻塞在通道上。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent_base::domain::AgentEvent;
use agent_base::ports::EventSink;

use crate::server::emitter::StateBroadcaster;

/// 送往界面的事件出口。
pub struct WsEventSink {
    /// 传输层广播器；`None` 表示"只记账不广播"（离线/测试形态）
    broadcaster: Option<Arc<StateBroadcaster>>,
    /// 保序事件账本
    events: Mutex<Vec<AgentEvent>>,
    /// 已见过的最大 `seq`（用于单调性校验）
    last_seq: AtomicU64,
    /// `seq` 违约记录：回退或重复都记在这里，不静默
    seq_violations: Mutex<Vec<String>>,
    /// 账本上限：超过就丢最旧的（防止长会话把内存吃光），丢弃会记账
    capacity: usize,
    dropped: AtomicU64,
}

impl WsEventSink {
    /// 建一个会驱动广播器的 sink。
    pub fn new(broadcaster: Arc<StateBroadcaster>) -> Self {
        Self::with_capacity(Some(broadcaster), 4096)
    }

    /// 只记账、不广播（离线形态；测试与 CLI 用）。
    pub fn recording_only() -> Self {
        Self::with_capacity(None, 4096)
    }

    pub fn with_capacity(broadcaster: Option<Arc<StateBroadcaster>>, capacity: usize) -> Self {
        Self {
            broadcaster,
            events: Mutex::new(Vec::new()),
            last_seq: AtomicU64::new(0),
            seq_violations: Mutex::new(Vec::new()),
            capacity: capacity.max(1),
            dropped: AtomicU64::new(0),
        }
    }

    /// 取走全部已记录事件（保序）。
    pub fn take_events(&self) -> Vec<AgentEvent> {
        std::mem::take(&mut *self.events.lock().expect("事件账本锁中毒"))
    }

    /// 只读快照（不清空）。
    pub fn snapshot(&self) -> Vec<AgentEvent> {
        self.events.lock().expect("事件账本锁中毒").clone()
    }

    /// `seq` 违约记录（回退/重复）。**空 = 事件流有序**。
    pub fn seq_violations(&self) -> Vec<String> {
        self.seq_violations.lock().expect("违约账本锁中毒").clone()
    }

    /// 因账本上限被丢弃的事件数。
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl EventSink for WsEventSink {
    fn emit(&self, event: AgentEvent) {
        // 1. INV-6：seq 必须严格递增。违约如实记账，但仍然入队——
        //    丢事件会让界面少显示东西，"记录并继续"比"静默吞掉"更接近可诊断。
        let prev = self.last_seq.load(Ordering::Relaxed);
        if event.seq <= prev {
            self.seq_violations
                .lock()
                .expect("违约账本锁中毒")
                .push(format!(
                    "事件 seq 未严格递增：thread={} prev={} current={} kind={}",
                    event.thread_id,
                    prev,
                    event.seq,
                    event.body.kind()
                ));
        } else {
            self.last_seq.store(event.seq, Ordering::Relaxed);
        }

        // 2. 保序入队（超限丢最旧并记账）
        {
            let mut events = self.events.lock().expect("事件账本锁中毒");
            if events.len() >= self.capacity {
                let overflow = events.len() + 1 - self.capacity;
                events.drain(0..overflow);
                self.dropped.fetch_add(overflow as u64, Ordering::Relaxed);
            }
            events.push(event);
        }

        // 3. 驱动传输：粗粒度快照（细粒度事件属 M6）
        if let Some(b) = &self.broadcaster {
            b.mark_dirty();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::{AgentEventBody, TurnStopReason};
    use agent_base::testing::RecordingSink;
    use crate::state::AgentStore;
    use tokio::sync::{mpsc, RwLock};

    fn ev(seq: u64, thread: &str, body: AgentEventBody) -> AgentEvent {
        AgentEvent::new(seq, 1_000 + seq as i64, thread, body)
    }

    fn broadcaster() -> (Arc<StateBroadcaster>, mpsc::UnboundedReceiver<String>) {
        let store = Arc::new(RwLock::new(AgentStore::new("E:/ws_sink_test".to_string())));
        let seq = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::unbounded_channel::<String>();
        (StateBroadcaster::new(store, seq, tx), rx)
    }

    #[test]
    fn test_events_are_recorded_in_order() {
        let sink = WsEventSink::recording_only();
        sink.emit(ev(1, "t1", AgentEventBody::TurnStarted));
        sink.emit(ev(2, "t1", AgentEventBody::TextDelta { text: "hi".into() }));
        sink.emit(ev(3, "t1", AgentEventBody::TurnFinished { stop: TurnStopReason::Completed }));

        let got = sink.snapshot();
        assert_eq!(got.len(), 3, "三个事件都必须被记录");
        assert_eq!(got[0].seq, 1);
        assert_eq!(got[1].seq, 2);
        assert_eq!(got[2].seq, 3);
        assert!(sink.seq_violations().is_empty(), "顺序正确时不应有违约记录");
    }

    #[test]
    fn test_seq_regression_is_recorded_not_swallowed() {
        let sink = WsEventSink::recording_only();
        sink.emit(ev(5, "t1", AgentEventBody::TurnStarted));
        sink.emit(ev(4, "t1", AgentEventBody::TextDelta { text: "back".into() }));

        let v = sink.seq_violations();
        assert_eq!(v.len(), 1, "seq 回退必须被记一条违约");
        assert!(v[0].contains("prev=5") && v[0].contains("current=4"), "违约信息要能定位：{v:?}");
        assert_eq!(sink.snapshot().len(), 2, "违约事件仍要入队，不许静默丢弃");
    }

    #[test]
    fn test_duplicate_seq_is_recorded() {
        let sink = WsEventSink::recording_only();
        sink.emit(ev(7, "t1", AgentEventBody::TurnStarted));
        sink.emit(ev(7, "t1", AgentEventBody::TurnStarted));
        assert_eq!(sink.seq_violations().len(), 1, "重复 seq 也必须记账");
    }

    #[tokio::test]
    async fn test_emit_drives_broadcaster() {
        let (b, mut rx) = broadcaster();
        let sink = WsEventSink::new(b);
        sink.emit(ev(1, "t1", AgentEventBody::TurnStarted));

        // mark_dirty 是同步的；等一个 16ms 窗口后广播器应推出快照帧
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        let frame = rx.try_recv().expect("emit 之后广播器必须推出快照帧");
        assert!(frame.contains("evt.state.snapshot"), "帧内容应是快照主题：{frame}");
    }

    #[test]
    fn test_capacity_drops_oldest_and_accounts_for_it() {
        let sink = WsEventSink::with_capacity(None, 3);
        for i in 1..=5u64 {
            sink.emit(ev(i, "t1", AgentEventBody::TextDelta { text: i.to_string() }));
        }
        let got = sink.snapshot();
        assert_eq!(got.len(), 3, "账本不得超过上限");
        assert_eq!(got[0].seq, 3, "超限时应丢最旧的");
        assert_eq!(sink.dropped_count(), 2, "丢弃必须记账");
    }

    #[test]
    fn test_take_events_clears_and_snapshot_does_not() {
        let sink = WsEventSink::recording_only();
        sink.emit(ev(1, "t1", AgentEventBody::TurnStarted));
        assert_eq!(sink.snapshot().len(), 1);
        assert_eq!(sink.take_events().len(), 1);
        assert!(sink.snapshot().is_empty(), "take 之后账本必须清空");
    }

    /// 真实 sink 与测试替身必须对同一份事件流给出一致结论（契约层面）。
    #[test]
    fn test_real_sink_agrees_with_recording_double_on_ordering() {
        let real = WsEventSink::recording_only();
        let double = RecordingSink::new();
        let stream = [
            ev(1, "t", AgentEventBody::TurnStarted),
            ev(2, "t", AgentEventBody::TextDelta { text: "a".into() }),
            ev(3, "t", AgentEventBody::TurnFinished { stop: TurnStopReason::Completed }),
        ];
        for e in stream.clone() {
            real.emit(e.clone());
            double.emit(e);
        }
        assert_eq!(real.snapshot(), double.snapshot(), "真实 sink 与替身的记录必须一致");
    }
}
