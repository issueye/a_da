//! 兼容 shim：工作区散列 / 会话 id 清洗已按 W1-T2a 搬到 `agent-adapter::store::slug`。
//!
//! 保留 `crate::session::{safe_id, workspace_slug}` 路径，**调用点零改动**（计划 R6）。

pub use agent_adapter::store::slug::*;
