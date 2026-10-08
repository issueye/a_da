//! ada-skeleton: 极简骨架产品（验收基座跨产品复用能力与零内核改动，M5-T3）

use std::sync::Arc;
use clap::Parser;
use agent_runtime::{AgentSpec, ProductBuilder};
use agent_base::testing::{FixedClock, FixedPrompt, InMemorySessionStore, MockScope, NeverCancel, RecordingApprovalGate, RecordingSink, ScriptedModelClient};
use agent_base::engine::TurnRequest;
use agent_base::model::{ProviderConfig, StreamDelta};
use agent_toolkit::core::FinishTool;

const SPEC_JSON: &str = include_str!("../agent.spec.json");

#[derive(Parser, Debug)]
#[command(author, version, about = "a_da 骨架助手 (跨产品复用验证产品)", long_about = None)]
struct CliArgs {
    /// 执行一轮单测交互演示
    #[arg(long, default_value_t = false)]
    demo: bool,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args = CliArgs::parse();

    // 1. 读取并校验规格
    let spec = AgentSpec::from_json_str(SPEC_JSON)?;
    println!("成功加载产品规格: {} ({})", spec.identity.name, spec.id);

    // 2. 装配骨架运行时
    let model = Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::Text { text: "你好！我是 ada-skeleton 极简骨架助手。".into() },
        StreamDelta::Done { stop_reason: "stop".into() },
    ]]));

    let builder = ProductBuilder::new(spec)
        .with_tool(Arc::new(FinishTool::default()))
        .with_model(model)
        .with_approval(Arc::new(RecordingApprovalGate::new(true)))
        .with_store(Arc::new(InMemorySessionStore::new()))
        .with_prompt(Arc::new(FixedPrompt::new("极简助手人格")))
        .with_scope(Arc::new(MockScope::new("skeleton_scope")))
        .with_clock(Arc::new(FixedClock::new(1700000000000)));

    let runtime = builder.build()?;
    let tool_names: Vec<String> = runtime.tools.descriptors().iter().map(|d| d.name.clone()).collect();
    println!("ada-skeleton 运行时装配成功！包含工具: {:?}", tool_names);

    if args.demo {
        let sink = RecordingSink::new();
        let cancel = NeverCancel;
        let req = TurnRequest::new(
            "skeleton_demo_thread",
            ProviderConfig {
                id: "local".into(),
                name: "local".into(),
                protocol: Default::default(),
                base_url: "http://localhost".into(),
                api_key: "k".into(),
                model: "m".into(),
                max_output_tokens: None,
                custom_headers: None,
                proxy_url: None,
            },
        );

        let outcome = runtime.run_turn(req, &sink, &cancel).await?;
        println!("演示运行完成，执行步数: {}, 产生事件数: {}", outcome.steps_taken, sink.snapshot().len());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skeleton_spec_is_valid() {
        let spec = AgentSpec::from_json_str(SPEC_JSON).expect("spec 解析合法");
        assert_eq!(spec.id, "ada-skeleton");
        assert_eq!(spec.toolkits, vec!["core"]);
    }

    #[tokio::test]
    async fn test_skeleton_runtime_executes_turn() {
        let spec = AgentSpec::from_json_str(SPEC_JSON).expect("spec 解析合法");
        let model = Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "骨架运转正常".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]]));

        let builder = ProductBuilder::new(spec)
            .with_tool(Arc::new(FinishTool::default()))
            .with_model(model)
            .with_approval(Arc::new(RecordingApprovalGate::new(true)))
            .with_store(Arc::new(InMemorySessionStore::new()))
            .with_prompt(Arc::new(FixedPrompt::new("测试")))
            .with_scope(Arc::new(MockScope::new("scope")))
            .with_clock(Arc::new(FixedClock::new(100)));

        let runtime = builder.build().expect("装配必须成功");
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

        let outcome = runtime.run_turn(req, &sink, &cancel).await.expect("执行成功");
        assert_eq!(outcome.steps_taken, 1);
        assert!(!sink.snapshot().is_empty());
    }

    #[tokio::test]
    async fn test_skeleton_conformance_passes() {
        agent_conformance::assert_single_engine_executes_turn(SPEC_JSON)
            .await
            .expect("骨架单一引擎合规");
    }
}
