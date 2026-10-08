//! 事件出口：引擎产出的事件**只有这一条出口**（INV-6）。
//!
//! 现状是 `tokio::mpsc::Sender<AgentLoopEvent>` 直接长在 `run_agent_loop` 的签名里，
//! 且 `dispatch.rs` 用手写 match 消费两份；换成端口后，传输、录制、测试替身各实现各的。

use crate::domain::AgentEvent;

pub trait EventSink: Send + Sync {
    /// 同步入队；**顺序由引擎保证**（同一 `thread_id` 的 `seq` 单调递增）。
    fn emit(&self, event: AgentEvent);
}
