//! SSR 界面的**路由组装点**（不监听、不 serve）。
//!
//! 网关挂载本模块返回的 [`Router`]，独立运行时 `standalone` 也调用同一份——
//! 两种形态的路由不会漂。

use std::path::PathBuf;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::guard;
use crate::handlers::{self, AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::chat))
        .route("/overview", get(handlers::dashboard))
        .route("/partials/dashboard", get(handlers::dashboard_partial))
        .route("/partials/processes", get(handlers::processes_partial))
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
        // 前端插件自省：**刻意不放在 `/api` 下**，避免与网关的 `/api/plugins`
        // （agent 插件列表）撞名；浏览器脚本见 `static/components.js`。
        .route(
            "/web-ui/frontend-plugins",
            get(handlers::plugins_api),
        )
        .route("/ext/{id}/assets/{*path}", get(handlers::plugin_asset))
        .route(
            "/ext/{id}/api/{*path}",
            get(handlers::plugin_api).post(handlers::plugin_api),
        )
        .route("/ext/{id}/{slug}", get(handlers::plugin_page))
        .route("/static/app.css", get(static_css))
        // 产品 LOGO：源码树 `assets/logo.png` 是唯一真源，编译期内嵌。
        .route("/static/logo.png", get(static_logo_png))
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
pub fn default_plugins_dir() -> PathBuf {
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

/// 产品 LOGO（`assets/logo.png`：黑天鹅 + 暖米白底，也是 Black Swan 主题的来源）。
async fn static_logo_png() -> Response {
    static_asset(
        include_bytes!("../../../assets/logo.png"),
        "image/png",
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
