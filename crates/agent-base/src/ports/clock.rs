//! 时间端口：内核里**不许**直接读系统时间。
//!
//! 现状是每个模块各自 `SystemTime::now()`（`runner::executor::now_ms`、`state::now_millis`…），
//! 于是"同一件事在不同地方用不同时间源"，测试也无法确定性重放。

/// 时间源。实现放在适配器（`agent-adapter::clock::SystemClock`）/ 测试替身（`testing::FixedClock`）。
pub trait Clock: Send + Sync {
    /// 当前时间的 Unix 毫秒时间戳。
    fn now_ms(&self) -> i64;
}
