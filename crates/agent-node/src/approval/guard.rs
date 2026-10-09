//! 兼容 shim：审批**策略**已按 W1-T6 搬到 `agent-adapter::approval::policy`。
//!
//! 保留 `crate::approval::{should_ask_approval, is_destructive_command, extract_command}`
//! 路径，**调用点零改动**（计划 R6）。策略是纯函数、无 IO、无单例，因此可以安全地
//! 放进适配器（与 AppHome 单例的情况不同，见计划 §13.2 第 6 条）。

pub use agent_adapter::approval::policy::{
    extract_command, is_command_tool, is_destructive_command, should_ask_approval,
};
