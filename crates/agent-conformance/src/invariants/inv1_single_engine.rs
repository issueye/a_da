//! INV-1: 单一引擎不变量
//! 仓库内只有 agent-base 一份多轮循环（run_turn），产品不得自带多轮循环。

use std::sync::Arc;
use agent_base::engine::TurnRequest;
use agent_base::model::{ProviderConfig, StreamDelta};
use agent_base::testing::{FixedClock, FixedPrompt, InMemorySessionStore, MockScope, NeverCancel, RecordingApprovalGate, RecordingSink, ScriptedModelClient};
use agent_runtime::{AgentSpec, ProductBuilder};
use agent_toolkit::core::FinishTool;

/// 断言单一引擎能驱动产品规格完成单轮
pub async fn assert_single_engine_executes_turn(spec_json: &str) -> Result<(), String> {
    let spec = AgentSpec::from_json_str(spec_json).map_err(|e| e.to_string())?;

    let model = Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::Text { text: "INV-1 单一引擎运行正常".into() },
        StreamDelta::Done { stop_reason: "stop".into() },
    ]]));

    let builder = ProductBuilder::new(spec)
        .with_tool(Arc::new(FinishTool::default()))
        .with_model(model)
        .with_approval(Arc::new(RecordingApprovalGate::new(true)))
        .with_store(Arc::new(InMemorySessionStore::new()))
        .with_prompt(Arc::new(FixedPrompt::new("系统提示词")))
        .with_scope(Arc::new(MockScope::new("test_scope")))
        .with_clock(Arc::new(FixedClock::new(1000)));

    let runtime = builder.build().map_err(|e| e.to_string())?;
    let sink = RecordingSink::new();
    let cancel = NeverCancel;
    let req = TurnRequest::new(
        "test_thread",
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

    let outcome = runtime.run_turn(req, &sink, &cancel).await.map_err(|e| e.to_string())?;
    if outcome.steps_taken == 0 {
        return Err("引擎未能步进".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inv1_single_engine() {
        let minimal_spec = r#"{
            "id": "ada-test-engine",
            "archetype": "assistant",
            "identity": { "name": "测试", "persona": "persona", "locale": "zh-CN" },
            "toolkits": ["core"],
            "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
            "policies": { "maxSteps": 10, "parallelTools": 1, "toolTimeoutSec": 30 }
        }"#;

        assert_single_engine_executes_turn(minimal_spec).await.expect("单一引擎断言必须通过");
    }
}
