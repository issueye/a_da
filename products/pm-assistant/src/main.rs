//! **项目管理助手（PM agent）**：把目标拆解后**经网关**委派给 coding agent。
//!
//! # 它与 coding agent 的关系
//!
//! ```text
//! 用户 ──► PM agent（本产品）
//!             │  invoke_subagent（AgentBus 端口）
//!             ▼
//!          GatewayAgentBus ──gateway.delegate──► 网关 ──► coding agent（ada-coding）
//! ```
//!
//! 它的"能力"几乎全在**声明**里：
//!
//! | 声明 | 效果 |
//! |---|---|
//! | `capabilities.delegation = "gateway"` | 组合根装配 `GatewayAgentBus`（而不是进程内本地总线） |
//! | `gateway.endpoint` | 网关在哪 |
//! | `capabilities.subagents = true` | 装配 `invoke_subagent` 工具 |
//! | `toolkits` | 它自己用什么工具（不亲自改代码，所以没有 `fs` 写工具包） |
//!
//! 本文件**看不到任何端口类型**——这是"产品是薄声明"的形态。
//!
//! # 为什么没有 `fs` 工具包
//!
//! PM agent 的价值是**拆解、分派、跟进、汇总**，不是改代码。
//! 给它文件写权限会让"PM 直接动手改代码"成为可能——那是**角色边界**的问题，
//! 不是权限配置问题。要改代码就委派给 coding agent。

use std::sync::Arc;

use agent_base::model::ProviderConfig;
use agent_host::{run_from_spec, HostOptions};
use agent_runtime::AgentSpec;
use clap::Parser;

const SPEC_JSON: &str = include_str!("../agent.spec.json");

#[derive(Parser, Debug)]
#[command(author, version, about = "a_da 项目管理助手（PM agent）", long_about = None)]
struct CliArgs {
    /// 装配并跑一轮演示（不联网：用脚本化模型）
    #[arg(long, default_value_t = false)]
    demo: bool,
    /// 覆盖声明里的网关端点
    #[arg(long)]
    gateway: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = CliArgs::parse();

    let mut spec = AgentSpec::from_json_str(SPEC_JSON)?;
    if let Some(endpoint) = args.gateway {
        spec.gateway = Some(agent_runtime::GatewaySpec { endpoint });
    }

    println!("加载产品规格: {} ({})", spec.identity.name, spec.id);
    println!(
        "委派模式: {:?} → {}",
        spec.capabilities.delegation,
        spec.gateway
            .as_ref()
            .map(|g| g.endpoint.as_str())
            .unwrap_or("（未声明网关端点）")
    );

    let hosted = run_from_spec(
        spec,
        HostOptions::new(std::env::current_dir()?)
            .with_sessions_root(std::env::temp_dir().join("pm-assistant-sessions"))
            // 演示不联网：脚本化模型。真实运行时由宿主注入网络模型。
            .with_model(Arc::new(agent_base::testing::ScriptedModelClient::new(vec![vec![
                agent_base::model::StreamDelta::Text {
                    text: "我先看一下有哪些可用的 coding agent。".into(),
                },
                agent_base::model::StreamDelta::Done {
                    stop_reason: "stop".into(),
                },
            ]])))
    )?;

    println!("PM agent 装配成功！工具: {:?}", hosted.tool_names());

    if args.demo {
        let outcome = hosted
            .run_turn("pm_demo_thread", provider_config())
            .await?;
        println!(
            "演示运行完成：步数 {}，事件 {}",
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
    use agent_runtime::DelegationMode;

    fn spec() -> AgentSpec {
        AgentSpec::from_json_str(SPEC_JSON).expect("spec 解析合法")
    }

    fn options() -> HostOptions {
        HostOptions::new(std::env::current_dir().expect("取当前目录失败"))
            .with_sessions_root(std::env::temp_dir().join("pm-assistant-test-sessions"))
            .with_model(Arc::new(agent_base::testing::ScriptedModelClient::new(vec![vec![
                agent_base::model::StreamDelta::Done {
                    stop_reason: "stop".into(),
                },
            ]])))
    }

    /// 声明必须是"经网关委派"——否则这个产品就退化成另一个进程内子智能体壳。
    #[test]
    fn test_spec_declares_gateway_delegation() {
        let s = spec();
        assert_eq!(s.id, "pm-assistant");
        assert_eq!(s.capabilities.delegation, DelegationMode::Gateway);
        assert!(s.capabilities.subagents, "要委派就必须有委派工具");
        assert!(
            s.gateway.as_ref().is_some_and(|g| !g.endpoint.trim().is_empty()),
            "声明了网关委派就必须给出端点"
        );
    }

    /// PM 不亲自改代码：`fs` 写工具包不得出现在声明里（角色边界，不是权限配置）。
    #[test]
    fn test_pm_does_not_declare_file_write_toolkits() {
        let s = spec();
        for banned in ["fs", "patch", "command"] {
            assert!(
                !s.toolkits.iter().any(|t| t.as_str() == banned),
                "PM agent 不该声明 `{banned}` 工具包——它的价值是拆解与分派，不是改代码"
            );
        }
    }

    /// 装配必须成功，且拿到委派工具（声明 → 装配 → catalog 这条链真的通了）。
    #[test]
    fn test_pm_assembles_with_delegation_tool() {
        let hosted = run_from_spec(spec(), options()).expect("装配必须成功");
        let names = hosted.tool_names();
        assert!(
            names.iter().any(|n| *n == "invoke_subagent"),
            "声明了 capabilities.subagents=true 就必须有委派工具，实际: {names:?}"
        );
    }

    /// 声明了网关委派却**没给端点** → 装配失败，**不回退到本地**。
    ///
    /// 回退会静默改变行为：产品说"我要跨节点协作"，实际退化成进程内临时子智能体。
    /// 宁可起不来，也不要"看起来装上了"。
    #[test]
    fn test_gateway_delegation_without_endpoint_fails_loudly() {
        let mut s = spec();
        s.gateway = None;
        let msg = match run_from_spec(s, options()) {
            Ok(_) => panic!("缺端点必须装配失败"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("gateway.endpoint"),
            "错误信息要点出缺的是哪个字段：{msg}"
        );
    }

    /// 单引擎合规：PM 产品同样跑 `AgentRuntime::run_turn`（INV-1 跨产品成立）。
    #[tokio::test]
    async fn test_pm_conformance_passes() {
        agent_conformance::assert_single_engine_executes_turn(SPEC_JSON)
            .await
            .expect("PM 产品单一引擎合规");
    }
}
