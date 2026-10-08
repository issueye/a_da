//! 兼容 shim：模型面已按设计搬到 `agent-base`（类型）与 `agent-adapter`（供应商 IO）。
//!
//! 保留 `crate::ai::*` 这条历史路径，避免一次性改动所有调用点；M1 收敛完后删除本文件。

pub use agent_adapter::model::*;
pub use agent_base::model::*;
