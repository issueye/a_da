//! 兼容 shim：模型面已按设计搬到 `agent-base`（类型）与 `agent-adapter`（供应商 IO）。
//!
//! S4 拆包后 `agent-node` 也自带一份同名转发——两个 crate 都需要这条历史路径，
//! 而它只是 `pub use`，重复几行比引入一层间接更清楚。

pub use agent_adapter::model::*;
pub use agent_base::model::*;
