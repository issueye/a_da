use agent_proto::PROTOCOL_VERSION;
use agent_rpc::server::{start_parent_watchdog, WsHostServer};
use agent_rpc::state::AgentStore;
use clap::Parser;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

/// 产品声明（W3-T2：宿主按它装配真引擎）。
const SPEC_JSON: &str = include_str!("../agent.spec.json");

#[derive(Parser, Debug)]
#[command(author, version, about = "a_da 原生 Agent 核心服务 (Rust)", long_about = None)]
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
}

/// 按产品声明装配真引擎；任何失败都**如实降级**为 legacy（返回 `None`）。
///
/// 注意三件事都是刻意的：
/// 1. **复用宿主的 `store`**：引擎的审批闸门要读 `config.approval`，各造一份会出现
///    "界面上是只读档、引擎按默认档跑"；
/// 2. **会话落盘到 app home**（不是工作区），与 legacy 路径一致；
/// 3. 返回的 `approval_mgr` 会被 ws 宿主用于构造 `Dispatcher`——**必须是同一个实例**，
///    否则 UI 的批准送不到闸门。
fn build_engine_injection(
    store: &Arc<RwLock<AgentStore>>,
    workspace: &str,
) -> Option<agent_rpc::server::dispatch::EngineInjection> {
    let ws = if workspace.trim().is_empty() {
        match std::env::current_dir() {
            Ok(p) => p,
            Err(e) => {
                warn!("取当前目录失败，降级为 legacy 引擎：{e}");
                return None;
            }
        }
    } else {
        std::path::PathBuf::from(workspace)
    };

    let spec = match agent_runtime::AgentSpec::from_json_str(SPEC_JSON) {
        Ok(s) => s,
        Err(e) => {
            warn!("产品规格解析失败，降级为 legacy 引擎：{e}");
            return None;
        }
    };

    let sessions_root =
        std::path::Path::new(&agent_node::session::get_app_home()).join("sessions");
    let options = agent_host::HostOptions::new(ws)
        .with_store(store.clone())
        .with_sessions_root(sessions_root);

    match agent_host::run_from_spec(spec, options) {
        Ok(hosted) => {
            let tool_count = hosted.tool_names().len();
            let agent_host::HostedProduct { runtime, approval, spec, .. } = hosted;
            info!("已按产品声明装配真引擎：工具 {tool_count} 个");
            Some(agent_rpc::server::dispatch::EngineInjection {
                runtime: Arc::new(runtime),
                approval_mgr: approval,
                // W3-T6：声明一起注入，握手才能如实回报能力位
                spec: Arc::new(spec),
            })
        }
        Err(e) => {
            warn!("真引擎装配失败：{e}");
            None
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let app_home = agent_node::session::get_app_home();
    let log_file_path = std::path::Path::new(&app_home).join("host.log");
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

    let token = match args.token {
        Some(t) if !t.trim().is_empty() => t,
        _ => uuid::Uuid::new_v4().simple().to_string(),
    };

    // 启动父进程看门狗
    if let Some(parent_pid) = args.parent_pid {
        start_parent_watchdog(parent_pid, 2000);
    }

    let store = Arc::new(RwLock::new(AgentStore::new(args.workspace.clone())));

    // W3-T2：按产品声明装配**真引擎**并注入宿主。
    //
    // 走哪条引擎由 `A_DA_ENGINE` 决定（默认 `legacy` → 行为与切换前完全一致，
    // 保证可回滚）；设 `A_DA_ENGINE=runtime` 才真的切到 `AgentRuntime::run_turn`。
    // 装配失败**不致命**：如实打日志并降级为 legacy，而不是让宿主起不来。
    let injection = build_engine_injection(&store, &args.workspace);

    // 启动 WebSocket 服务
    let server = WsHostServer::bind_with_engine(args.port, token, store, injection).await?;
    let current_pid = std::process::id();

    // 打印符合协议 §1.8 规范的标准就绪行至 stdout
    let ready_json = serde_json::json!({
        "ready": true,
        "port": server.port,
        "pid": current_pid,
        "protocolVersion": PROTOCOL_VERSION,
    });

    println!("A_DA_HOST_READY {}", ready_json);
    std::io::stdout().flush()?;

    info!("a-da 原生核心就绪，PID: {}, 监听端口: {}", current_pid, server.port);

    // 持续运行服务直到收到终止信号或父进程看门狗触发退出
    #[cfg(target_os = "windows")]
    {
        match tokio::signal::windows::ctrl_c() {
            Ok(mut sig) => {
                tokio::select! {
                    Some(_) = sig.recv() => {
                        info!("收到 Ctrl+C 退出信号，a-da 原生核心安全关闭");
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
        info!("收到退出信号，a-da 原生核心安全关闭");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    const SPEC_JSON: &str = include_str!("../agent.spec.json");

    #[test]
    fn test_ada_coding_spec_is_valid() {
        let spec = agent_runtime::AgentSpec::from_json_str(SPEC_JSON).expect("spec 必须合法");
        assert_eq!(spec.id, "ada-coding");
        assert_eq!(spec.archetype, "coding");
        assert!(spec.capabilities.rollback);
        assert!(spec.capabilities.plugins);
    }

    /// W2-T2 守门：产品**声明的工具包**必须能真的装配出工具，且与注册表逐名对齐。
    ///
    /// 这条把"声明 → 工具包真源 → catalog"整条链路钉在产品自己的测试里：
    /// 声明了一个不存在的工具包（例如曾经的 `patch`）会在这里直接红。
    #[test]
    fn test_ada_coding_declared_toolkits_assemble() {
        use std::collections::BTreeSet;

        let spec = agent_runtime::AgentSpec::from_json_str(SPEC_JSON).expect("spec 必须合法");
        let tools = agent_toolkit::tools_for_toolkits(&spec.toolkits, std::path::Path::new("."))
            .expect("声明的工具包必须都能装配（未知名/重名都会 Err）");

        let assembled: BTreeSet<String> =
            tools.iter().map(|t| t.descriptor().name.clone()).collect();
        let registry: BTreeSet<String> = agent_toolkit::standard_tool_descriptors()
            .iter()
            .map(|d| d.name.clone())
            .collect();

        // 工具包只能提供注册表里的工具（不许造出注册表没有的名字）
        assert!(
            assembled.is_subset(&registry),
            "工具包提供了注册表里没有的工具：{:?}",
            assembled.difference(&registry).collect::<Vec<_>>()
        );

        // W4-T5：唯一**不由工具包**提供的是宿主注入的委派工具
        // （它需要 SubagentManager/父 provider/检查点，工具包工厂构造不出来）。
        let host_provided: BTreeSet<String> =
            registry.difference(&assembled).cloned().collect();
        assert_eq!(
            host_provided,
            BTreeSet::from(["invoke_subagent".to_string()]),
            "除 invoke_subagent 外不应有其它工具指望宿主注入"
        );
        assert!(
            spec.capabilities.subagents,
            "ada-coding 声明支持子智能体 → 宿主会注入 invoke_subagent；\
             若这里为 false，注册表里的 invoke_subagent 就会成为不可达工具"
        );

        assert!(
            assembled.contains("git_status") && assembled.contains("project_inspect"),
            "W2-T2 的插件工具应已进 catalog：{assembled:?}"
        );
    }

    #[tokio::test]
    async fn test_ada_coding_conformance() {
        agent_conformance::assert_single_engine_executes_turn(SPEC_JSON)
            .await
            .expect("ada-coding 产品单一引擎合规性校验通过");
    }
}
