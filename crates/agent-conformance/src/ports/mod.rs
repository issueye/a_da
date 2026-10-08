//! 8 个核心端口契约合规套件（`docs/agent-base-design.md` v0.2 §8.1）

pub mod approval;
pub mod event;
pub mod model;
pub mod plugin;
pub mod rollback;
pub mod scope;
pub mod session;
pub mod tool;

pub use approval::verify_approval_gate_contract;
pub use event::verify_events_conformance;
pub use model::verify_model_client_contract;
pub use plugin::verify_plugin_tool_descriptor;
pub use rollback::verify_rollback_policy_contract;
pub use scope::verify_scope_contract;
pub use session::verify_session_store_contract;
pub use tool::{verify_tool_catalog_contract, verify_tool_contract, verify_tool_receipt_contract};
