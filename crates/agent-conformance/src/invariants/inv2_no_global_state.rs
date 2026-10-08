//! INV-8: 无隐藏全局态不变量
//! 同进程可跑两个 runtime 且互不干扰，时钟、会话存储、审批等均通过组合根注入。

use std::sync::Arc;
use agent_base::engine::TurnRequest;
use agent_base::model::{ProviderConfig, StreamDelta};
use agent_base::testing::{FixedClock, FixedPrompt, InMemorySessionStore, MockScope, NeverCancel, RecordingApprovalGate, RecordingSink, ScriptedModelClient};
use agent_runtime::{AgentSpec, ProductBuilder};
use agent_toolkit::core::FinishTool;

/// 断言同进程内两个独立 runtime 互不干扰
pub async fn assert_isolated_runtimes_in_same_process() -> Result<(), String> {
    let spec1 = AgentSpec::from_json_str(r#"{
        "id": "runtime_1",
        "archetype": "assistant",
        "identity": { "name": "助手1", "persona": "persona1", "locale": "zh-CN" },
        "toolkits": ["core"],
        "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
        "policies": { "maxSteps": 5, "parallelTools": 1, "toolTimeoutSec": 30 }
    }"#).map_err(|e| e.to_string())?;

    let spec2 = AgentSpec::from_json_str(r#"{
        "id": "runtime_2",
        "archetype": "coding",
        "identity": { "name": "助手2", "persona": "persona2", "locale": "zh-CN" },
        "toolkits": ["core"],
        "capabilities": { "images": false, "streaming": true, "rollback": false, "subagents": false },
        "policies": { "maxSteps": 5, "parallelTools": 1, "toolTimeoutSec": 30 }
    }"#).map_err(|e| e.to_string())?;

    // 分别注入独立的依赖
    let store1 = Arc::new(InMemorySessionStore::new());
    let store2 = Arc::new(InMemorySessionStore::new());

    let clock1 = Arc::new(FixedClock::new(1000));
    let clock2 = Arc::new(FixedClock::new(2000));

    let rt1 = ProductBuilder::new(spec1)
        .with_tool(Arc::new(FinishTool::default()))
        .with_model(Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "我是1".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]])))
        .with_approval(Arc::new(RecordingApprovalGate::new(true)))
        .with_store(store1.clone())
        .with_prompt(Arc::new(FixedPrompt::new("p1")))
        .with_scope(Arc::new(MockScope::new("s1")))
        .with_clock(clock1.clone())
        .build().map_err(|e| e.to_string())?;

    let rt2 = ProductBuilder::new(spec2)
        .with_tool(Arc::new(FinishTool::default()))
        .with_model(Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "我是2".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]])))
        .with_approval(Arc::new(RecordingApprovalGate::new(true)))
        .with_store(store2.clone())
        .with_prompt(Arc::new(FixedPrompt::new("p2")))
        .with_scope(Arc::new(MockScope::new("s2")))
        .with_clock(clock2.clone())
        .build().map_err(|e| e.to_string())?;

    let sink1 = RecordingSink::new();
    let sink2 = RecordingSink::new();

    let pcfg = ProviderConfig {
        id: "p".into(),
        name: "p".into(),
        protocol: Default::default(),
        base_url: "http://localhost".into(),
        api_key: "k".into(),
        model: "m".into(),
        max_output_tokens: None,
        custom_headers: None,
        proxy_url: None,
    };

    let (res1, res2) = tokio::join!(
        rt1.run_turn(TurnRequest::new("thread_rt1", pcfg.clone()), &sink1, &NeverCancel),
        rt2.run_turn(TurnRequest::new("thread_rt2", pcfg), &sink2, &NeverCancel)
    );

    res1.map_err(|e| e.to_string())?;
    res2.map_err(|e| e.to_string())?;

    // 状态完全隔离：store1 只有 thread_rt1，store2 只有 thread_rt2
    assert_eq!(store1.get("thread_rt1").len(), 1);
    assert_eq!(store1.get("thread_rt2").len(), 0);
    assert_eq!(store2.get("thread_rt2").len(), 1);
    assert_eq!(store2.get("thread_rt1").len(), 0);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inv2_no_global_state() {
        assert_isolated_runtimes_in_same_process().await.expect("多实例隔离测试必须通过");
    }
}
