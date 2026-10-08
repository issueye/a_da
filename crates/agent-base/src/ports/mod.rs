//! 端口：内核与外部世界之间的**唯一接缝**。
//!
//! 规则（设计 §5、INV-2）：**契约性方法没有默认实现**。某个实现不支持某项能力时，
//! 必须返回 [`crate::domain::AgentError::Unsupported`] 或在能力位里如实声明不支持，
//! 不许给一个"什么都不做"的默认实现——那正是 AGENTS.md §15 那个静默坑的来源。

pub mod app_home;
pub mod approval;
pub mod cancel;
pub mod clock;
pub mod events;
pub mod model;
pub mod scope;
pub mod tools;

pub use app_home::AppHome;
pub use approval::{AnsweredBy, ApprovalGate, ApprovalOutcome, ApprovalRequest};
pub use cancel::CancelToken;
pub use clock::Clock;
pub use events::EventSink;
pub use model::{CompletionRequest, DeltaStream, ModelCapabilities, ModelClient, ModelError};
pub use scope::Scope;
pub use tools::{BoxFuture, Consumer, ContractViolation, Tool, ToolCatalog, ToolContext, ToolError};
