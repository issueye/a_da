//! 领域事件：引擎产出的**唯一跨层契约**（INV-6）。
//!
//! 传输（inproc / WebSocket / CLI）、界面投影、日志都只消费这里的事件；
//! 事件带全局单调 `seq` 与线程归属，因此"缺口即重同步"可以机械判定。

use crate::domain::diag::DiagnosticEvent;
use crate::domain::tool::ToolReceipt;
use crate::domain::trace::TraceCtx;
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
    /// 上下文已达窗口上限：**提前失败**，而不是发一个注定被拒的请求。
    ///
    /// 实测教训：445k tokens 的请求被网关在流式过程中掐断，报告只写了一半，
    /// 界面看到的却是笼统的"模型调用失败"。设了窗口就必须自报"是上下文撞顶了"。
    ContextWindowExceeded { prompt_tokens: u64, window: u64 },
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
    /// 请求用户批准这次工具调用。
    ///
    /// 🔴 **必须带 `args`**：审批卡要回答的是"我到底在批准什么"——危险命令的原文、
    /// 要覆写的文件路径。只给一个工具名，用户只能闭着眼点"批准"，那道确认闸门就白设了。
    /// 这也是本事件与 [`Self::ToolCallStarted`] 的分工：审批在前、建卡在后，
    /// 审批卡拿不到 `ToolCallStarted` 的 args，只能自己带。
    ApprovalRequested { call_id: String, tool: String, args: serde_json::Value },
    QuestionAsked { call_id: String, question: serde_json::Value },
    SubagentStarted { thread_id: String, subagent: String },
    SubagentFinished { thread_id: String, ok: bool, summary: String },
    UsageReported { usage: TokenUsage, duration_ms: u64 },
    TurnFinished { stop: TurnStopReason },
    Failed { message: String },
    /// **诊断遥测**（扩展点埋点）：只给人看，永不进模型上下文，也不进会话 items 的正文。
    ///
    /// 见 [`crate::domain::diag`]：一个变体承载全部扩展点遥测，点位名在 `point` 字段里。
    /// 分开的理由是"加一个点位不该动五个文件的穷尽匹配"。
    Diagnostic(DiagnosticEvent),
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
            Self::Diagnostic(_) => "diagnostic",
        }
    }
}

/// 带信封的事件：`seq` 全局单调，`thread_id` 说明归属（子线程事件也走同一条流）。
///
/// `trace` 是**埋点关联上下文**（P0）：轮次 id / 步号 / 工具调用 id / 来源标注。
/// 它挂在信封上而不是各个 body 上——一次改动覆盖全部事件类型，且既有构造点
/// （宿主、子智能体、契约测试）用 [`TraceCtx::none`] 就能保持原样。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentEvent {
    pub seq: u64,
    pub at_ms: i64,
    pub thread_id: String,
    pub trace: TraceCtx,
    pub body: AgentEventBody,
}

impl AgentEvent {
    /// 无轮次上下文的构造（既有调用点语义不变）。
    pub fn new(seq: u64, at_ms: i64, thread_id: impl Into<String>, body: AgentEventBody) -> Self {
        Self {
            seq,
            at_ms,
            thread_id: thread_id.into(),
            trace: TraceCtx::none(),
            body,
        }
    }

    /// 带关联上下文的构造（引擎路径用这个）。
    pub fn with_trace(
        seq: u64,
        at_ms: i64,
        thread_id: impl Into<String>,
        trace: TraceCtx,
        body: AgentEventBody,
    ) -> Self {
        Self {
            seq,
            at_ms,
            thread_id: thread_id.into(),
            trace,
            body,
        }
    }
}
