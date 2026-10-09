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

use std::io::Write;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

use agent_base::model::ProviderConfig;
use agent_host::{run_from_spec, HostOptions};
use agent_proto::PROTOCOL_VERSION;
use agent_rpc::server::{start_parent_watchdog, WsHostServer};
use agent_rpc::state::AgentStore;
use agent_runtime::AgentSpec;
use clap::Parser;

const SPEC_JSON: &str = include_str!("../agent.spec.json");

#[derive(Parser, Debug)]
#[command(author, version, about = "a_da 项目管理助手（PM agent）", long_about = None)]
struct CliArgs {
    /// 启用主机模式
    #[arg(long, default_value_t = false)]
    host: bool,

    /// 监听端口（0 表示系统自动分配空闲端口）
    #[arg(long, default_value_t = 0)]
    port: u16,

    /// 握手认证令牌（与 UI 进程共享）
    #[arg(long)]
    token: Option<String>,

    /// 父进程 PID（提供后将启动看门狗自动防孤儿退出）
    #[arg(long)]
    parent_pid: Option<u32>,

    /// 工作区默认路径
    #[arg(long, default_value = "")]
    workspace: String,

    /// 覆盖声明里的网关端点，并于启动后自动向网关注册
    #[arg(long)]
    gateway: Option<String>,

    /// 装配并跑一轮演示（不联网：用脚本化模型）
    #[arg(long, default_value_t = false)]
    demo: bool,
}

/// 按 PM 产品声明装配真引擎；失败返回 None。
fn build_pm_engine_injection(
    store: &Arc<RwLock<AgentStore>>,
    workspace: &str,
    gateway_override: Option<&str>,
) -> Option<agent_rpc::server::dispatch::EngineInjection> {
    let mut spec = match AgentSpec::from_json_str(SPEC_JSON) {
        Ok(s) => s,
        Err(e) => {
            warn!("解析 PM 规格失败：{e}");
            return None;
        }
    };

    if let Some(endpoint) = gateway_override {
        spec.gateway = Some(agent_runtime::GatewaySpec {
            endpoint: endpoint.to_string(),
        });
    }

    let ws = if workspace.trim().is_empty() {
        match std::env::current_dir() {
            Ok(p) => p,
            Err(e) => {
                warn!("取当前目录失败：{e}");
                return None;
            }
        }
    } else {
        std::path::PathBuf::from(workspace)
    };

    let sessions_root =
        std::path::Path::new(&agent_node::session::get_app_home()).join("pm_sessions");
    let options = HostOptions::new(ws)
        .with_store(store.clone())
        .with_sessions_root(sessions_root);

    match run_from_spec(spec, options) {
        Ok(hosted) => {
            let tool_count = hosted.tool_names().len();
            let agent_host::HostedProduct {
                runtime,
                approval,
                spec,
                ..
            } = hosted;
            info!("已按 PM 产品声明装配真引擎：工具 {tool_count} 个");
            Some(agent_rpc::server::dispatch::EngineInjection {
                runtime: Arc::new(runtime),
                approval_mgr: approval,
                spec: Arc::new(spec),
            })
        }
        Err(e) => {
            warn!("PM 真引擎装配失败：{e}");
            None
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let app_home = agent_node::session::get_app_home();
    let log_file_path = std::path::Path::new(&app_home).join("pm_host.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path);

    if let Ok(file) = log_file {
        let subscriber = FmtSubscriber::builder()
            .with_max_level(Level::INFO)
            .with_target(false)
            .with_writer(file)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    } else {
        let subscriber = FmtSubscriber::builder()
            .with_max_level(Level::INFO)
            .with_target(false)
            .with_writer(std::io::sink)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    }

    let args = CliArgs::parse();

    // 1. 命令行演示模式
    if args.demo {
        let mut spec = AgentSpec::from_json_str(SPEC_JSON)?;
        if let Some(endpoint) = args.gateway.clone() {
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
                .with_model(Arc::new(agent_base::testing::ScriptedModelClient::new(vec![vec![
                    agent_base::model::StreamDelta::Text {
                        text: "我先看一下有哪些可用的 coding agent。".into(),
                    },
                    agent_base::model::StreamDelta::Done {
                        stop_reason: "stop".into(),
                    },
                ]]))),
        )?;

        println!("PM agent 装配成功！工具: {:?}", hosted.tool_names());
        let outcome = hosted
            .run_turn("pm_demo_thread", provider_config())
            .await?;
        println!(
            "演示运行完成：步数 {}，事件 {}",
            outcome.steps_taken,
            hosted.events.snapshot().len()
        );
        return Ok(());
    }

    // 2. 主机服务模式 (Host Server)
    let token = match args.token {
        Some(t) if !t.trim().is_empty() => t,
        _ => uuid::Uuid::new_v4().simple().to_string(),
    };

    // 启动父进程看门狗
    if let Some(parent_pid) = args.parent_pid {
        start_parent_watchdog(parent_pid, 2000);
    }

    let store = Arc::new(RwLock::new(AgentStore::new(args.workspace.clone())));

    // 装配 PM 真引擎
    let injection = build_pm_engine_injection(&store, &args.workspace, args.gateway.as_deref());

    // 启动 WebSocket 服务
    let server = WsHostServer::bind_with_engine(
        args.port,
        token.clone(),
        store,
        injection,
    )
    .await?;
    let current_pid = std::process::id();

    // 打印符合协议 §1.8 规范的标准就绪行至 stdout
    let ready_json = serde_json::json!({
        "ready": true,
        "port": server.port,
        "token": token,
        "pid": current_pid,
        "product": "pm-assistant",
        "protocolVersion": PROTOCOL_VERSION,
    });

    println!("A_DA_HOST_READY {}", ready_json);
    std::io::stdout().flush()?;

    info!("pm-assistant 核心就绪，PID: {}, 监听端口: {}", current_pid, server.port);

    // 若配置了 --gateway，启动后台协程主动向网关注册自身并保持长连接
    if let Some(gateway_endpoint) = args.gateway {
        let server_port = server.port;
        let auth_token = token.clone();
        let ws_path = args.workspace.clone();
        tokio::spawn(async move {
            register_to_gateway("pm-assistant", &gateway_endpoint, server_port, &auth_token, &ws_path, current_pid).await;
        });
    }

    // 持续运行服务直到收到终止信号或父进程看门狗触发退出
    #[cfg(target_os = "windows")]
    {
        match tokio::signal::windows::ctrl_c() {
            Ok(mut sig) => {
                tokio::select! {
                    Some(_) = sig.recv() => {
                        info!("收到 Ctrl+C 退出信号，pm-assistant 安全关闭");
                    }
                    _ = std::future::pending::<()>() => {}
                }
            }
            Err(e) => {
                info!("当前环境未附加控制台，由看门狗看护退出: {}", e);
                std::future::pending::<()>().await;
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = tokio::signal::ctrl_c().await;
        info!("收到退出信号，pm-assistant 安全关闭");
    }

    Ok(())
}

/// 主动连接网关并完成自主注册。
async fn register_to_gateway(
    product: &str,
    gateway_url: &str,
    server_port: u16,
    token: &str,
    workspace: &str,
    pid: u32,
) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let ws = if workspace.trim().is_empty() {
        std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        workspace.to_string()
    };

    let mut connect_url = gateway_url.trim().to_string();
    let delim = if connect_url.contains('?') { "&" } else { "?" };
    connect_url.push_str(&format!("{delim}role=agent"));

    info!("正在连接网关进行自注册: {}", connect_url);

    let mut stream = None;
    for attempt in 1..=30 {
        match connect_url.clone().into_client_request() {
            Ok(req) => match tokio_tungstenite::connect_async(req).await {
                Ok((ws_stream, _)) => {
                    stream = Some(ws_stream);
                    break;
                }
                Err(e) => {
                    warn!("连接网关失败（尝试 {attempt}/30）: {e}");
                }
            },
            Err(e) => {
                warn!("构造网关连接请求失败: {e}");
                return;
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    }

    let Some(mut ws_stream) = stream else {
        warn!("未能连上网关服务，放弃自注册");
        return;
    };

    let endpoint = format!("ws://127.0.0.1:{server_port}/rpc?token={token}");
    let register_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "gateway.register",
        "params": {
            "product": product,
            "workspace": ws,
            "endpoint": endpoint,
            "pid": pid,
        }
    });

    if let Err(e) = ws_stream.send(tokio_tungstenite::tungstenite::Message::Text(register_req.to_string().into())).await {
        warn!("发送 gateway.register 失败: {e}");
        return;
    }

    if let Some(Ok(resp)) = ws_stream.next().await {
        info!("网关注册响应: {resp}");
    }

    info!("{} 已成功向网关自注册，保持长连接通道...", product);
    while let Some(msg) = ws_stream.next().await {
        if let Ok(m) = msg {
            if m.is_close() {
                warn!("网关控制连接已关闭");
                break;
            }
        } else {
            break;
        }
    }
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
