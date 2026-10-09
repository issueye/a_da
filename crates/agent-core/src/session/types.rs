//! 兼容 shim：会话**落盘格式**已按 W1-T2a 搬到 `agent-adapter::store`（设计 §9：store-fs 属适配器）。
//!
//! 保留 `crate::session::*` 路径，**调用点零改动**（计划 R6）。
//! 本文件在 W1-T2b 完成、调用点全部改指适配器后删除。

pub use agent_adapter::store::types::*;
