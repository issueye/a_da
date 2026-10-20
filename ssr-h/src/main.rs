//! ssr-h：a_da 的独立 SSR agent 前端实验版。
//!
//! 刻意不加入根 workspace：`cargo build -p ssr-h` 不会成为现有构建的一部分；
//! 运行 `cargo run --manifest-path ssr-h/Cargo.toml` 才会启动这个独立页面。

mod agent;
mod handlers;
mod markdown;
mod plugins;
mod templates;
mod ui;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use serde::Deserialize;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use agent::AgentClient;
use plugins::PluginRegistry;
use handlers::AppState;

#[derive(Debug, Parser)]
#[command(name = "ssr-h", about = "a_da SSR agent frontend prototype")]
struct Args {
    /// 监听地址；默认只绑回环
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// 页面端口
    #[arg(long, default_value_t = 8788)]
    port: u16,

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ssr_h=info,tower_http=info".into()),
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

    let workspace = args
        .workspace
        .or_else(|| std::env::var("A_DA_WORKSPACE").ok())
        .or_else(|| discovery.as_ref().and_then(|d| d.workspace.clone()))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| ".".to_string());

    let plugins_dir = args.plugins_dir.unwrap_or_else(default_plugins_dir);

    let frontend = Arc::new(PluginRegistry::new(plugins_dir.clone()));
    let agent = Arc::new(AgentClient::new(gateway.clone(), token, workspace.clone()));
    let state = AppState { agent, frontend };

    let host: IpAddr = args
        .host
        .parse()
        .map_err(|error| anyhow::anyhow!("非法监听地址 {}: {error}", args.host))?;
    let addr = std::net::SocketAddr::new(host, args.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("ssr-h listening on http://{addr}");
    tracing::info!("gateway: {gateway}");
    tracing::info!("workspace: {workspace}");
    tracing::info!("frontend plugins: {}", plugins_dir.display());

    axum::serve(listener, app(state)).await?;
    Ok(())
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::chat))
        .route("/overview", get(handlers::dashboard))
        .route("/partials/dashboard", get(handlers::dashboard_partial))
        .route(
            "/threads",
            get(handlers::chat).post(handlers::create_thread),
        )
        .route("/threads/{id}/partial", get(handlers::transcript_partial))
        .route("/threads/{id}", get(handlers::chat_thread))
        .route("/threads/{id}/send", post(handlers::send_message))
        .route("/threads/{id}/abort", post(handlers::abort_thread))
        .route("/threads/{id}/retry", post(handlers::retry_thread))
        .route("/threads/{id}/queue/clear", post(handlers::clear_queue))
        .route(
            "/threads/{id}/queue/{index}/promote",
            post(handlers::promote_queue),
        )
        .route(
            "/threads/{id}/queue/{index}/remove",
            post(handlers::remove_queue),
        )
        .route(
            "/approvals/{call_id}/decision",
            post(handlers::decide_approval),
        )
        .route("/providers", get(handlers::providers))
        .route("/extensions", get(handlers::extensions))
        .route("/extensions/reload", post(handlers::reload_frontend))
        .route("/extensions/{id}/enable", post(handlers::enable_plugin))
        .route("/extensions/{id}/disable", post(handlers::disable_plugin))
        .route("/agent-plugins/reload", post(handlers::reload_agent))
        .route("/api/plugins", get(handlers::plugins_api))
        .route("/ext/{id}/assets/{*path}", get(handlers::plugin_asset))
        .route("/ext/{id}/{slug}", get(handlers::plugin_page))
        .route("/static/app.css", get(static_css))
        .route("/static/components.css", get(static_components_css))
        .route("/static/app.js", get(static_app_js))
        .route("/static/components.js", get(static_components_js))
        .route("/static/chat.js", get(static_chat_js))
        .route("/static/theme.js", get(static_theme_js))
        .route("/static/htmx.min.js", get(static_htmx))
        .route("/static/alpine.min.js", get(static_alpine))
        .fallback(handlers::not_found)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// 插件目录按“可执行文件旁 → 当前目录 → 源码目录”查找。
///
/// 这样 `cargo run` 和把 `ssr-h.exe` 连同 `frontend-plugins/` 拷贝到别的机器都能工作。
fn default_plugins_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from));
    let cwd = std::env::current_dir().ok();
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        exe_dir.as_ref().map(|dir| dir.join("frontend-plugins")),
        cwd.as_ref().map(|dir| dir.join("frontend-plugins")),
        Some(manifest.join("frontend-plugins")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|path| path.is_dir())
        .unwrap_or_else(|| manifest.join("frontend-plugins"))
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

async fn static_css() -> Response {
    static_asset(
        include_bytes!("../static/app.css"),
        "text/css; charset=utf-8",
    )
}

/// 组件库样式（`--ada-*` 语义变量 + `ada-*` 组件类）。
async fn static_components_css() -> Response {
    static_asset(
        include_bytes!("../static/components.css"),
        "text/css; charset=utf-8",
    )
}

/// 组件库交互脚本（焦点态、折叠、复制、插槽）。
async fn static_components_js() -> Response {
    static_asset(
        include_bytes!("../static/components.js"),
        "text/javascript; charset=utf-8",
    )
}

async fn static_app_js() -> Response {
    static_asset(
        include_bytes!("../static/app.js"),
        "text/javascript; charset=utf-8",
    )
}

async fn static_chat_js() -> Response {
    static_asset(
        include_bytes!("../static/chat.js"),
        "text/javascript; charset=utf-8",
    )
}

async fn static_theme_js() -> Response {
    static_asset(
        include_bytes!("../static/theme.js"),
        "text/javascript; charset=utf-8",
    )
}

async fn static_htmx() -> Response {
    static_asset(
        include_bytes!("../static/htmx.min.js"),
        "text/javascript; charset=utf-8",
    )
}

async fn static_alpine() -> Response {
    static_asset(
        include_bytes!("../static/alpine.min.js"),
        "text/javascript; charset=utf-8",
    )
}

fn static_asset(bytes: &'static [u8], content_type: &'static str) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        bytes,
    )
        .into_response()
}
