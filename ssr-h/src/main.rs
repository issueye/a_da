//! ssr-h：a_da 的独立 SSR agent 前端实验版。
//!
//! 刻意不加入根 workspace：`cargo build -p ssr-h` 不会成为现有构建的一部分；
//! 运行 `cargo run --manifest-path ssr-h/Cargo.toml` 才会启动这个独立页面。

mod agent;
mod context;
mod guard;
mod handlers;
mod html;
mod markdown;
mod plugins;
mod questions;
mod templates;
mod tsx;
mod ui;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use serde::Deserialize;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use agent::AgentClient;
use handlers::AppState;
use plugins::PluginRegistry;

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

    let plugins_dir = args.plugins_dir.unwrap_or_else(default_plugins_dir);

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

    axum::serve(listener, app(state)).await?;
    Ok(())
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::chat))
        .route("/overview", get(handlers::dashboard))
        .route("/partials/dashboard", get(handlers::dashboard_partial))
        .route("/__auth", get(guard::auth_form).post(guard::auth_submit))
        .route(
            "/threads",
            get(handlers::chat).post(handlers::create_thread),
        )
        .route("/threads/{id}/partial", get(handlers::transcript_partial))
        .route("/threads/{id}/live", get(handlers::live))
        .route("/threads/{id}/stream", get(handlers::live_stream))
        .route("/threads/{id}", get(handlers::chat_thread))
        .route(
            "/threads/{id}/send",
            // 图片以 data URI 随表单提交（最多 4 张、单张 base64 后 ≤12MB）：
            // axum 默认 2MB 的 body 上限会直接 413，这里按最大合法体积放宽。
            post(handlers::send_message).layer(DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route("/threads/{id}/abort", post(handlers::abort_thread))
        // 模型选择器（DeepSeek 式弹层）：切换模型 / 推理等级后回到原会话。
        .route("/model/select", post(handlers::select_model))
        .route("/model/effort", post(handlers::select_effort))
        .route("/approval/mode", post(handlers::set_approval_mode))
        .route("/threads/{id}/retry", post(handlers::retry_thread))
        .route("/threads/{id}/rename", post(handlers::rename_thread))
        .route("/threads/{id}/delete", post(handlers::delete_thread))
        // 空会话里的模式 / 工作区选择（原「新建会话」弹窗的两项，挪到会话页内）
        .route("/threads/{id}/mode", post(handlers::set_thread_mode))
        .route(
            "/threads/{id}/workspace",
            post(handlers::set_thread_workspace),
        )
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
        .route(
            "/questions/{call_id}/answer",
            post(handlers::answer_question),
        )
        .route("/providers", get(handlers::providers_redirect))
        .route("/workspaces", get(handlers::workspace_picker))
        .route("/workspaces/select", post(handlers::select_workspace))
        .route("/workspaces/mkdir", post(handlers::make_workspace_dir))
        // 空会话页的弹窗目录选择器（htmx 片段；/workspaces 页面仍是无 JS 兜底）
        .route(
            "/workspaces/picker",
            get(handlers::workspace_picker_fragment),
        )
        .route(
            "/workspaces/picker/mkdir",
            post(handlers::workspace_picker_mkdir),
        )
        .route("/providers/new", post(handlers::create_provider))
        .route("/providers/{id}", post(handlers::save_provider))
        .route(
            "/providers/{id}/activate",
            post(handlers::activate_provider),
        )
        .route("/providers/{id}/delete", post(handlers::delete_provider))
        .route("/providers/{id}/models", post(handlers::add_provider_model))
        .route(
            "/providers/{id}/models/fetch",
            post(handlers::fetch_provider_models),
        )
        .route(
            "/providers/{id}/models/{index}",
            post(handlers::update_provider_model),
        )
        .route(
            "/providers/{id}/models/{index}/delete",
            post(handlers::delete_provider_model),
        )
        .route("/providers/{id}/test", post(handlers::test_provider))
        .route("/extensions", get(handlers::extensions))
        .route("/extensions/reload", post(handlers::reload_frontend))
        .route("/extensions/{id}/enable", post(handlers::enable_plugin))
        .route("/extensions/{id}/disable", post(handlers::disable_plugin))
        .route("/agent-plugins/reload", post(handlers::reload_agent))
        .route("/api/plugins", get(handlers::plugins_api))
        .route("/ext/{id}/assets/{*path}", get(handlers::plugin_asset))
        .route(
            "/ext/{id}/api/{*path}",
            get(handlers::plugin_api).post(handlers::plugin_api),
        )
        .route("/ext/{id}/{slug}", get(handlers::plugin_page))
        .route("/static/app.css", get(static_css))
        // 组件库按层拆开：引用顺序必须保持 components → layout → widgets → shell → pages，
        // 与拆分前的单文件串联顺序一致（层叠行为不变）。
        .route("/static/components.css", get(static_components_css))
        .route("/static/layout.css", get(static_layout_css))
        .route("/static/widgets.css", get(static_widgets_css))
        .route("/static/shell.css", get(static_shell_css))
        .route("/static/pages.css", get(static_pages_css))
        .route("/static/app.js", get(static_app_js))
        .route("/static/ada-select.js", get(static_ada_select_js))
        .route("/static/ada-modal.js", get(static_ada_modal_js))
        .route("/static/components.js", get(static_components_js))
        .route("/static/chat.js", get(static_chat_js))
        .route("/static/theme.js", get(static_theme_js))
        .route("/static/htmx.min.js", get(static_htmx))
        .route("/static/alpine.min.js", get(static_alpine))
        .fallback(handlers::not_found)
        .with_state(state.clone())
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(handlers::rewrite_plugin_redirect))
        // 必须最外层：先做 Host/Origin/令牌校验，再进压缩与业务。
        .layer(axum::middleware::from_fn_with_state(state, guard::guard))
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

/// 布局层：应用外壳、左右两栏、供应商清单与模型表格。
async fn static_layout_css() -> Response {
    static_asset(
        include_bytes!("../static/layout.css"),
        "text/css; charset=utf-8",
    )
}

/// 控件层：下拉选择与弹窗。
async fn static_widgets_css() -> Response {
    static_asset(
        include_bytes!("../static/widgets.css"),
        "text/css; charset=utf-8",
    )
}

/// 外壳层：复制按钮、侧栏、主区、审批/排队浮层。
async fn static_shell_css() -> Response {
    static_asset(
        include_bytes!("../static/shell.css"),
        "text/css; charset=utf-8",
    )
}

/// 页面层：任务规划、插件悬浮区、响应式。
async fn static_pages_css() -> Response {
    static_asset(
        include_bytes!("../static/pages.css"),
        "text/css; charset=utf-8",
    )
}

/// Web Component：`<ada-select>`。
async fn static_ada_select_js() -> Response {
    static_asset(
        include_bytes!("../static/ada-select.js"),
        "text/javascript; charset=utf-8",
    )
}

/// Web Component：`<ada-modal>`。
async fn static_ada_modal_js() -> Response {
    static_asset(
        include_bytes!("../static/ada-modal.js"),
        "text/javascript; charset=utf-8",
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

