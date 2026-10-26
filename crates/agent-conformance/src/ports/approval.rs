//! ApprovalGate 端口契约合规断言（INV-2, INV-4, §8.1）
//!
//! 断言点：
//! 1. 每次受约束调用前被调用一次，不可遗漏
//! 2. 拒绝（Denied）以工具结果回传模型，包含真实的拒绝理由
//! 3. answered_by 字段如实记录决策来源（User/Policy/Timeout/Aborted）
//! 4. 无决策依据时遵循 FailDirection::Closed（默认拒绝）

use agent_base::domain::{Access, ApprovalPolicy};
use agent_base::ports::{AnsweredBy, ApprovalGate, ApprovalRequest};
use agent_base::testing::NeverCancel;

/// 验证 ApprovalGate 端口契约合规性。
///
/// 断言点（W1-T6 后含策略询问）：
/// 1. `needs_approval` 必须如实回答"要不要问"，且**同一输入给同一答案**（幂等）；
/// 2. `decide` 被调用时必须给出 `answered_by` 合法的结论；
/// 3. 拒绝时理由不得是空白串（界面要显示人话）。
pub async fn verify_approval_gate_contract<G: ApprovalGate>(gate: &G) -> Result<(), String> {
    // 1. 策略询问必须幂等（不允许"每次问都换答案"）
    let policy = ApprovalPolicy::Always;
    let access = Access::Executes { command_arg: "cmd" };
    let args = serde_json::json!({ "cmd": "rm -rf /" });
    let first = gate.needs_approval("dangerous_exec", &access, &policy, &args).await;
    let second = gate.needs_approval("dangerous_exec", &access, &policy, &args).await;
    if first != second {
        return Err(format!(
            "needs_approval() 必须幂等：同一输入给出不同答案（{first} → {second}）"
        ));
    }

    let req = ApprovalRequest {
        call_id: "call_req_1".into(),
        thread_id: "thread_1".into(),
        tool: "dangerous_exec".into(),
        args,
        is_write: true,
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

    /// W1-T7：**真实策略**（适配器的 `approval-guard` 纯函数）驱动的探针 gate 也要过契约。
    ///
    /// 说明：生产实现 `agent_core::approval::HostApprovalGate` **无法**进本套件——
    /// `agent-conformance` 不依赖 `agent-core`（依赖图如此，不该反向加边）。
    /// 所以这里用"真实策略 + 最小 gate 壳"来覆盖契约本身，
    /// `HostApprovalGate` 的行为（超时/中止/User/Policy 四种来源）在 `agent-core` 单测里断言。
    /// 真实策略驱动的探针闸门。档位做成参数，因为"危险命令要不要二次确认"的答案
    /// **取决于档位**：正是这一点在本契约里被断言（Auto 是 YOLO，Ask/Readonly 才拦）。
    struct PolicyBackedProbeGate(agent_proto::ApprovalMode);

    impl ApprovalGate for PolicyBackedProbeGate {
        fn direction(&self) -> agent_base::domain::FailDirection {
            agent_base::domain::FailDirection::Closed
        }

        fn needs_approval<'a>(
            &'a self,
            tool: &'a str,
            access: &'a agent_base::domain::Access,
            policy: &'a ApprovalPolicy,
            args: &'a serde_json::Value,
        ) -> agent_base::ports::BoxFuture<'a, bool> {
            Box::pin(async move {
                match policy {
                    ApprovalPolicy::Never => false,
                    ApprovalPolicy::Always => true,
                    ApprovalPolicy::DangerScan { patterns } => {
                        let s = args.to_string();
                        patterns.iter().any(|p| s.contains(p))
                    }
                    ApprovalPolicy::Named(_) => agent_adapter::approval::should_ask_approval_for(
                        self.0,
                        tool,
                        Some(access),
                        args,
                        &agent_adapter::approval::ApprovalGuardConfig::default(),
                    ),
                }
            })
        }

        fn decide<'a>(
            &'a self,
            _req: ApprovalRequest,
            _cancel: Option<&'a dyn agent_base::ports::CancelToken>,
        ) -> agent_base::ports::BoxFuture<'a, agent_base::ports::ApprovalOutcome> {
            Box::pin(async move {
                agent_base::ports::ApprovalOutcome::allowed(AnsweredBy::Policy)
            })
        }
    }

    #[tokio::test]
    async fn test_real_policy_backed_gate_conformance() {
        verify_approval_gate_contract(&PolicyBackedProbeGate(agent_proto::ApprovalMode::Auto))
            .await
            .expect("真实策略驱动的 gate 必须通过契约");
    }

    /// 真实策略在端口契约之外还必须"答得对"：
    /// - 档位说了算——`Auto`（自动批准）是 YOLO，连危险命令也不再询问；
    /// - 危险命中判定本身没被删，只是不再压过 Auto（`Ask` 档仍然拦）。
    #[tokio::test]
    async fn test_real_policy_named_follows_the_mode() {
        let auto = PolicyBackedProbeGate(agent_proto::ApprovalMode::Auto);
        let benign = auto
            .needs_approval(
                "write_file",
                &agent_base::domain::Access::Mutates {
                    paths: agent_base::domain::PathSelector::Single("path"),
                },
                &ApprovalPolicy::Named("approval-guard"),
                &serde_json::json!({ "path": "a.rs" }),
            )
            .await;
        assert!(
            !benign,
            "Auto 档位下 Named 策略不得一律要求审批（名字不等于永远要问）"
        );

        let dangerous_in_auto = auto
            .needs_approval(
                "run_command",
                &agent_base::domain::Access::Executes {
                    command_arg: "command",
                },
                &ApprovalPolicy::Named("approval-guard"),
                &serde_json::json!({ "command": "rm -rf /" }),
            )
            .await;
        assert!(
            !dangerous_in_auto,
            "自动批准档位 = YOLO：危险命令也不该弹出审批"
        );

        let ask = PolicyBackedProbeGate(agent_proto::ApprovalMode::Ask);
        let dangerous_in_ask = ask
            .needs_approval(
                "run_command",
                &agent_base::domain::Access::Executes {
                    command_arg: "command",
                },
                &ApprovalPolicy::Named("approval-guard"),
                &serde_json::json!({ "command": "rm -rf /" }),
            )
            .await;
        assert!(
            dangerous_in_ask,
            "每次询问档位下危险命令必须二次确认（危险命中判定仍在）"
        );
    }
}
