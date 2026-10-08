//! 工具调用与回执：**能力由描述符声明**（INV-3），**回执结构化且字段必填**（INV-5）。
//!
//! 这一层是 AGENTS.md §1/§2/§4/§5/§6/§10/§18 的"类型化"落点：
//! - 只读性、审批策略、回滚策略、执行模式、终止语义全部来自 [`ToolDescriptor`]，
//!   不再有 `is_readonly_tool` 那样的按名字名单；
//! - [`ToolReceipt`] 的 `duration_ms` 是**派生**的，不是并列字段，因此不存在"两个键写歪一个"的静默失效。

use serde::{Deserialize, Serialize};

/// 工具调用请求（最小面；参数保持原始 JSON，避免提前绑定各家的 schema 风格）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

/// 工具对"作用域"的访问方式（取代按名字的只读判定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    /// 会改动作用域内的路径；`PathSelector` 说明参数里哪个字段是路径
    Mutates { paths: PathSelector },
    /// 会执行外部命令；`command_arg` 是参数里的命令字段名
    Executes { command_arg: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSelector {
    /// 单目标：参数里一个路径字段
    Single(&'static str),
    /// 批量：参数里一个"文件数组"字段（每个元素都要进回滚记录，见 §4）
    Batch(&'static str),
}

/// 审批策略（取代"谁决定免问"散在几处的现状）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalPolicy {
    Never,
    Always,
    /// 命中危险模式才问（即使用户开了自动批准）
    DangerScan { patterns: Vec<String> },
    /// 由命名的策略实现决定（如插件提供的 approval-guard）
    Named(&'static str),
}

/// 回滚策略（取代 coding 专有的"写前检查点"手写分支）。生活类助手可以换成别的实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackPolicy {
    None,
    SingleTarget,
    PerTargetInBatch,
}

/// 执行模式（取代"整批都声明 parallel 才重叠"的隐式约定，AGENTS.md §6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Execution {
    Sequential,
    ParallelSafe,
}

/// 终止语义（取代 `ToolResult.terminate` 被丢弃的现状，AGENTS.md §5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    ContinueTurn,
    EndTurn,
}

/// 工具自述：**注册表即真源**（INV-3）。任何消费侧（只读过滤、审批、回滚、子智能体白名单、
/// 插件声明）都必须从这里取，不许各写一份名单。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDescriptor {
    pub name: String,
    pub summary: String,
    pub schema: serde_json::Value,
    pub access: Access,
    pub approval: ApprovalPolicy,
    pub rollback: RollbackPolicy,
    pub execution: Execution,
    pub termination: Termination,
}

impl ToolDescriptor {
    /// 是否只读——所有"只读过滤"都必须问这一句，而不是查名单。
    pub fn is_readonly(&self) -> bool {
        matches!(self.access, Access::ReadOnly)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Success,
    Error,
    Denied,
    Timeout,
    Aborted,
}

/// 工具回执（INV-5）：字段必填、时间必填、耗时派生。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolReceipt {
    pub status: ToolStatus,
    pub output: String,
    pub data: Option<serde_json::Value>,
    pub details: Option<serde_json::Value>,
    pub started_at: i64,
    pub finished_at: i64,
}

impl ToolReceipt {
    pub fn new(status: ToolStatus, output: impl Into<String>, started_at: i64, finished_at: i64) -> Self {
        Self { status, output: output.into(), data: None, details: None, started_at, finished_at }
    }

    pub fn success(output: impl Into<String>, started_at: i64, finished_at: i64) -> Self {
        Self::new(ToolStatus::Success, output, started_at, finished_at)
    }

    pub fn error(output: impl Into<String>, started_at: i64, finished_at: i64) -> Self {
        Self::new(ToolStatus::Error, output, started_at, finished_at)
    }

    /// 耗时由起止时间派生——**不存在**"duration 字段没填"这种状态。
    pub fn duration_ms(&self) -> u64 {
        (self.finished_at - self.started_at).max(0) as u64
    }

    pub fn ok(&self) -> bool {
        matches!(self.status, ToolStatus::Success)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_derives_duration_and_never_goes_negative() {
        let r = ToolReceipt::success("ok", 1_000, 1_250);
        assert_eq!(r.duration_ms(), 250);
        assert!(r.ok());

        // 时钟回拨（NTP/夏令时）不能让耗时变成负数或回绕成大数
        let skewed = ToolReceipt::success("ok", 2_000, 1_000);
        assert_eq!(skewed.duration_ms(), 0);
    }

    #[test]
    fn readonly_comes_from_access_not_from_a_name_list() {
        let mut d = ToolDescriptor {
            name: "whatever".into(),
            summary: String::new(),
            schema: serde_json::json!({ "type": "object" }),
            access: Access::ReadOnly,
            approval: ApprovalPolicy::Never,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        };
        assert!(d.is_readonly());

        // 同一个名字，只要描述符说是写——它就是写（这正是不看名字的好处）
        d.access = Access::Mutates { paths: PathSelector::Single("path") };
        assert!(!d.is_readonly());
    }
}
