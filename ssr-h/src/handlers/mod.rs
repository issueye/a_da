//! HTTP 处理器：按页面家族拆成子模块，这里只放共享类型与工具。
//!
//! - [`chat`]：对话页 / live 轮询 / 会话动作（发送、中止、重试、排队、审批）
//! - [`dashboard`]：概览
//! - [`providers`]：供应商与模型管理（含编辑/新增弹窗）
//! - [`extensions`]：插件页与插件动作
//!
//! 新增页面请按家族放进对应子模块，不要再往本文件里堆。

use askama::Template;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use crate::agent::AgentClient;
use crate::context::{AppMeta, RenderContext};
use crate::guard::SharedAccess;
use crate::plugins::{PluginRegistry, Slot};
use crate::templates::*;
use crate::ui::{self, ConfirmDialog};

#[derive(Clone)]
pub struct AppState {
    pub agent: Arc<AgentClient>,
    pub frontend: Arc<PluginRegistry>,
    pub access: SharedAccess,
}

pub struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn upstream(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, message)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>ssr-h 错误</title>\
             <body style=\"font:14px/1.6 system-ui;padding:2rem;max-width:720px;margin:auto\">\
             <h1 style=\"font-size:18px\">请求失败</h1><p style=\"color:#b91c1c\">{}</p>\
             <p><a href=\"/\">返回概览</a></p></body>",
            escape_html(&self.message)
        );
        (self.status, Html(body)).into_response()
    }
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn render<T: Template>(template: &T) -> Result<Html<String>, AppError> {
    template.render().map(Html).map_err(|error| {
        AppError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("模板渲染失败: {error}"),
        )
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    notice: Option<String>,
    error: Option<String>,
    count: Option<usize>,
    failed: Option<usize>,
    thread: Option<String>,
    /// 供应商页当前选中项（Master-Detail 的 Detail）。
    provider: Option<String>,
    /// 供应商详情区的页签（`connection` / `models`，缺省为 `connection`）。
    tab: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateThreadForm {
    title: String,
    /// 目标工作区；缺省时由侧栏/后端决定（通常是当前激活工作区）。
    workspace: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SendForm {
    text: String,
}

#[derive(Debug, Deserialize)]
pub struct ApprovalForm {
    thread_id: String,
    approved: String,
}

fn flash_from_query(query: &PageQuery) -> Option<Flash> {
    if let Some(error) = &query.error {
        return Some(Flash {
            kind: "error".to_string(),
            message: error.clone(),
        });
    }
    let notice = query.notice.as_deref()?;
    let message = match notice {
        "frontend_reloaded" => {
            let failed = query.failed.unwrap_or(0);
            if failed > 0 {
                format!(
                    "前端插件已重新扫描：{} 个（{} 个加载失败，见下方错误）",
                    query.count.unwrap_or(0),
                    failed
                )
            } else {
                format!("前端插件已重新扫描：当前 {} 个", query.count.unwrap_or(0))
            }
        }
        "agent_reloaded" => "已请求 agent 重载插件；新增工具将在下一条消息生效".to_string(),
        "created" => "会话已创建".to_string(),
        "sent" => "消息已发送".to_string(),
        "aborted" => "已请求中止当前轮次".to_string(),
        "approved" => "已提交批准结果".to_string(),
        "rejected" => "已提交拒绝结果".to_string(),
        "retried" => "已请求重试上一轮".to_string(),
        "queue_promoted" => "已把排队消息提升到队首".to_string(),
        "queue_removed" => "已删除排队消息".to_string(),
        "queue_cleared" => "已清空当前会话的排队消息".to_string(),
        "workspace_selected" => "已切换工作区".to_string(),
        "workspace_created" => "已在工作区创建会话".to_string(),
        "workspace_mkdir" => "已新建目录".to_string(),
        "plugin_enabled" => "插件已启用并挂载到界面".to_string(),
        "plugin_disabled" => "插件已停用，界面已回到默认实现".to_string(),
        "provider_created" => "已新建供应商，填好端点与密钥后保存".to_string(),
        "provider_saved" => "供应商连接参数已保存".to_string(),
        "provider_saved_activated" => "已保存并设为当前供应商".to_string(),
        "provider_activated" => "已切换当前供应商".to_string(),
        "provider_deleted" => "供应商已删除".to_string(),
        "model_added" => "模型已加入该供应商".to_string(),
        "model_saved" => "模型参数已保存".to_string(),
        "model_deleted" => "模型已从该供应商移除".to_string(),
        other => other.to_string(),
    };
    Some(Flash {
        kind: "success".to_string(),
        message,
    })
}

async fn base(state: &AppState, title: &str, active: &str, flash: Option<Flash>) -> PageBase {
    base_with_snapshot(state, title, active, flash, None).await
}

/// 页面外壳 + 快照。
///
/// 有快照的页面（聊天页）把 `data.tokens` 一起给插槽，插件才能在输入框上方显示
/// 实时用量；没有快照的页面（供应商/扩展）维持原样，插件读到 `tokens = null`。
async fn base_with_snapshot(
    state: &AppState,
    title: &str,
    active: &str,
    flash: Option<Flash>,
    snapshot: Option<&Value>,
) -> PageBase {
    let context = match snapshot {
        Some(snapshot) => render_context_with_tokens(state, active, snapshot, None),
        None => render_context(state, active),
    };
    let slots = slot_content(&state.frontend, context).await;
    // 侧栏"新会话"要落在当前激活工作区。快照里的 `workspace.project` 是 agent 的真实
    // 当前项目（与 desktop-ui 的 `activeWorkspace` 同源）；没有快照时退回启动参数。
    let active_workspace = snapshot
        .and_then(|snapshot| snapshot.get("workspace"))
        .and_then(|workspace| workspace.get("project"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|workspace| !workspace.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.agent.workspace.clone());
    PageBase {
        title: title.to_string(),
        active: active.to_string(),
        active_workspace_name: workspace_name(&active_workspace),
        active_workspace,
        gateway: state.agent.base_url().to_string(),
        product: "ssr-h · agent frontend".to_string(),
        plugin_nav: state.frontend.nav_links(),
        plugin_styles: state.frontend.styles(),
        flash,
        slots,
        icons: ui::sprite(),
        // 每个页面外壳都带一份确认弹窗（组件库渲染），二级页不再退回原生 confirm。
        confirm_dialog: ConfirmDialog::markup(),
    }
}

/// 插件片段/页面的只读上下文：不把 `AppState` 整个暴露给模板。
fn render_context(state: &AppState, active: &str) -> RenderContext {
    RenderContext::new(AppMeta {
        workspace: state.agent.workspace.clone(),
        gateway: state.agent.base_url().to_string(),
        product: "ssr-h · agent frontend".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
    .with_data(serde_json::json!({
        "plugin_count": state.frontend.count(),
    }))
    .with_route_path(active)
}

/// 带上下文用量的渲染上下文。
///
/// 只有拿得到 `snapshot` 的页面（插件页、概览卡片）才有 `data.tokens`；
/// 拿不到时插件就这么写 `props.data.tokens` 为 `null`，页面照常渲染。
///
/// `thread_id` 是"这个插槽属于哪条会话"：聊天页传当前会话，二级页传 `None`
/// （沿用快照的 `activeThreadId`）。不传会出现"界面在看 A，天气插件按快照的
/// activeThreadId（可能是 B）算用量"——实测显示成 0%。
fn render_context_with_tokens(
    state: &AppState,
    active: &str,
    snapshot: &Value,
    thread_id: Option<&str>,
) -> RenderContext {
    render_context(state, active).with_data(serde_json::json!({
        "plugin_count": state.frontend.count(),
        "tokens": token_usage_view(snapshot, thread_id),
    }))
}

/// 当前会话的**上下文用量**（给插件用）。
///
/// 口径与引擎的上下文治理一致：最近一次模型调用如实报回的 `usage.promptTokens`
/// 除以 `config.contextWindow`——是"这一轮实际塞进窗口的输入规模"，不是累计消费，
/// 也不是按字符估算。窗口未配置（0）时 `percent` 为 `null`，由插件自己决定怎么显示。
fn token_usage_view(snapshot: &Value, thread_id: Option<&str>) -> Value {
    let threads = array_at(snapshot, "threads");
    let wanted = thread_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| str_at(snapshot, "activeThreadId").to_string());
    let thread = threads
        .iter()
        .find(|thread| !wanted.is_empty() && str_at(thread, "id") == wanted)
        .or_else(|| threads.first());
    let used = thread.map(last_prompt_tokens).unwrap_or(0);
    let window = snapshot
        .get("config")
        .and_then(|config| config.get("contextWindow"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    tracing::debug!(
        thread = %wanted,
        found = thread.is_some(),
        items = thread
            .and_then(|thread| thread.get("items"))
            .and_then(|items| items.as_array())
            .map(Vec::len)
            .unwrap_or(0),
        used,
        window,
        "token usage view"
    );
    let percent = (window > 0).then(|| {
        // 四舍五入到整数百分比；窗口已知时用它分档（晴 / 多云 / 阴 / 暴雨）。
        let scaled = used.saturating_mul(100);
        (scaled.saturating_add(window / 2) / window).min(100)
    });
    // 会话显示名与侧栏同口径：原始标题是"新会话"时，用第一条用户消息兜底。
    // 否则插件浮窗会显示"新会话"，而界面标题早就变成任务名了。
    let thread_title = thread
        .map(|thread| {
            let raw = str_at(thread, "title").trim();
            if raw.is_empty() || raw == "新会话" {
                array_at(thread, "items")
                    .iter()
                    .find(|item| str_at(item, "kind") == "user")
                    .map(|item| short_title(str_at(item, "text"), 30))
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| "新会话".to_string())
            } else {
                raw.to_string()
            }
        })
        .unwrap_or_default();
    serde_json::json!({
        "used": used,
        "window": window,
        "percent": percent,
        "threadId": thread.map(|thread| str_at(thread, "id")).unwrap_or(""),
        "threadTitle": thread_title,
        "model": snapshot
            .get("config")
            .and_then(|config| config.get("model"))
            .and_then(Value::as_str)
            .unwrap_or(""),
    })
}

/// 会话里**最近一次**带用量的助手回复的输入规模。
///
/// 一轮里有多次工具调用时，最后一条助手卡的 promptTokens 就是"当前上下文大小"；
/// 流式中的助手卡还没有 usage，要跳过而不是当成 0。
fn last_prompt_tokens(thread: &Value) -> u64 {
    array_at(thread, "items")
        .iter()
        .rev()
        .find_map(|item| {
            if str_at(item, "kind") != "assistant" {
                return None;
            }
            item.get("usage")
                .and_then(|usage| usage.get("promptTokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0)
}

/// 组装输入框（组件库 + 插件插槽）。
///
/// 工具行左右两侧与辅助区都是**插件插槽**：插件可以往里放自己的控件，
/// 插件挂了或坏了，插槽退回默认实现，输入框照常可用。
///
/// 十个插槽一次性丢进阻塞线程池渲染（TSX 是同步 CPU 活），别在这里逐个 `await`。
async fn slot_content(registry: &Arc<PluginRegistry>, context: RenderContext) -> SlotContent {
    let outputs = crate::plugins::render_slots_off_thread(
        Arc::clone(registry),
        Slot::all(),
        context,
    )
    .await;
    let mut by_slot: std::collections::HashMap<&'static str, String> = Slot::all()
        .iter()
        .zip(outputs)
        .map(|(slot, output)| (slot.as_str(), output.html))
        .collect();
    let mut pick = |slot: Slot| by_slot.remove(slot.as_str()).unwrap_or_default();
    SlotContent {
        sidebar_top: pick(Slot::SidebarTop),
        sidebar_bottom: pick(Slot::SidebarBottom),
        topbar_right: pick(Slot::TopbarRight),
        composer_left: pick(Slot::ComposerLeft),
        composer_right: pick(Slot::ComposerRight),
        composer_footer: pick(Slot::ComposerFooter),
        composer_above: pick(Slot::ComposerAbove),
        transcript_after: pick(Slot::TranscriptAfter),
        extensions_list: pick(Slot::ExtensionsList),
        dashboard_widgets: pick(Slot::DashboardWidgets),
    }
}

fn redirect(path: &str) -> Response {
    (
        StatusCode::SEE_OTHER,
        [(
            header::LOCATION,
            HeaderValue::from_str(path).unwrap_or(HeaderValue::from_static("/")),
        )],
    )
        .into_response()
}

/// 用户可见的失败原因：去掉 URL 里的凭据，只留第一行。
///
/// 上游错误可能很长（甚至带网关地址），直接塞进页面既难读也容易泄露；
/// 完整错误进服务端日志，页面只给"发生了什么"。
fn public_reason(error: &anyhow::Error) -> String {
    let text = redact_tokens(&format!("{error:#}"));
    let first = text.lines().next().unwrap_or("未知错误").trim();
    truncate(first, 200)
}

/// 把 `token=…` 的值打码（URL query 是最常见的泄漏位置）。
fn redact_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find("token=") {
        out.push_str(&rest[..position]);
        out.push_str("token=***");
        rest = &rest[position + "token=".len()..];
        let end = rest
            .find(|ch: char| ch == '&' || ch.is_whitespace() || ch == '"' || ch == '\'')
            .unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// 查询参数编码：只让 ASCII 安全字符原样通过，其余按 UTF-8 字节百分号编码。
fn encode_query(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 动作失败：回跳到会话页并用 flash 说明原因，**不**把整个界面换成裸错误页。
fn str_at<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn array_at<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn running_ids(snapshot: &Value) -> Vec<String> {
    array_at(snapshot, "runningThreadIds")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// 工作区显示名：路径末段（`E:/code/app` → `app`）。
fn workspace_name(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

fn short_title(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.push('…');
    }
    out
}

fn thread_row(value: &Value, running: &[String]) -> ThreadRow {
    let id = str_at(value, "id").to_string();
    let items = value.get("items").and_then(Value::as_array);
    let item_slice = items.map(Vec::as_slice).unwrap_or(&[]);
    let raw_title = str_at(value, "title").trim();
    let title = if raw_title.is_empty() || raw_title == "新会话" {
        item_slice
            .iter()
            .find(|item| str_at(item, "kind") == "user")
            .map(|item| short_title(str_at(item, "text"), 30))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "新会话".to_string())
    } else {
        raw_title.to_string()
    };
    ThreadRow {
        running: running.iter().any(|r| r == &id),
        id,
        title,
        workspace: str_at(value, "workspace").to_string(),
        mode: {
            let mode = str_at(value, "mode");
            if mode.is_empty() {
                "code".to_string()
            } else {
                mode.to_string()
            }
        },
        item_count: items.map(Vec::len).unwrap_or(0),
        created_at: value
            .get("createdAt")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n…（已截断）");
    out
}

/// 当前模型：优先取运行中快照的 `config.model`，回退到 `/api/config` 的 saved。
pub async fn not_found(State(state): State<AppState>) -> Response {
    let page = ErrorPage {
        base: base(&state, "页面不存在", "", None).await,
        message: "这个地址没有对应的页面。".to_string(),
    };
    match page.render() {
        Ok(html) => (StatusCode::NOT_FOUND, Html(html)).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("渲染错误页失败: {error}"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_reason_redacts_tokens_and_keeps_only_the_first_line() {
        let error =
            anyhow::anyhow!("连接网关失败: ws://127.0.0.1:1/rpc?workspace=w&token=SECRET\n第二行");
        let reason = public_reason(&error);
        assert!(!reason.contains("SECRET"));
        assert!(!reason.contains("第二行"));
        assert!(reason.contains("token=***"));
    }

    #[test]
    fn encode_query_percent_encodes_utf8_and_reserved_chars() {
        assert_eq!(encode_query("a b&c=中"), "a+b%26c%3D%E4%B8%AD");
    }

    fn snapshot(threads: serde_json::Value, window: u64) -> Value {
        serde_json::json!({
            "activeThreadId": "t1",
            "threads": threads,
            "config": { "model": "deepseek-chat", "contextWindow": window },
        })
    }

    fn assistant(prompt: u64) -> Value {
        serde_json::json!({
            "kind": "assistant",
            "usage": { "promptTokens": prompt, "completionTokens": 10, "totalTokens": prompt + 10 }
        })
    }

    fn thread(id: &str, items: Vec<Value>) -> Value {
        serde_json::json!({ "id": id, "title": format!("会话 {id}"), "items": items })
    }

    #[test]
    fn token_usage_is_prompt_tokens_over_context_window() {
        let thread = thread("t1", vec![assistant(64_000)]);
        let view = token_usage_view(&snapshot(serde_json::json!([thread]), 128_000), None);
        assert_eq!(view["used"], 64_000);
        assert_eq!(view["window"], 128_000);
        assert_eq!(view["percent"], 50);
        assert_eq!(view["threadTitle"], "会话 t1");
        assert_eq!(view["model"], "deepseek-chat");
    }

    #[test]
    fn token_usage_uses_the_latest_assistant_usage_and_skips_streaming_cards() {
        // 最后一张助手卡还在流式（没有 usage），不能用它把用量清零。
        let thread = thread(
            "t1",
            vec![
                assistant(1_000),
                serde_json::json!({ "kind": "user", "text": "再来" }),
                serde_json::json!({ "kind": "assistant", "text": "（正在生成）" }),
            ],
        );
        let view = token_usage_view(&snapshot(serde_json::json!([thread]), 10_000), None);
        assert_eq!(view["used"], 1_000);
        assert_eq!(view["percent"], 10);
    }

    #[test]
    fn token_usage_title_falls_back_to_first_user_message() {
        // 会话头还叫"新会话"、界面标题已经由首条用户消息派生时，
        // 插件浮窗/完整页面应显示同一个名字，而不是"新会话"。
        let thread = serde_json::json!({
            "id": "t1",
            "title": "新会话",
            "items": [
                { "kind": "user", "text": "提交后端代码并验证构建" },
                assistant(1_000),
            ],
        });
        let view = token_usage_view(&snapshot(serde_json::json!([thread]), 100_000), Some("t1"));
        assert_eq!(view["threadTitle"], "提交后端代码并验证构建");
    }

    #[test]
    fn token_usage_rounds_and_clamps_to_100() {
        let round = thread("t1", vec![assistant(1)]);
        assert_eq!(
            token_usage_view(&snapshot(serde_json::json!([round]), 3), None)["percent"],
            33
        );
        let over = thread("t1", vec![assistant(999_999)]);
        assert_eq!(
            token_usage_view(&snapshot(serde_json::json!([over]), 128_000), None)["percent"],
            100
        );
    }

    #[test]
    fn token_usage_without_window_is_null_instead_of_zero_percent() {
        // 窗口未配置：`0%` 会假装"很空"，必须给 null 让插件显示"无数据"。
        let thread = thread("t1", vec![assistant(5_000)]);
        let view = token_usage_view(&snapshot(serde_json::json!([thread]), 0), None);
        assert_eq!(view["used"], 5_000);
        assert!(view["percent"].is_null(), "{view}");
    }

    #[test]
    fn token_usage_follows_the_active_thread_then_falls_back_to_the_first() {
        let threads = serde_json::json!([
            thread("t1", vec![assistant(1_000)]),
            thread("t2", vec![assistant(90_000)]),
        ]);
        let mut snap = snapshot(threads.clone(), 100_000);
        snap["activeThreadId"] = serde_json::json!("t2");
        assert_eq!(token_usage_view(&snap, None)["percent"], 90);
        assert_eq!(token_usage_view(&snap, Some("t1"))["percent"], 1);

        // 没有任何活动会话时用第一条，而不是让页面变成"无数据"。
        snap["activeThreadId"] = serde_json::json!("");
        assert_eq!(token_usage_view(&snap, None)["percent"], 1);
    }

    #[test]
    fn token_usage_of_an_empty_gateway_snapshot_is_all_zero() {
        let view = token_usage_view(&Value::Null, None);
        assert_eq!(view["used"], 0);
        assert_eq!(view["window"], 0);
        assert!(view["percent"].is_null());
        assert_eq!(view["threadId"], "");
    }
}

mod chat;
mod dashboard;
mod extensions;
mod providers;
mod workspaces;

pub use chat::*;
pub use dashboard::*;
pub use extensions::*;
pub use providers::*;
pub use workspaces::*;

