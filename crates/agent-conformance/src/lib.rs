//! AGENT CONFORMANCE 合规套件（`docs/agent-base-design.md` v0.2 §8）
//!
//! 提供：
//! 1. [`ports`] —— 8 个端口契约断言集合（ModelClient, ToolCatalog, ApprovalGate, SessionStore, RollbackStore, PluginHost, ScopePolicy, EventSink）
//! 2. [`invariants`] —— 8 条跨端口系统级不变量（单一引擎、隔离运行、注册表真源、失败闭合、结构化回执、单调 seq、领域投影分离、取消贯穿）

pub mod invariants;
pub mod ports;

pub use invariants::*;
pub use ports::*;
