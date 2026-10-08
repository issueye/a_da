use agent_core::protocol::PROTOCOL_VERSION;
use agent_core::server::{start_parent_watchdog, WsHostServer};
use agent_core::state::AgentStore;
use clap::Parser;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

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

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let app_home = agent_core::session::get_app_home();
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

    let store = Arc::new(RwLock::new(AgentStore::new(args.workspace)));

    // 启动 WebSocket 服务
    let server = WsHostServer::bind(args.port, token, store).await?;
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

    #[tokio::test]
    async fn test_ada_coding_conformance() {
        agent_conformance::assert_single_engine_executes_turn(SPEC_JSON)
            .await
            .expect("ada-coding 产品单一引擎合规性校验通过");
    }
}
