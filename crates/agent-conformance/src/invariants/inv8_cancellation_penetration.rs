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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inv8_cancellation_penetration() {
        assert_cancellation_penetrates_turn().await.expect("取消贯穿测试必须通过");
    }
}
