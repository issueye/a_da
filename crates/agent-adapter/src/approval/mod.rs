//! 审批适配器：**策略**（纯判定）在这里；**执行**（问用户）在宿主。
//!
//! AGENTS.md §14：策略归插件 `approval-guard`、执行归核心 `askUser`，点位顺序不能反。

pub mod policy;

pub use policy::{
    extract_command, is_command_tool, is_destructive_command, should_ask_approval,
    ApprovalGuardConfig,
};
