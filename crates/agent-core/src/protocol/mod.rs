//! 兼容 shim：线协议已按设计搬到 `agent-proto`。
//!
//! 保留 `crate::protocol::*`（含 `crate::protocol::methods::X` 这类子模块路径）；
//! M1 收敛完后删除本文件。

pub use agent_proto::*;
