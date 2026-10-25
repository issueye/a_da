//! 独立运行入口（开发 / 实机验收）：起一个监听、连已启动的网关。
//!
//! 最终形态由网关直接挂载本 crate；这个入口保留一条"不依赖网关进程"的回归路径，
//! 也方便对比 `HttpBackend` 与 `GatewayBackend` 的行为。

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use serde::Deserialize;

use crate::agent::AgentClient;
use crate::guard;
use crate::handlers::AppState;
use crate::plugins::PluginRegistry;
use crate::router;

#[derive(Debug, Parser)]
#[command(name = "ssr-h", about = "a_da SSR agent frontend prototype")]
struct Args {
    /// 监听地址；默认只绑回环
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// 页面端口
    #[arg(long, default_value_t = 8788)]
    port: u16,

    /// 允许绑定非回环地址（必须同时设置 --ui-token）
    #[arg(long)]
    allow_remote: bool,

    /// 浏览器访问令牌；非回环监听必填，回环也可主动开启
    #[arg(long)]
    ui_token: Option<String>,

    /// 网关 HTTP 地址，例如 http://127.0.0.1:52353
    #[arg(long)]
    gateway: Option<String>,

    /// 网关 token（没有鉴权时可省略）
    #[arg(long)]
    token: Option<String>,

    /// agent 工作区
    #[arg(long)]
    workspace: Option<String>,

    /// 前端插件目录
    #[arg(long)]
    plugins_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct Discovery {
    url: Option<String>,
    port: Option<u16>,
    token: Option<String>,
    workspace: Option<String>,
}

pub async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agent_web=info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();
    let discovery = read_discovery();

    let gateway = args
        .gateway
        .or_else(|| std::env::var("A_DA_GATEWAY_URL").ok())
        .or_else(|| discovery.as_ref().and_then(|d| d.url.clone()))
        .or_else(|| {
            discovery
                .as_ref()
                .and_then(|d| d.port)
                .map(|port| format!("http://127.0.0.1:{port}"))
        })
        .unwrap_or_else(|| "http://127.0.0.1:52353".to_string());

    let token = args
        .token
        .or_else(|| std::env::var("A_DA_GATEWAY_TOKEN").ok())
        .or_else(|| discovery.as_ref().and_then(|d| d.token.clone()))
        .unwrap_or_default();

    // 工作区优先级：显式参数 → 环境变量 → 网关发现文件 → 当前目录。
    // 发现文件里的 workspace 可能是 gate 自己的应用目录（实测本机是 `~/.a-da`），
    // 直接拿来当 agent 工作区会把凭据目录暴露给会话，所以必须先校验。
    let discovery_raw = discovery.as_ref().and_then(|d| d.workspace.clone());
    let discovery_workspace = discovery_raw
        .clone()
        .filter(|candidate| valid_workspace(candidate));
    let cwd_workspace = std::env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().to_string());
    let (workspace, workspace_source) = if let Some(workspace) = args.workspace {
        (workspace, "--workspace")
    } else if let Ok(workspace) = std::env::var("A_DA_WORKSPACE") {
        (workspace, "A_DA_WORKSPACE")
    } else if let Some(workspace) = discovery_workspace {
        (workspace, "gateway_endpoint.json")
    } else if let Some(workspace) = cwd_workspace {
        if let Some(raw) = discovery_raw {
            tracing::warn!(
                "忽略 gateway_endpoint.json 里的 workspace（不存在或指向 ~/.a-da）: {raw}"
            );
        }
        (workspace, "current_dir")
    } else {
        (".".to_string(), "fallback")
    };
    if !std::path::Path::new(&workspace).is_dir() {
        tracing::warn!("工作区目录不存在，网关可能拒绝拉起 agent: {workspace}");
    }

    let plugins_dir = args.plugins_dir.unwrap_or_else(router::default_plugins_dir);

    let host: IpAddr = args
        .host
        .parse()
        .map_err(|error| anyhow::anyhow!("非法监听地址 {}: {error}", args.host))?;

    // 非回环监听会把整个 agent（以及服务端的网关 token）暴露到局域网；
    // 与网关自身的安全模型对齐：必须显式确认 + 访问令牌，否则拒绝启动。
    if let Some(token) = args.ui_token.as_deref() {
        guard::validate_token(token)
            .map_err(|message| anyhow::anyhow!("{message}（--ui-token 太弱或被拒绝）"))?;
    }
    if !host.is_loopback() {
        if !args.allow_remote {
            anyhow::bail!(
                "拒绝对非回环地址 {host} 监听：默认只允许 127.0.0.1。\
                 确实需要局域网访问时，请显式加 --allow-remote 并设置 --ui-token。"
            );
        }
        if args.ui_token.as_deref().unwrap_or("").is_empty() {
            anyhow::bail!(
                "非回环监听必须设置 --ui-token（ssr-h 只提供明文 HTTP，令牌是唯一闸门）。"
            );
        }
        tracing::warn!(
            "ssr-h 正在以明文 HTTP 监听 {host}:{}，访问令牌会以明文传输；请只在可信网络使用",
            args.port
        );
    }

    let access = Arc::new(guard::Access::for_host(host, args.port, args.ui_token));
    let frontend = Arc::new(PluginRegistry::new(plugins_dir.clone()));
    let agent = Arc::new(AgentClient::new(gateway.clone(), token, workspace.clone()));
    let state = AppState {
        agent,
        frontend,
        access: access.clone(),
    };

    let addr = std::net::SocketAddr::new(host, args.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("ssr-h listening on http://{addr}");
    tracing::info!("gateway: {gateway}");
    tracing::info!("workspace: {workspace}（来源：{workspace_source}）");
    tracing::info!("frontend plugins: {}", plugins_dir.display());
    tracing::info!("access: {}", guard::access_summary(&access));

    axum::serve(listener, router::router(state)).await?;
    Ok(())
}

fn read_discovery() -> Option<Discovery> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let path = PathBuf::from(home)
        .join(".a-da")
        .join("gateway_endpoint.json");
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 网关发现文件里的 workspace 只有在"真实存在且不是应用目录"时才采用。
///
/// 实测本机的 `gateway_endpoint.json` 里 `workspace` 是 `C:\Users\<user>\.a-da`；
/// 拿它当 agent 工作区等于把保留目录（凭据/会话）暴露成会话的默认目录。
fn valid_workspace(candidate: &str) -> bool {
    let path = std::path::Path::new(candidate.trim());
    if !path.is_dir() {
        return false;
    }
    let Some(home) = app_home() else {
        return true;
    };
    match (path.canonicalize(), home.canonicalize()) {
        (Ok(candidate), Ok(home)) => candidate != home,
        _ => true,
    }
}

fn app_home() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(|home| PathBuf::from(home).join(".a-da"))
}
