//! 领域模型：与产品无关、与传输无关、与落盘格式无关。
//!
//! 这一层是"什么是一个 agent 在跑"的唯一定义。适配器可以有无数实现，
//! 但它们交换的必须是这里的类型。

pub mod error;
pub mod event;
pub mod message;
pub mod tool;

pub use error::{AgentError, DenialKind, FailDirection};
pub use event::{AgentEvent, AgentEventBody, TurnStopReason};
pub use message::{AgentMessage, ToolCallBlock};
pub use tool::{
    Access, ApprovalPolicy, Execution, PathSelector, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt, ToolStatus,
};
