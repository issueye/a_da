//! 端口契约合规套件（`docs/architecture/kernel-design.md` v0.2 §8.1）
//!
//! 端口清单以 `crates/agent-base/src/ports/` 的 `pub trait` 为**唯一真源**；
//! `cargo xtask verify-wiring` 会从 trait 定义派生清单、逐个要求有生产实现。
//! （此处曾写死"8 个端口"，与事实不符——清单会随端口增加而变，所以不再写数字。）

pub mod approval;
pub mod cancel;
pub mod event;
pub mod model;
pub mod plugin;
pub mod prompt;
pub mod rollback;
pub mod scope;
pub mod script;
pub mod session;
pub mod tool;

pub use approval::verify_approval_gate_contract;
pub use cancel::verify_cancel_contract;
pub use event::{event_probe_stream, verify_event_sink_contract, verify_events_conformance};
pub use model::verify_model_client_contract;
pub use plugin::verify_plugin_tool_descriptor;
pub use prompt::verify_prompt_source_contract;
pub use rollback::verify_rollback_policy_contract;
pub use scope::verify_scope_contract;
pub use script::{
    verify_script_host_contract, write_script_probe, ScriptProbe, PROBE_ECHO_EXPORT,
    PROBE_SPIN_EXPORT,
};
pub use session::verify_session_store_contract;
pub use tool::{verify_tool_catalog_contract, verify_tool_contract, verify_tool_receipt_contract};
