//! 兼容 shim：审批策略配置已按 W1-T6 搬到 `agent-adapter::approval::policy`。
//!
//! 保留 `crate::approval::ApprovalGuardConfig` 路径，**调用点零改动**（计划 R6）。

pub use agent_adapter::approval::policy::ApprovalGuardConfig;
