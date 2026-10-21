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
use crate::guard::SharedAccess;
use crate::plugins::{PluginRegistry, Slot};
use crate::slang::{AppMeta, RenderContext, Trust};
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

fn base(state: &AppState, title: &str, active: &str, flash: Option<Flash>) -> PageBase {
    let context = render_context(state, active);
    PageBase {
        title: title.to_string(),
        active: active.to_string(),
        workspace: state.agent.workspace.clone(),
        gateway: state.agent.base_url().to_string(),
        product: "ssr-h · agent frontend".to_string(),
        plugin_nav: state.frontend.nav_links(),
        flash,
        slots: slot_content(&state.frontend, &context),
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
    .with_trust(Trust::Local)
    .with_data(
        serde_json::json!({
            "plugin_count": state.frontend.count(),
        })
        .into(),
    )
    .with_route_path(active)
}

/// 组装输入框（组件库 + 插件插槽）。
///
/// 工具行左右两侧与辅助区都是**插件插槽**：插件可以往里放自己的控件，
/// 插件挂了或坏了，插槽退回默认实现，输入框照常可用。
/// 输入框与辅助区需要的运行时状态。
///
/// 打包成一个结构体而不是 11 个参数：页首渲染与 live 轮询必须用**同一份**输入，
/// 否则"页面上的状态"和"轮询回来的状态"会各写一套。

fn slot_content(registry: &PluginRegistry, context: &RenderContext) -> SlotContent {
    let pick = |slot: Slot| registry.render_slot_with(slot, context).html;
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
        base: base(&state, "页面不存在", "", None),
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
}

mod chat;
mod dashboard;
mod extensions;
mod providers;

pub use chat::*;
pub use dashboard::*;
pub use extensions::*;
pub use providers::*;
