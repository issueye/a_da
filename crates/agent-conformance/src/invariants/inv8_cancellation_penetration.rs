//! 取消贯穿不变量
//! 取消令牌一旦触发，多轮循环与工具执行立即中止，并生成最终 Aborted 停机回执。

use std::sync::Arc;
use agent_base::domain::TurnStopReason;
use agent_base::engine::TurnRequest;
use agent_base::model::{ProviderConfig, StreamDelta};
use agent_base::testing::{FixedClock, FixedPrompt, InMemorySessionStore, ManualCancel, MockScope, RecordingApprovalGate, RecordingSink, ScriptedModelClient};
use agent_runtime::{AgentSpec, ProductBuilder};
use agent_toolkit::core::FinishTool;

/// 断言取消令牌贯穿中断执行流
pub async fn assert_cancellation_penetrates_turn() -> Result<(), String> {
    let spec = AgentSpec::from_json_str(r#"{
        "id": "cancel_test",
        "archetype": "assistant",
        "identity": { "name": "测试", "persona": "persona", "locale": "zh-CN" },
        "toolkits": ["core"],
        "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
        "policies": { "maxSteps": 5, "parallelTools": 1, "toolTimeoutSec": 30 }
    }"#).map_err(|e| e.to_string())?;

    let cancel = ManualCancel::new();
    cancel.cancel(); // 提前置为已取消

    let model = Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::Text { text: "由于取消，这里不会被消费".into() },
        StreamDelta::Done { stop_reason: "stop".into() },
    ]]));

    let rt = ProductBuilder::new(spec)
        .with_tool(Arc::new(FinishTool::default()))
        .with_model(model)
        .with_approval(Arc::new(RecordingApprovalGate::new(true)))
        .with_store(Arc::new(InMemorySessionStore::new()))
        .with_prompt(Arc::new(FixedPrompt::new("p")))
        .with_scope(Arc::new(MockScope::new("s")))
        .with_clock(Arc::new(FixedClock::new(100)))
        .build().map_err(|e| e.to_string())?;

    let sink = RecordingSink::new();
    let req = TurnRequest::new(
        "thread_cancel",
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
        },
    );

    let outcome = rt.run_turn(req, &sink, &cancel).await.map_err(|e| e.to_string())?;
    assert_eq!(outcome.stop_reason, TurnStopReason::Aborted, "取消后停机原因必须是 Aborted");

    Ok(())
}

/// **执行中**取消必须穿透到工具（W4-T4）。
///
/// 上面那条只覆盖"启动前已取消"——那是最弱的一种情形：引擎在进循环前就返回了，
/// 根本没走到 `Tool::execute`。真正要证明的是：**轮次已经跑起来、工具正在执行时**
/// 翻转取消令牌，工具能拿到它并提前退出，且轮次以 `Aborted` 收尾。
///
/// 这正是 P0-4（取消链断裂）的合规层证据：`run_command` 的假接线（W4-T1）
/// 在端口层就表现为"工具收不到取消"。
pub async fn assert_cancellation_penetrates_running_tool() -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use agent_base::domain::{
        Access, ApprovalPolicy, Execution, RollbackPolicy, Termination, ToolCall, ToolDescriptor,
        ToolReceipt, ToolStatus,
    };
    use agent_base::ports::{BoxFuture, Tool, ToolContext};

    /// 会一直阻塞、直到观察到取消的测试工具。
    struct SlowTool {
        descriptor: ToolDescriptor,
        observed_cancel: Arc<AtomicBool>,
    }

    impl Tool for SlowTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }

        fn execute<'a>(
            &'a self,
            _call: &'a ToolCall,
            ctx: &'a ToolContext<'a>,
        ) -> BoxFuture<'a, ToolReceipt> {
            Box::pin(async move {
                let started = ctx.cancel.is_cancelled() as i64;
                // 最多等 10s；只要取消翻转就立刻退出并如实记账
                for _ in 0..500 {
                    if ctx.cancel.is_cancelled() {
                        self.observed_cancel.store(true, Ordering::SeqCst);
                        return ToolReceipt::new(
                            ToolStatus::Aborted,
                            "工具收到取消并提前退出",
                            started,
                            started + 1,
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                ToolReceipt::new(ToolStatus::Error, "工具没有被取消（超时）", started, started + 1)
            })
        }
    }

    let spec = AgentSpec::from_json_str(
        r#"{
        "id": "cancel_running_test",
        "archetype": "assistant",
        "identity": { "name": "测试", "persona": "persona", "locale": "zh-CN" },
        "toolkits": ["core"],
        "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
        "policies": { "maxSteps": 5, "parallelTools": 1, "toolTimeoutSec": 30 }
    }"#,
    )
    .map_err(|e| e.to_string())?;

    let observed = Arc::new(AtomicBool::new(false));
    let slow_tool = Arc::new(SlowTool {
        descriptor: ToolDescriptor {
            name: "slow_tool".to_string(),
            summary: "阻塞直到被取消".to_string(),
            schema: serde_json::json!({ "type": "object" }),
            access: Access::ReadOnly,
            approval: ApprovalPolicy::Never,
            rollback: RollbackPolicy::None,
            execution: Execution::Sequential,
            termination: Termination::ContinueTurn,
        },
        observed_cancel: observed.clone(),
    });

    // 模型第一轮就要调这个阻塞工具
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            StreamDelta::ToolCall {
                call: agent_base::model::ToolCallInfo {
                    id: "call_slow".into(),
                    name: "slow_tool".into(),
                    args: "{}".into(),
                },
            },
            StreamDelta::Done { stop_reason: "tool_calls".into() },
        ],
        vec![
            StreamDelta::Text { text: "不该走到这里".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ],
    ]));

    let rt = ProductBuilder::new(spec)
        .with_tool(slow_tool)
        .with_model(model)
        .with_approval(Arc::new(RecordingApprovalGate::new(true)))
        .with_store(Arc::new(InMemorySessionStore::new()))
        .with_prompt(Arc::new(FixedPrompt::new("p")))
        .with_scope(Arc::new(MockScope::new("s")))
        .with_clock(Arc::new(FixedClock::new(100)))
        .build()
        .map_err(|e| e.to_string())?;

    let cancel = Arc::new(ManualCancel::new());
    let cancel_for_task = cancel.clone();

    let req = TurnRequest::new(
        "thread_cancel_running",
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
        },
    );

    let sink = RecordingSink::new();
    let turn = tokio::spawn(async move {
        let outcome = rt.run_turn(req, &sink, &*cancel_for_task).await;
        (outcome, sink)
    });

    // 等工具真的开始执行，再按"停止键"
    tokio::time::sleep(Duration::from_millis(150)).await;
    cancel.cancel();

    let (outcome, sink) = tokio::time::timeout(Duration::from_secs(10), turn)
        .await
        .map_err(|_| "执行中取消后轮次没有及时收尾（取消没有穿透）".to_string())?
        .map_err(|e| format!("任务 panic: {e}"))?;

    let outcome = outcome.map_err(|e| e.to_string())?;
    if !observed.load(Ordering::SeqCst) {
        return Err("工具没有观察到取消——取消没有穿透到 Tool::execute（P0-4 的症状）".into());
    }
    if outcome.stop_reason != TurnStopReason::Aborted {
        return Err(format!("取消后停机原因应为 Aborted，实际 {:?}", outcome.stop_reason));
    }

    // 回执也必须如实：工具以 Aborted 收尾，而不是 Error
    let aborted_receipt = sink.snapshot().iter().any(|e| {
        matches!(
            &e.body,
            agent_base::domain::AgentEventBody::ToolCallFinished { receipt, .. }
                if receipt.status == ToolStatus::Aborted
        )
    });
    if !aborted_receipt {
        return Err("工具回执状态应为 Aborted（结构化回执必须如实反映取消）".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inv8_cancellation_penetration() {
        assert_cancellation_penetrates_turn().await.expect("取消贯穿测试必须通过");
    }

    #[tokio::test]
    async fn test_inv8_cancellation_penetrates_running_tool() {
        assert_cancellation_penetrates_running_tool()
            .await
            .expect("执行中取消必须穿透到工具（W4-T4）");
    }
}
