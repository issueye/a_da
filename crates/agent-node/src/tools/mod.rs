//! 兼容 shim：工具实现已按设计搬到 `agent-toolkit`。
//!
//! `is_readonly_tool` / `is_write_tool` 已改为**从 `ToolDescriptor` 派生**（W2-T2），
//! 本转发保留历史路径 `crate::tools::*`。

pub use agent_toolkit::*;
