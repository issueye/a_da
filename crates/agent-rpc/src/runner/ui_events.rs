//! 界面事件集（原 `agent_loop.rs`）。
//!
//! # 为什么这个文件还在，而 `run_agent_loop` 没了
//!
//! W3-T4 删掉了 legacy 主循环 `run_agent_loop`——仓库里现在只有 `agent-base` 一份
//! 多轮循环（`AgentRuntime::run_turn`，INV-1）。
//!
//! 但 `dispatch.rs` 里那套"投影到 `AgentStore` → 推给界面"的代码消费的是
//! [`AgentLoopEvent`]，而这套投影是几千行里最不该动的地方。所以事件类型**保留**，
//! 由 [`super::engine_bridge::LoopEventBridge`] 从领域事件（`AgentEvent`）投影出来：
//!
//! ```text
//! AgentRuntime::run_turn ──emit──▶ AgentEvent ──LoopEventBridge──▶ AgentLoopEvent ──▶ AgentStore ──▶ 界面
//! ```
//!
//! 也就是说：这个枚举现在是**纯投影层类型**，不再有"另一个引擎"的含义。
//! 它的名字里的 "Loop" 是历史遗留（`AgentLoopEvent` 已被前端 DTO 与多处代码引用，
//! 改名会带来跨层改动而无行为收益）。

/// 引擎产出、供界面消费的事件（由领域事件投影而来）。
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
    },
    ToolAwaitingQuestion { id: String, question: serde_json::Value },
    /// 工具调用**等待用户批准**（W3-T3）。
    ///
    /// 界面按 `status == "waiting_approval"` 渲染"批准/拒绝"按钮并回发 `approval.decide`，
    /// 最终唤醒引擎的审批闸门。真引擎的 `ApprovalRequested` 领域事件投影到这里。
    ApprovalRequested { id: String, tool: String },
    /// 一次大模型调用结束后的真实用量与耗时（界面遥测条与单条回复徽章的数据源）
    AssistantStats { usage: Option<agent_base::model::TokenUsage>, duration_ms: u64, turn_duration_ms: u64 },
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
                AgentLoopEvent::ToolAwaitingQuestion { .. } => "question",
                AgentLoopEvent::ApprovalRequested { .. } => "approval",
                AgentLoopEvent::AssistantStats { .. } => "stats",
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
            },
            AgentLoopEvent::ToolAwaitingQuestion { id: "i".into(), question: serde_json::json!({}) },
            AgentLoopEvent::ApprovalRequested { id: "i".into(), tool: "t".into() },
            AgentLoopEvent::AssistantStats { usage: None, duration_ms: 1, turn_duration_ms: 2 },
            AgentLoopEvent::TurnFinished { stop_reason: "stop".into() },
            AgentLoopEvent::Error { message: "m".into() },
        ];

        let mut kinds: Vec<&str> = all.iter().map(kind_of).collect();
        kinds.sort_unstable();
        let mut unique = kinds.clone();
        unique.dedup();
        assert_eq!(unique.len(), all.len(), "同一种类出现多次：{kinds:?}");
        assert_eq!(unique.len(), 9, "界面事件共 9 种：{unique:?}");
    }
}
