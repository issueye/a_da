//! INV-6: 事件有序可重放不变量
//! 事件序列带严格单调递增 seq，快照带 seq，支持重试与重放。

use agent_base::domain::AgentEvent;

/// 断言事件流 seq 具备严格单调性
pub fn assert_events_seq_strictly_monotonic(events: &[AgentEvent]) -> Result<(), String> {
    let mut prev_seq = 0;
    for (i, ev) in events.iter().enumerate() {
        if i == 0 {
            prev_seq = ev.seq;
            continue;
        }
        if ev.seq <= prev_seq {
            return Err(format!("检测到 seq 乱序或重复: 第 {} 个事件 seq={} <= 前一个 {}", i, ev.seq, prev_seq));
        }
        prev_seq = ev.seq;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::AgentEventBody;

    #[test]
    fn test_inv6_event_monotonic_seq() {
        let events = vec![
            AgentEvent { seq: 10, thread_id: "t".into(), at_ms: 1000, body: AgentEventBody::TurnStarted },
            AgentEvent { seq: 11, thread_id: "t".into(), at_ms: 1005, body: AgentEventBody::TurnStarted },
            AgentEvent { seq: 12, thread_id: "t".into(), at_ms: 1010, body: AgentEventBody::TurnStarted },
        ];
        assert_events_seq_strictly_monotonic(&events).expect("单调递增序列通过");
    }
}
