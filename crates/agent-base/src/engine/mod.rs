//! AGENT BASE 内核引擎模块。
//!
//! 包含：
//! - [`AgentRuntime`]：多轮驱动与收尾保证的单一引擎
//! - [`RunPolicy`]：运行策略（步数预算、工具并发等）
//! - [`TurnRequest`] 与 [`TurnOutcome`]：单轮请求与执行产出
//! - [`format_messages_for_model`]：消息格式化纯函数

pub mod policy;
pub mod prompt_format;
pub mod runtime;
pub mod turn;

pub use policy::RunPolicy;
pub use prompt_format::format_messages_for_model;
pub use runtime::AgentRuntime;
pub use turn::{TurnOutcome, TurnRequest};
