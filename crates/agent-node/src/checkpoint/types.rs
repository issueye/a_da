//! 兼容 shim：检查点落盘类型已按 W1-T2a 搬到 `agent-adapter::store::checkpoint_types`。
//!
//! 保留 `crate::checkpoint::*` 路径，**调用点零改动**（计划 R6）。

pub use agent_adapter::store::checkpoint_types::*;
