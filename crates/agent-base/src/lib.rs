//! AGENT BASE 内核（`docs/agent-base-design.md` v0.2）
//!
//! 约束（设计 §3、INV-8）：
//! - **零 IO**：不得出现 `std::fs` / `std::env` / `std::process` / `reqwest` / `tokio::net`；
//!   时间只能经 [`ports::Clock`]，应用目录只能经 [`ports::AppHome`]。
//! - **零产品名词**：不得出现 `workspace` / `plugin` / `calendar` / `patch` 这类产品词汇，
//!   也不得依赖任何其它内部 crate。
//! - `std::path` 的**类型**（`Path` / `PathBuf`）允许作为值传递，但不得拿它做 IO。
//!
//! 分层：
//! - [`domain`] —— 领域类型：消息、工具调用与回执、事件、错误与失败方向
//! - [`ports`] —— 端口 trait：Clock / AppHome / EventSink / CancelToken / Scope / Tool(Catalog) / ModelClient / ApprovalGate
//! - [`model`] —— 模型面类型：对话消息、流式增量、用量、供应商配置
//! - [`testing`] —— 测试替身：FixedClock / RecordingSink / TempAppHome / NeverCancel
//!
//! 当前落地进度见 `docs/agent-base-plan.md` §8、§10。

pub mod domain;
pub mod model;
pub mod ports;
pub mod testing;
