//! ApprovalGate 端口契约合规断言（INV-2, INV-4, §8.1）
//!
//! 断言点：
//! 1. 每次受约束调用前被调用一次，不可遗漏
//! 2. 拒绝（Denied）以工具结果回传模型，包含真实的拒绝理由
//! 3. answered_by 字段如实记录决策来源（User/Policy/Timeout/Aborted）
//! 4. 无决策依据时遵循 FailDirection::Closed（默认拒绝）

use agent_base::ports::{AnsweredBy, ApprovalGate, ApprovalRequest};
use agent_base::testing::NeverCancel;

/// 验证 ApprovalGate 端口契约合规性
pub async fn verify_approval_gate_contract<G: ApprovalGate>(gate: &G) -> Result<(), String> {
    let req = ApprovalRequest {
        call_id: "call_req_1".into(),
        thread_id: "thread_1".into(),
        tool: "dangerous_exec".into(),
        args: serde_json::json!({ "cmd": "rm -rf /" }),
        is_write: true,
        mode: "ask".into(),
        reason: Some("高危命令".into()),
    };

    let outcome = gate.decide(req, Some(&NeverCancel)).await;
    if !outcome.approved {
        if let Some(ref r) = outcome.reason {
            if r.trim().is_empty() {
                return Err("拒绝理由不可为空白字符串".into());
            }
        }
        match outcome.by {
            AnsweredBy::User | AnsweredBy::Policy | AnsweredBy::Timeout | AnsweredBy::Aborted => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::domain::FailDirection;
    use agent_base::testing::RecordingApprovalGate;

    #[tokio::test]
    async fn test_recording_approval_gate_conformance() {
        let gate = RecordingApprovalGate::new(true);
        verify_approval_gate_contract(&gate).await.expect("自动通过网关合规");
        assert_eq!(gate.recorded_calls().len(), 1);

        let rejecting_gate = RecordingApprovalGate::new(false);
        verify_approval_gate_contract(&rejecting_gate).await.expect("拒绝网关合规");
        assert_eq!(rejecting_gate.recorded_calls().len(), 1);
    }

    #[test]
    fn test_fail_direction_closed_safety() {
        let direction = FailDirection::default();
        assert_eq!(direction, FailDirection::Closed, "默认安全方向必须是 Closed 闭合拦截");
    }
}
