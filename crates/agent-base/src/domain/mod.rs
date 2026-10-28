//! 领域模型：与产品无关、与传输无关、与落盘格式无关。
//!
//! 这一层是"什么是一个 agent 在跑"的唯一定义。适配器可以有无数实现，
//! 但它们交换的必须是这里的类型。

pub mod diag;
pub mod error;
pub mod event;
pub mod message;
pub mod plugin_event;
pub mod tool;
pub mod trace;

pub use diag::{
    DiagFamily, DiagOutcome, DiagnosticEvent, DIAG_POINTS, POINT_EVENT_ENTERED,
    POINT_EVENT_FINISHED, POINT_EVENT_REPLACE_APPLIED, POINT_EVENT_REPLACE_REJECTED,
    POINT_EVENT_SKIPPED, POINT_PLUGIN_DISCOVERED, POINT_PLUGIN_REPLACED_BUILTIN,
    POINT_PLUGIN_SKIPPED, POINT_PROMPT_COMPOSED, POINT_SCRIPT_FINISHED, POINT_SCRIPT_INVOKED,
    POINT_SKILL_INVALID, POINT_SKILL_LOADED, POINT_SKILL_SKIPPED, POINT_TURN_ABORTED,
};
pub use error::{AgentError, DenialKind};
pub use event::{AgentEvent, AgentEventBody, TurnStopReason};
pub use message::{AgentMessage, ToolCallBlock};
pub use plugin_event::{
    EventAction, EventEffect, EventFailure, EventLane, EventOutcome, EventSource, EventSpec,
    PluginEvent,
};
pub use tool::{
    Access, Execution, PathSelector, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
    ToolReceipt, ToolStatus,
};
pub use trace::{make_turn_id, Origin, TraceCtx};
