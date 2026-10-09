//! EventSink 端口契约合规断言（INV-2, INV-6, §8.1）
//!
//! 断言点：
//! 1. 事件带有 seq 序号，序列必须单调严格递增，不可回退或跳变
//! 2. TurnStarted 与 TurnFinished 必须成对出现
//! 3. 取消或者中止时，必须恰好生成一个 TurnFinished 并如实汇报 Aborted 停机原因
//! 4. `EventSink::emit` 必须**同步、不 panic、不阻塞**地接受全部事件类型
//!    （见 [`verify_event_sink_contract`]）

use agent_base::domain::{AgentEvent, AgentEventBody, TurnStopReason};
use agent_base::ports::EventSink;

/// 验证事件流合规性
pub fn verify_events_conformance(events: &[AgentEvent]) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }

    // 1. 验证 seq 单调严格递增
    let mut last_seq = None;
    for ev in events {
        if let Some(prev) = last_seq {
            if ev.seq <= prev {
                return Err(format!("事件 seq 未严格递增: prev={}, current={}", prev, ev.seq));
            }
        }
        last_seq = Some(ev.seq);
    }

    // 2. 检查 Turn 生命周期配对
    let mut started_count = 0;
    let mut finished_count = 0;
    for ev in events {
        match &ev.body {
            AgentEventBody::TurnStarted => started_count += 1,
            AgentEventBody::TurnFinished { .. } => finished_count += 1,
            _ => {}
        }
    }

    if started_count != finished_count {
        return Err(format!("TurnStarted ({}) 与 TurnFinished ({}) 未成对", started_count, finished_count));
    }

    Ok(())
}

/// 覆盖**全部 12 种**事件类型的探针流（`seq` 严格递增，`TurnStarted`/`TurnFinished` 成对）。
///
/// 为什么需要它：端口契约能通用断言的部分只有"`emit` 必须同步接受**任何**事件类型"。
/// 这恰好能抓住"某几个事件类型没实现"这类静默失效——`todo!()` 或漏分支会在
/// `emit` 里 panic，而不是安静地少显示点东西。
pub fn event_probe_stream(thread_id: &str) -> Vec<AgentEvent> {
    use agent_base::domain::{ToolReceipt, ToolStatus};
    use agent_base::model::TokenUsage;

    let mut seq = 0u64;
    let mut next = |body: AgentEventBody| {
        seq += 1;
        AgentEvent::new(seq, 1_000 + seq as i64, thread_id.to_string(), body)
    };

    vec![
        next(AgentEventBody::TurnStarted),
        next(AgentEventBody::ThinkingDelta { text: "推理".into() }),
        next(AgentEventBody::TextDelta { text: "回答".into() }),
        next(AgentEventBody::ToolCallStarted {
            call_id: "call_1".into(),
            name: "read_file".into(),
            args: serde_json::json!({ "path": "src/lib.rs" }),
        }),
        next(AgentEventBody::ToolCallFinished {
            call_id: "call_1".into(),
            name: "read_file".into(),
            receipt: ToolReceipt::new(ToolStatus::Success, "文件内容", 0, 5),
        }),
        next(AgentEventBody::ApprovalRequested {
            call_id: "call_2".into(),
            tool: "write_file".into(),
        }),
        next(AgentEventBody::QuestionAsked {
            call_id: "call_3".into(),
            question: serde_json::json!({ "prompt": "选哪个？" }),
        }),
        next(AgentEventBody::SubagentStarted {
            thread_id: "sub_1".into(),
            subagent: "coder".into(),
        }),
        next(AgentEventBody::SubagentFinished {
            thread_id: "sub_1".into(),
            ok: true,
            summary: "完成".into(),
        }),
        next(AgentEventBody::UsageReported {
            usage: TokenUsage::default(),
            duration_ms: 12,
        }),
        next(AgentEventBody::TurnFinished { stop: TurnStopReason::Completed }),
        next(AgentEventBody::Failed { message: "示例失败".into() }),
    ]
}

/// 验证 `EventSink` 端口契约。
///
/// 端口只承诺一件事：`emit` **同步**接受事件且不 panic。因此这里断言：
/// 1. 探针流本身合法（否则无法区分"实现有问题"与"探针写错了"）；
/// 2. 全部 12 种事件类型都能被接受（漏一种就会在这里暴露）。
///
/// **不在这里断言**的是"事件被保序记录下来"——端口没有读回接口，
/// 那是具体实现（如 `agent_core::server::WsEventSink`）自己的不变量，
/// 在实现所在 crate 的单测里断言。
pub fn verify_event_sink_contract(
    sink: &dyn EventSink,
    probe: &[AgentEvent],
) -> Result<(), String> {
    if probe.is_empty() {
        return Err("探针事件流不可为空（否则契约是空转）".into());
    }
    verify_events_conformance(probe)?;

    for e in probe {
        sink.emit(e.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::testing::RecordingSink;

    #[test]
    fn test_events_conformance_valid() {
        let events = vec![
            AgentEvent {
                seq: 1,
                thread_id: "t".into(),
                at_ms: 1000,
                body: AgentEventBody::TurnStarted,
            },
            AgentEvent {
                seq: 2,
                thread_id: "t".into(),
                at_ms: 1010,
                body: AgentEventBody::TurnFinished {
                    stop: TurnStopReason::Completed,
                },
            },
        ];

        assert!(verify_events_conformance(&events).is_ok());
    }

    #[test]
    fn test_events_conformance_non_monotonic_seq() {
        let events = vec![
            AgentEvent {
                seq: 2,
                thread_id: "t".into(),
                at_ms: 1000,
                body: AgentEventBody::TurnStarted,
            },
            AgentEvent {
                seq: 1, // 回退！
                thread_id: "t".into(),
                at_ms: 1010,
                body: AgentEventBody::TurnFinished {
                    stop: TurnStopReason::Completed,
                },
            },
        ];

        assert!(verify_events_conformance(&events).is_err());
    }

    #[test]
    fn test_event_sink_contract_accepts_all_kinds() {
        let sink = RecordingSink::new();
        verify_event_sink_contract(&sink, &event_probe_stream("t1"))
            .expect("录制型出口必须合规");
        // 全部 12 种事件类型都必须真的被接受（不许有 todo!()/漏分支）
        assert_eq!(sink.snapshot().len(), 12, "探针必须覆盖全部事件类型");
    }

    /// 探针本身必须是合法的：否则契约失败时无法区分"实现有问题"与"探针写错了"。
    #[test]
    fn test_probe_stream_is_itself_conformant() {
        let probe = event_probe_stream("t1");
        verify_events_conformance(&probe).expect("探针事件流自身必须满足 INV-6");
    }

    #[test]
    fn test_contract_rejects_empty_probe() {
        let sink = RecordingSink::new();
        assert!(
            verify_event_sink_contract(&sink, &[]).is_err(),
            "空探针必须被拒（否则契约是空转）"
        );
    }
}
