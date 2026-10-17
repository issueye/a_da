//! INV-6 的 `seq` 单调性审计：**一处实现，三个事件出口共用**。
//!
//! 不变量被复制多份，就等于没有不变量——此前 `WsEventSink`、`LoopEventBridge`、
//! `DirectEventSink` 各自内联了同一段 18 行"读 last_seq → 比较 → 记账 → 写回"。
//! 改判定规则（比如允许相等、或改为按 thread 分别计 seq）必然漏一处，
//! 而漏的那一处会**静默放行**违约。所以这里只留一份。
//!
//! 语义（三个出口原来就一致，此处保持）：
//! - `seq` 必须**严格递增**；回退或重复都算违约；
//! - 违约**只记账、不丢事件**——丢事件会让界面少显示东西，
//!   "记录并继续"比"静默吞掉"更接近可诊断；
//! - 记账用 `std::sync::Mutex` 做极短临界区，`emit` 是同步方法，不跨 await 持锁。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use agent_base::domain::AgentEvent;

/// `seq` 单调性守卫。每个事件出口持有一个实例。
#[derive(Debug, Default)]
pub struct SeqGuard {
    /// 已见过的最大 `seq`
    last: AtomicU64,
    /// 违约记录：回退或重复都记在这里，不静默
    violations: Mutex<Vec<String>>,
}

impl SeqGuard {
    pub const fn new() -> Self {
        Self {
            last: AtomicU64::new(0),
            violations: Mutex::new(Vec::new()),
        }
    }

    /// 校验一个事件的 `seq`：返回 `true` 表示递增（正常），`false` 表示违约（已记账）。
    ///
    /// 返回值的唯一用途是让调用方在需要时做诊断；**所有现有出口都选择继续派发**。
    pub fn check(&self, event: &AgentEvent) -> bool {
        let prev = self.last.load(Ordering::Relaxed);
        if event.seq <= prev {
            self.violations
                .lock()
                .expect("违约账本锁中毒")
                .push(format!(
                    "事件 seq 未严格递增：thread={} prev={} current={} kind={}",
                    event.thread_id,
                    prev,
                    event.seq,
                    event.body.kind()
                ));
            false
        } else {
            self.last.store(event.seq, Ordering::Relaxed);
            true
        }
    }

    /// 违约记录快照（**空 = 事件流有序**）。
    pub fn violations(&self) -> Vec<String> {
        self.violations.lock().expect("违约账本锁中毒").clone()
    }

    /// 违约条数（不必克隆整个账本）。
    pub fn violation_count(&self) -> usize {
        self.violations.lock().expect("违约账本锁中毒").len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::{AgentEvent, AgentEventBody};

    fn ev(seq: u64) -> AgentEvent {
        AgentEvent::new(seq, 1_700_000_000_000, "thread-x", AgentEventBody::TurnStarted)
    }

    #[test]
    fn strict_increase_is_clean() {
        let g = SeqGuard::new();
        assert!(g.check(&ev(1)));
        assert!(g.check(&ev(2)));
        assert_eq!(g.violation_count(), 0);
    }

    #[test]
    fn repeat_and_rollback_are_recorded() {
        let g = SeqGuard::new();
        assert!(g.check(&ev(5)));
        assert!(!g.check(&ev(5)), "重复 seq 必须违约");
        assert!(!g.check(&ev(4)), "回退 seq 必须违约");
        let v = g.violations();
        assert_eq!(v.len(), 2);
        assert!(v[0].contains("prev=5") && v[0].contains("current=5"));
    }
}
