//! 领域事件：引擎产出的**唯一跨层契约**（INV-6）。
//!
//! 传输（inproc / WebSocket / CLI）、界面投影、日志都只消费这里的事件；
//! 事件带全局单调 `seq` 与线程归属，因此"缺口即重同步"可以机械判定。

use crate::domain::tool::ToolReceipt;
use crate::model::TokenUsage;

/// 一轮为什么结束。**没有隐式的步数上限**：设了预算就必须自报 `BudgetExhausted`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStopReason {
    /// 模型不再要工具（正常收尾）
    Completed,
    /// 用户或端口取消
    Aborted,
    ModelError,
    /// 显式预算耗尽（不是"悄悄截断"）
    BudgetExhausted { limit_steps: u32 },
    /// 被策略拒绝（不是错误）
    Denied,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEventBody {
    TurnStarted,
    ThinkingDelta { text: String },
    TextDelta { text: String },
    ToolCallStarted { call_id: String, name: String, args: serde_json::Value },
    ToolCallFinished { call_id: String, name: String, receipt: ToolReceipt },
    ApprovalRequested { call_id: String, tool: String },
    QuestionAsked { call_id: String, question: serde_json::Value },
    SubagentStarted { thread_id: String, subagent: String },
    SubagentFinished { thread_id: String, ok: bool, summary: String },
    UsageReported { usage: TokenUsage, duration_ms: u64 },
    TurnFinished { stop: TurnStopReason },
    Failed { message: String },
}

impl AgentEventBody {
    /// 事件类型名（日志/调试/线协议主题名用；与线协议 `evt.*` 的映射在 agent-proto 里做）。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::TurnStarted => "turn.started",
            Self::ThinkingDelta { .. } => "thinking.delta",
            Self::TextDelta { .. } => "text.delta",
            Self::ToolCallStarted { .. } => "tool.started",
            Self::ToolCallFinished { .. } => "tool.finished",
            Self::ApprovalRequested { .. } => "approval.requested",
            Self::QuestionAsked { .. } => "question.asked",
            Self::SubagentStarted { .. } => "subagent.started",
            Self::SubagentFinished { .. } => "subagent.finished",
            Self::UsageReported { .. } => "usage",
            Self::TurnFinished { .. } => "turn.finished",
            Self::Failed { .. } => "failed",
        }
    }
}

/// 带信封的事件：`seq` 全局单调，`thread_id` 说明归属（子线程事件也走同一条流）。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentEvent {
    pub seq: u64,
    pub at_ms: i64,
    pub thread_id: String,
    pub body: AgentEventBody,
}

impl AgentEvent {
    pub fn new(seq: u64, at_ms: i64, thread_id: impl Into<String>, body: AgentEventBody) -> Self {
        Self { seq, at_ms, thread_id: thread_id.into(), body }
    }
}
