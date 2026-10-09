//! INV-4: 失败方向在类型里不变量
//!
//! 失败策略默认采用 `FailDirection::Closed`（失败时阻断而非放行）。
//!
//! # 为什么要从"断言 `default()`"升级（W5-T5）
//!
//! 旧版本只有 `assert_eq!(FailDirection::default(), FailDirection::Closed)`——
//! 那是在断言**枚举的 `#[default]` 属性**，与"系统在拿不到判定依据时真的会拒绝"
//! 没有关系。它是一个**重言式**：把引擎里所有方向裁决都删掉，它照样绿。
//!
//! 现在断言的是**引擎行为**：
//! 1. 无判定依据（`Timeout`/`Aborted`）+ `Closed` → 工具**不被执行**，回执 `Denied`；
//! 2. 无判定依据 + `Open` → 工具**被执行**（`direction()` 真的有用）；
//! 3. 确定答案（`User`/`Policy`）→ **不受方向影响**（人的决定不能被 fail-open 翻掉）。

use std::sync::Arc;

use agent_base::domain::{
    Access, ApprovalPolicy, Execution, FailDirection, RollbackPolicy, Termination, ToolDescriptor,
    ToolReceipt, ToolStatus, TurnStopReason,
};
use agent_base::engine::TurnRequest;
use agent_base::model::{ProviderConfig, StreamDelta, ToolCallInfo};
use agent_base::ports::{ApprovalGate, ApprovalOutcome, AnsweredBy};
use agent_base::testing::{
    FixedClock, FixedPrompt, InMemorySessionStore, MockScope, MockTool,
    NeverCancel, RecordingApprovalGate, RecordingSink, ScriptedModelClient,
};
use agent_runtime::{AgentSpec, ProductBuilder};

fn spec() -> AgentSpec {
    AgentSpec::from_json_str(
        r#"{
        "id": "inv4_test",
        "archetype": "assistant",
        "identity": { "name": "测试", "persona": "persona", "locale": "zh-CN" },
        "toolkits": ["core"],
        "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
        "policies": { "maxSteps": 5, "parallelTools": 1, "toolTimeoutSec": 30 }
    }"#,
    )
    .expect("spec 合法")
}

fn provider_config() -> ProviderConfig {
    ProviderConfig {
        id: "p".into(),
        name: "p".into(),
        protocol: Default::default(),
        base_url: "http://localhost".into(),
        api_key: "k".into(),
        model: "m".into(),
        max_output_tokens: None,
        custom_headers: None,
        proxy_url: None,
    }
}

fn gated_tool() -> Arc<dyn agent_base::ports::Tool> {
    Arc::new(MockTool::new(
        ToolDescriptor {
            name: "gated_tool".to_string(),
            summary: "受审批约束的工具".to_string(),
            schema: serde_json::json!({ "type": "object" }),
            access: Access::ReadOnly,
            approval: ApprovalPolicy::Always,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        },
        ToolReceipt::success("工具真的跑了", 0, 1),
    ))
}

/// 跑一轮"受审批约束的工具调用"，返回工具回执状态。
async fn receipt_status_for(
    answered_by: AnsweredBy,
    approved: bool,
    direction: FailDirection,
) -> Result<ToolStatus, String> {
    let outcome = if approved {
        ApprovalOutcome::allowed(answered_by)
    } else {
        ApprovalOutcome::denied(answered_by, "没有判定依据")
    };
    let gate = Arc::new(
        RecordingApprovalGate::with_outcomes(vec![outcome]).with_direction(direction),
    );
    if ApprovalGate::direction(gate.as_ref()) != direction {
        return Err("闸门替身没有如实声明方向".into());
    }

    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: ToolCallInfo {
                    id: "c1".into(),
                    name: "gated_tool".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::Done { stop_reason: "tool_calls".into() },
        ],
        vec![
            StreamDelta::Text { text: "done".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ],
    ]));

    let rt = ProductBuilder::new(spec())
        .with_tool(gated_tool())
        .with_model(model)
        .with_approval(gate)
        .with_store(Arc::new(InMemorySessionStore::new()))
        .with_prompt(Arc::new(FixedPrompt::new("p")))
        .with_scope(Arc::new(MockScope::new("s")))
        .with_clock(Arc::new(FixedClock::new(100)))
        .build()
        .map_err(|e| e.to_string())?;

    let sink = RecordingSink::new();
    let outcome = rt
        .run_turn(
            TurnRequest::new("inv4", provider_config()).with_user_prompt("go"),
            &sink,
            &NeverCancel,
        )
        .await
        .map_err(|e| e.to_string())?;
    if outcome.stop_reason != TurnStopReason::Completed {
        return Err(format!("轮次应正常结束，实际 {:?}", outcome.stop_reason));
    }

    let mut statuses: Vec<ToolStatus> = sink
        .snapshot()
        .iter()
        .filter_map(|e| match &e.body {
            agent_base::domain::AgentEventBody::ToolCallFinished { receipt, .. } => {
                Some(receipt.status.clone())
            }
            _ => None,
        })
        .collect();
    if statuses.len() != 1 {
        return Err(format!("应恰好一个工具回执，实际 {:?}", statuses));
    }
    Ok(statuses.remove(0))
}

/// INV-4 断言：默认方向为 `Closed`，且**引擎真的按它裁决**。
pub async fn assert_fail_direction_is_consumed() -> Result<(), String> {
    if FailDirection::default() != FailDirection::Closed {
        return Err("基座 FailDirection 默认值必须是 Closed".into());
    }

    // 1. 无判定依据 + Closed → 拒绝（工具没跑）
    let closed = receipt_status_for(AnsweredBy::Timeout, false, FailDirection::Closed).await?;
    if closed != ToolStatus::Denied {
        return Err(format!("Closed 时超时必须拒绝，实际 {closed:?}"));
    }

    // 2. 无判定依据 + Open → 放行（**direction() 真的起作用**）
    let open = receipt_status_for(AnsweredBy::Timeout, false, FailDirection::Open).await?;
    if open != ToolStatus::Success {
        return Err(format!("Open 时超时必须放行，实际 {open:?}"));
    }

    // 3. 安全网：闸门声明 Closed 却对"没答案"给了放行 → 引擎翻成拒绝
    let net = receipt_status_for(AnsweredBy::Aborted, true, FailDirection::Closed).await?;
    if net != ToolStatus::Denied {
        return Err(format!(
            "声明 Closed 时不得放行无判定依据的调用，实际 {net:?}"
        ));
    }

    // 4. 确定答案不受方向影响：`User` 拒绝 + Open 方向 → 仍然拒绝
    let user_denied = receipt_status_for(AnsweredBy::User, false, FailDirection::Open).await?;
    if user_denied != ToolStatus::Denied {
        return Err(format!(
            "用户明确拒绝不得被 fail-open 覆盖，实际 {user_denied:?}"
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inv4_fail_direction() {
        assert_fail_direction_is_consumed()
            .await
            .expect("失败方向必须被真实消费");
    }
}
