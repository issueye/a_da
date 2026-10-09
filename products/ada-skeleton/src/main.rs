//! ada-skeleton: 极简骨架产品（验收基座跨产品复用能力与零内核改动，M5-T3）
//!
//! W3-T1 起，本产品**不再自己拼端口**——它只做三件事：
//! 1. 读自己的 `agent.spec.json`；
//! 2. 交给宿主装配层 [`agent_host::run_from_spec`]；
//! 3. 跑一轮。
//!
//! 这正是"产品是薄声明"的形态：端口选择、工具装配、事件出口全在 `agent-host`，
//! 产品代码里看不到任何一个端口类型。

use std::sync::Arc;

use agent_base::model::{ProviderConfig, StreamDelta};
use agent_base::testing::ScriptedModelClient;
use agent_host::{run_from_spec, HostOptions};
use agent_runtime::AgentSpec;
use clap::Parser;

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

    // 2. 装配：宿主层按声明装工具包、接真实端口。
    //    骨架是**演示产品**，没有真实 API key，所以只注入脚本化模型；
    //    其余端口全部是生产实现（WorkspaceScope / FsSessionStore / CodingPromptSource /
    //    SystemClock / HostApprovalGate / WsEventSink）。
    let model = Arc::new(ScriptedModelClient::new(vec![vec![
        StreamDelta::Text { text: "你好！我是 ada-skeleton 极简骨架助手。".into() },
        StreamDelta::Done { stop_reason: "stop".into() },
    ]]));

    let hosted = run_from_spec(
        spec,
        HostOptions::new(std::env::current_dir()?)
            // 演示产品不往工作区里写会话，落到临时目录
            .with_sessions_root(std::env::temp_dir().join("ada-skeleton-sessions"))
            .with_model(model),
    )?;

    println!("ada-skeleton 宿主装配成功！包含工具: {:?}", hosted.tool_names());

    // 3. 跑一轮
    if args.demo {
        let outcome = hosted.run_turn("skeleton_demo_thread", provider_config()).await?;
        println!(
            "演示运行完成，执行步数: {}, 产生事件数: {}",
            outcome.steps_taken,
            hosted.events.snapshot().len()
        );
    }

    Ok(())
}

fn provider_config() -> ProviderConfig {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AgentSpec {
        AgentSpec::from_json_str(SPEC_JSON).expect("spec 解析合法")
    }

    fn scripted() -> Arc<dyn agent_base::ports::ModelClient> {
        Arc::new(ScriptedModelClient::new(vec![vec![
            StreamDelta::Text { text: "骨架运转正常".into() },
            StreamDelta::Done { stop_reason: "stop".into() },
        ]]))
    }

    fn host_options() -> HostOptions {
        HostOptions::new(std::env::current_dir().expect("取当前目录失败"))
            .with_sessions_root(std::env::temp_dir().join("ada-skeleton-test-sessions"))
            .with_model(scripted())
    }

    #[test]
    fn test_skeleton_spec_is_valid() {
        let s = spec();
        assert_eq!(s.id, "ada-skeleton");
        assert_eq!(s.toolkits, vec!["core"]);
    }

    /// W3-T1 守门：产品走**宿主装配层**（`agent-host`），不再自己拼端口。
    #[tokio::test]
    async fn test_skeleton_runs_through_host_layer() {
        let hosted = run_from_spec(spec(), host_options()).expect("宿主装配必须成功");

        // W2-T1 守门：`core` 工具包的工具必须真的进了 catalog（声明 → 装配 → catalog）
        let names = hosted.tool_names();
        for expect in ["ask_user", "todo", "finish"] {
            assert!(
                names.contains(&expect),
                "`core` 工具包应提供 `{expect}`，实际: {names:?}"
            );
        }

        let outcome = hosted
            .run_turn("test_thread", provider_config())
            .await
            .expect("执行成功");
        assert_eq!(outcome.steps_taken, 1);
        assert!(!hosted.events.snapshot().is_empty(), "事件出口必须有内容");
        assert!(hosted.events.seq_violations().is_empty(), "事件 seq 不得违约");
    }

    #[tokio::test]
    async fn test_skeleton_conformance_passes() {
        agent_conformance::assert_single_engine_executes_turn(SPEC_JSON)
            .await
            .expect("骨架单一引擎合规");
    }
}
