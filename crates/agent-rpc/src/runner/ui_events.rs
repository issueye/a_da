//! 界面事件集（原 `agent_loop.rs`，已收敛）。
//!
//! # 统一事件流演进
//!
//! 调度层 `dispatch.rs` 与引擎统一入口 `run_agent_turn` 现已全面收敛至直接消费微内核领域事件
//! [`agent_base::domain::AgentEvent`] 与 [`agent_base::domain::AgentEventBody`]：
//!
//! ```text
//! AgentRuntime::run_turn ──emit──▶ AgentEvent ──DirectEventSink──▶ dispatch.rs ──▶ AgentStore ──▶ 界面
//! ```
//!
//! 本文件中的 [`AgentLoopEvent`] 仅保留用于历史投影用例兼容与差集断言。

/// 历史过渡期界面事件集（已收敛至领域事件 `AgentEvent`）。
#[derive(Debug, Clone)]
pub enum AgentLoopEvent {
    Thinking { text: String },
    TextDelta { text: String },
    ToolCallStarted { name: String, id: String, args: String },
    ToolCallFinished {
        name: String,
        id: String,
        ok: bool,
        output: Option<String>,
        duration_ms: Option<u64>,
        started_at: Option<i64>,
        finished_at: Option<i64>,
        status: Option<String>,
        details: Option<serde_json::Value>,
    },
    /// 一次大模型调用结束后的真实用量与耗时（界面遥测条与单条回复徽章的数据源）
    AssistantStats { usage: Option<agent_base::model::TokenUsage>, duration_ms: u64, turn_duration_ms: u64 },
    /// 本轮因**上下文撞顶**而收尾（引擎的 `TurnStopReason::ContextWindowExceeded`）。
    ///
    /// 调度层据此**自动压缩并续跑**，而不是把"请手动压缩"丢给用户。
    ContextWindowExceeded { prompt_tokens: u64, window: u64 },
    TurnFinished { stop_reason: String },
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 界面事件集必须覆盖桥接投影会产出的**全部**类型。
    ///
    /// 穷尽 `match`：新增变体时这里编译失败，强迫作者同步投影与 UI 投影两侧
    /// （W3-T3 的教训：漏一个变体就是一条静默失效的链路）。
    #[test]
    fn test_ui_event_set_is_exhaustively_known() {
        fn kind_of(e: &AgentLoopEvent) -> &'static str {
            match e {
                AgentLoopEvent::Thinking { .. } => "thinking",
                AgentLoopEvent::TextDelta { .. } => "text",
                AgentLoopEvent::ToolCallStarted { .. } => "tool.started",
                AgentLoopEvent::ToolCallFinished { .. } => "tool.finished",
                AgentLoopEvent::AssistantStats { .. } => "stats",
                AgentLoopEvent::ContextWindowExceeded { .. } => "context.window.exceeded",
                AgentLoopEvent::TurnFinished { .. } => "turn.finished",
                AgentLoopEvent::Error { .. } => "error",
            }
        }

        let all = vec![
            AgentLoopEvent::Thinking { text: "t".into() },
            AgentLoopEvent::TextDelta { text: "t".into() },
            AgentLoopEvent::ToolCallStarted { name: "n".into(), id: "i".into(), args: "{}".into() },
            AgentLoopEvent::ToolCallFinished {
                name: "n".into(),
                id: "i".into(),
                ok: true,
                output: None,
                duration_ms: None,
                started_at: None,
                finished_at: None,
                status: None,
                details: None,
            },
            AgentLoopEvent::AssistantStats { usage: None, duration_ms: 1, turn_duration_ms: 2 },
            AgentLoopEvent::ContextWindowExceeded { prompt_tokens: 445_637, window: 128_000 },
            AgentLoopEvent::TurnFinished { stop_reason: "stop".into() },
            AgentLoopEvent::Error { message: "m".into() },
        ];

        let mut kinds: Vec<&str> = all.iter().map(kind_of).collect();
        kinds.sort_unstable();
        let mut unique = kinds.clone();
        unique.dedup();
        assert_eq!(unique.len(), all.len(), "同一种类出现多次：{kinds:?}");
        assert_eq!(unique.len(), 8, "界面事件共 8 种：{unique:?}");
    }
}
