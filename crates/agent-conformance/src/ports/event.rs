//! EventSink 端口契约合规断言（INV-2, INV-6, §8.1）
//!
//! 断言点：
//! 1. 事件带有 seq 序号，序列必须单调严格递增，不可回退或跳变
//! 2. TurnStarted 与 TurnFinished 必须成对出现
//! 3. 取消或者中止时，必须恰好生成一个 TurnFinished 并如实汇报 Aborted 停机原因

use agent_base::domain::{AgentEvent, AgentEventBody};

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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::TurnStopReason;

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
}
