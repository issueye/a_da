//! 页面与动作处理器。
//!
//! 所有 agent 数据都来自现有网关；本层只做 SSR 投影，不持有会话状态。

use std::sync::Arc;

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;

use crate::agent::AgentClient;
use crate::guard::SharedAccess;
use crate::plugins::{PluginRegistry, Slot};
use crate::templates::*;
use crate::ui::{
    self, AuxItem, Button, ButtonKind, ButtonVariant, Chip, Composer, ComposerAux, IconButton,
    Render, SendButton, Size, Tone,
};

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
        other => other.to_string(),
    };
    Some(Flash {
        kind: "success".to_string(),
        message,
    })
}

fn base(state: &AppState, title: &str, active: &str, flash: Option<Flash>) -> PageBase {
    PageBase {
        title: title.to_string(),
        active: active.to_string(),
        workspace: state.agent.workspace.clone(),
        gateway: state.agent.base_url().to_string(),
        product: "ssr-h · agent frontend".to_string(),
        plugin_nav: state.frontend.nav_links(),
        flash,
        slots: slot_content(&state.frontend),
        icons: ui::sprite(),
    }
}

/// 组装输入框（组件库 + 插件插槽）。
///
/// 工具行左右两侧与辅助区都是**插件插槽**：插件可以往里放自己的控件，
/// 插件挂了或坏了，插槽退回默认实现，输入框照常可用。
/// 输入框与辅助区需要的运行时状态。
///
/// 打包成一个结构体而不是 11 个参数：页首渲染与 live 轮询必须用**同一份**输入，
/// 否则"页面上的状态"和"轮询回来的状态"会各写一套。
#[derive(Debug, Clone)]
struct ComposerArgs<'a> {
    thread_id: &'a str,
    model: &'a str,
    approval: &'a str,
    approval_tone: &'a str,
    workspace_label: &'a str,
    workspace_path: &'a str,
    connected: bool,
    running: bool,
    queue_len: usize,
    approval_count: usize,
    item_count: usize,
}

fn composer_html(slots: &SlotContent, args: &ComposerArgs<'_>) -> String {
    // 左组：附件（未落地，明确禁用）、工作区、审批档位。
    let mut left = String::new();
    left.push_str(
        &IconButton::new("i-plus", "附件与提及（规划中，暂不可用）")
            .kind(ButtonKind::Button)
            .class("ada-composer__plus")
            .attrs("disabled aria-disabled=\"true\"")
            .render(),
    );
    if !args.workspace_label.is_empty() {
        left.push_str(
            &Chip::new(args.workspace_label, Tone::Neutral)
                .icon("i-git-branch")
                .title(args.workspace_path.to_string())
                .class("ada-chip--workspace")
                .render(),
        );
    }
    // 审批档位与模型都是只读展示：不加下拉箭头，避免"看起来能点、点了没反应"。
    // 用独立容器包住，live 轮询才能只换这一个药丸（不重建整条工具行）。
    left.push_str(&format!(
        r#"<span id="composer-approval">{}</span>"#,
        approval_chip_html(args.approval, args.approval_tone)
    ));
    left.push_str(&slots.composer_left);

    // 右组：模型 + 发送。
    let mut right = String::new();
    right.push_str(&format!(
        r#"<span id="composer-model">{}</span>"#,
        model_chip_html(args.model)
    ));
    right.push_str(&slots.composer_right);
    right.push_str(&SendButton::new("发送").title("发送（Enter）").render());

    Composer {
        action: format!("/threads/{}/send", args.thread_id),
        placeholder: "向 a_da 提问，或指派一个任务…".to_string(),
        focus_hint: Some("Enter 发送 · Shift+Enter 换行".to_string()),
        left,
        right,
        aux: Some(composer_aux(slots, args)),
    }
    .render()
}

/// 输入框下方的辅助区：状态提示 + 上下文 + 真实可用的快捷操作。
fn composer_aux(slots: &SlotContent, args: &ComposerArgs<'_>) -> ComposerAux {
    let mut status = Vec::new();
    if args.connected {
        status.push(AuxItem::new("网关已连接", Tone::Success).icon("i-plug"));
    } else {
        status.push(AuxItem::new("网关未连接", Tone::Danger).icon("i-alert"));
    }
    if args.running {
        status.push(AuxItem::new("运行中", Tone::Warn).icon("i-clock"));
    }
    if args.approval_count > 0 {
        status.push(
            AuxItem::new(format!("等待批准 {} 项", args.approval_count), Tone::Warn)
                .icon("i-shield-check"),
        );
    }
    if args.queue_len > 0 {
        status.push(
            AuxItem::new(format!("排队 {} 条", args.queue_len), Tone::Brand).icon("i-layers"),
        );
    }

    let mut context = Vec::new();
    // 断线时不显示"0 条消息"这类无法确认的数字（失败安全）。
    if args.connected {
        context.push(
            AuxItem::new(format!("{} 条消息", args.item_count), Tone::Neutral).icon("i-chat"),
        );
    }
    if !args.workspace_label.is_empty() {
        context.push(AuxItem::new(args.workspace_label, Tone::Neutral).icon("i-folder"));
    }

    let mut actions = Vec::new();
    // 断线时动作按钮一律不出现：它们只会再失败一次。
    if args.connected {
        if args.running {
            actions.push(
                Button::new("中止")
                    .variant(ButtonVariant::Danger)
                    .size(Size::Sm)
                    .icon("i-stop")
                    .action(format!("/threads/{}/abort", args.thread_id))
                    .attrs("data-confirm=\"确定中止当前轮次吗？\"")
                    .class("ada-aux__action"),
            );
        }
        if args.queue_len > 0 {
            actions.push(
                Button::new("清空队列")
                    .variant(ButtonVariant::Ghost)
                    .size(Size::Sm)
                    .icon("i-trash")
                    .action(format!("/threads/{}/queue/clear", args.thread_id))
                    .attrs("data-confirm=\"确定清空当前会话的排队消息吗？\"")
                    .class("ada-aux__action"),
            );
        }
        actions.push(
            Button::new("重试上一轮")
                .variant(ButtonVariant::Ghost)
                .size(Size::Sm)
                .icon("i-refresh")
                .action(format!("/threads/{}/retry", args.thread_id))
                .class("ada-aux__action"),
        );
    }

    ComposerAux {
        status,
        context,
        actions,
        hint: Some("Enter 发送 · Shift+Enter 换行".to_string()),
        plugins: slots.composer_footer.clone(),
    }
}

fn composer_aux_html(slots: &SlotContent, args: &ComposerArgs<'_>) -> String {
    composer_aux(slots, args).render()
}

/// 顶栏的状态药丸（运行中 / 审批档位 / 未连接）。
fn topbar_status_html(
    running: bool,
    connected: bool,
    approval: &str,
    approval_tone: &str,
) -> String {
    let mut out = String::new();
    if running {
        out.push_str(
            r#"<span class="ada-chip ada-chip--warn"><span class="ada-dot ada-dot--warn"></span>运行中</span>"#,
        );
    }
    if connected {
        let tone = match approval_tone {
            "danger" => Tone::Danger,
            "warn" => Tone::Warn,
            _ => Tone::Neutral,
        };
        out.push_str(&Chip::new(approval, tone).title("审批档位").render());
    } else {
        out.push_str(
            &Chip::new("未连接", Tone::Danger)
                .title("无法连接 agent 网关")
                .render(),
        );
    }
    out
}

fn approval_chip_html(label: &str, tone: &str) -> String {
    let tone = match tone {
        "danger" => Tone::Danger,
        "warn" => Tone::Warn,
        _ => Tone::Neutral,
    };
    Chip::new(label, tone)
        .icon("i-shield")
        .title("审批档位：在桌面端设置中调整")
        .class("ada-chip--approval")
        .render()
}

fn model_chip_html(model: &str) -> String {
    Chip::new(model, Tone::Neutral)
        .icon("i-sparkles")
        .title(format!("当前模型：{model}（在桌面端切换）"))
        .class("ada-chip--model")
        .render()
}

/// 顶栏右侧的刷新 / 停止（插件插槽在模板里单独保留）。
fn topbar_actions_html(thread_id: &str, running: bool) -> String {
    let mut out = IconButton::new("i-refresh", "刷新")
        .href(format!("/threads/{thread_id}"))
        .render();
    if running {
        out.push_str(
            &Button::new("停止")
                .variant(ButtonVariant::Danger)
                .size(Size::Sm)
                .icon("i-stop")
                .action(format!("/threads/{thread_id}/abort"))
                .attrs("data-confirm=\"确定中止当前轮次吗？\"")
                .render(),
        );
    }
    out
}

/// 插槽内容在 SSR 期注入：插件贡献的 HTML 直接进页面，
/// 插件失败时该插槽退回默认实现（空），主应用不受影响。
fn slot_content(registry: &PluginRegistry) -> SlotContent {
    let pick = |slot: Slot| registry.render_slot(slot).html;
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
fn action_failed(thread_id: &str, prefix: &str, error: anyhow::Error) -> Response {
    tracing::warn!("{prefix}: {error:#}");
    let message = format!("{prefix}：{}", public_reason(&error));
    redirect(&format!(
        "/threads/{thread_id}?error={}",
        encode_query(&message)
    ))
}

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

fn tool_status_label(raw: &str) -> String {
    match raw {
        "waiting_approval" => "等待批准",
        "awaiting" => "待答复",
        "running" => "执行中",
        "done" | "completed" | "success" => "完成",
        "error" | "failed" => "失败",
        "timeout" => "已超时",
        "queued" => "排队中",
        "" => "",
        other => other,
    }
    .to_string()
}

/// 工具状态的语义色：warn（等待批准）/ brand（执行中、待答复）/ danger（失败）。
///
/// 完成态不给色——默认就是"完成"，到处挂绿标反而吵。
fn tool_status_tone(raw: &str) -> String {
    match raw {
        "waiting_approval" => "warn",
        "awaiting" | "running" | "queued" => "brand",
        "error" | "failed" | "timeout" => "danger",
        _ => "",
    }
    .to_string()
}

/// 工具名 → 界面中文标签。
///
/// 与 desktop-ui 的 `TOOL_LABEL`（`desktop-ui/src/utils/tool-labels.ts`）保持同一套文案：
/// 工具名真源在 agent 的注册表里，这里只做"能查到就翻译，查不到就显示原名"。
fn tool_label(name: &str) -> String {
    match name {
        "list_files" => "列出文件",
        "read_file" => "读取文件",
        "search_files" => "搜索代码",
        "write_file" => "写入文件",
        "edit_file" => "修改文件",
        "run_command" => "执行命令",
        "run_background" => "后台命令",
        "check_task" => "查看后台任务",
        "kill_task" => "停止后台任务",
        "todo" => "任务规划",
        "invoke_subagent" => "委派子智能体",
        "ask_user" => "向用户提问",
        "git" => "Git 操作",
        "code_outline" => "代码大纲",
        "run_tests" => "运行测试",
        "batch_write" => "批量写入",
        "batch_replace" => "批量替换",
        "project_inspect" => "项目诊断",
        "create_plugin" => "创建插件",
        "news_query" => "查询资讯",
        "weather" => "查询天气",
        "finish" => "结束回合",
        "" => "工具",
        other => other,
    }
    .to_string()
}

/// 从调用参数里抽出"这个工具在动什么"的主文本。
fn args_text(args: &Value) -> String {
    if let Some(text) = args.as_str() {
        return text.replace(['\r', '\n'], " ").trim().to_string();
    }
    for key in [
        "path", "file", "filePath", "command", "cmd", "task", "query", "keyword", "pattern", "url",
        "question", "message", "id",
    ] {
        if let Some(value) = args.get(key).and_then(Value::as_str) {
            if !value.trim().is_empty() {
                return value.replace(['\r', '\n'], " ").trim().to_string();
            }
        }
    }
    String::new()
}

/// 参数归一化：agent 有时把参数写成 JSON 字符串（`rawArgs`），这里统一成对象。
fn normalize_args(value: Value) -> Value {
    if let Some(text) = value.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if parsed.is_object() {
                return parsed;
            }
        }
    }
    value
}

/// 参数的可读化展示：对象按 JSON 美化，字符串尽量解析后美化。
fn args_display(args: &Value) -> String {
    if args.is_null() {
        return String::new();
    }
    if let Some(text) = args.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if parsed.is_object() {
                return serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| text.to_string());
            }
        }
        return text.to_string();
    }
    serde_json::to_string_pretty(args).unwrap_or_default()
}

/// 工具结果的展示：结果条目列表优先铺成"标题 + 来源"，其余按 JSON 美化。
///
/// 资讯类工具的结果是几十条新闻的 JSON 数组，直接甩给用户读没有意义；
/// 展平成标题列表才看得懂"它到底查到了什么"。
fn details_display(details: &Value) -> String {
    if let Some(items) = details.get("items").and_then(Value::as_array) {
        let mut lines: Vec<String> = Vec::new();
        for entry in items.iter().take(8) {
            let title = entry
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if title.is_empty() {
                continue;
            }
            let source = entry
                .get("source")
                .or_else(|| entry.get("sourceLabel"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if source.is_empty() {
                lines.push(title.to_string());
            } else {
                lines.push(format!("{title}  —  {source}"));
            }
        }
        if !lines.is_empty() {
            if items.len() > lines.len() {
                lines.push(format!("（共 {} 条）", items.len()));
            }
            return lines.join("\n");
        }
    }
    serde_json::to_string_pretty(details).unwrap_or_default()
}

/// 路径类工具：把路径拆成"文件名 + 目录"，目录更淡、文件名更实。
const PATH_TOOLS: [&str; 5] = [
    "read_file",
    "write_file",
    "edit_file",
    "list_files",
    "code_outline",
];

/// 工具目标对象（文件名 / 命令 / 查询词）与它的目录部分。
///
/// 对齐 desktop-ui `toolTarget` 的思路：`git` 用 action 翻译成人话，
/// 路径类工具拆成"文件名 + 目录"，其余直接取参数里的主字段。
fn tool_target(name: &str, args: &Value) -> (String, String) {
    if name == "git" {
        let action = {
            let raw = str_at(args, "action").to_lowercase();
            if raw.is_empty() {
                "status".to_string()
            } else {
                raw
            }
        };
        let file = str_at(args, "file");
        let path = str_at(args, "path");
        let reference = str_at(args, "ref");
        let branch = str_at(args, "branch");
        let message = str_at(args, "message");
        let file = if file.is_empty() { path } else { file };
        let target = match action.as_str() {
            "status" => "查看仓库改动".to_string(),
            "diff" => {
                if file.is_empty() {
                    "查看未暂存差异".to_string()
                } else {
                    format!("查看 {file} 的改动")
                }
            }
            "log" => "近期提交".to_string(),
            "show" => format!("查看提交 {}", pick(reference, "HEAD")),
            "blame" => format!("追溯 {} 的逐行作者", pick(file, "文件")),
            "add" => "暂存改动".to_string(),
            "commit" => {
                if message.is_empty() {
                    "创建提交".to_string()
                } else {
                    format!("提交：{}", short_title(message, 40))
                }
            }
            "checkout" => format!("切换到 {}", pick(branch, "分支")),
            "restore" => format!("恢复 {}", pick(file, "文件")),
            "reset" => "重置到 HEAD".to_string(),
            "stash" => "stash 列表".to_string(),
            "branch" => "列出分支".to_string(),
            "merge" => format!("合并 {}", pick(branch, "分支")),
            "push" => "推送到远端".to_string(),
            "pull" => "拉取远端".to_string(),
            other => format!("git {other}"),
        };
        return (target, String::new());
    }
    match name {
        "run_tests" => return ("执行测试套件".to_string(), String::new()),
        "project_inspect" => return ("探测技术栈与项目环境".to_string(), String::new()),
        "create_plugin" => {
            let id = str_at(args, "id");
            return (
                if id.is_empty() {
                    "生成插件骨架".to_string()
                } else {
                    format!("插件 {id}")
                },
                String::new(),
            );
        }
        "batch_write" | "batch_replace" => {
            let count = array_at(args, "files").len();
            return (
                if name == "batch_write" {
                    format!("写入 {count} 个文件")
                } else {
                    format!("在 {count} 个文件中替换")
                },
                String::new(),
            );
        }
        "invoke_subagent" => {
            let role = {
                let raw = str_at(args, "subagent_id");
                if raw.is_empty() {
                    str_at(args, "role")
                } else {
                    raw
                }
            };
            let role = if role.is_empty() {
                "子智能体"
            } else {
                role
            };
            let task = short_title(str_at(args, "task"), 40);
            return (
                if task.is_empty() {
                    format!("[{role}]")
                } else {
                    format!("[{role}] {task}")
                },
                String::new(),
            );
        }
        _ => {}
    }

    let summary = args_text(args);
    if summary.is_empty() {
        return (String::new(), String::new());
    }
    if !PATH_TOOLS.contains(&name) {
        return (summary, String::new());
    }
    let normalized = summary.replace('\\', "/");
    match normalized.rfind('/') {
        Some(cut) => (
            normalized[cut + 1..].to_string(),
            normalized[..=cut].to_string(),
        ),
        None => (normalized, String::new()),
    }
}

fn pick<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() {
        fallback
    } else {
        value
    }
}

/// 工具图标 key：与 desktop-ui `renderToolIcon` 的同一套语义。
fn tool_icon(name: &str) -> String {
    match name {
        "list_files" => "folder",
        "read_file" | "write_file" | "edit_file" | "batch_write" | "code_outline" => "file",
        "batch_replace" => "edit",
        "search_files" | "project_inspect" => "search",
        "create_plugin" => "package",
        "find_symbol" => "code",
        "run_command" | "run_background" => "terminal",
        "run_tests" => "check",
        "check_task" => "clock",
        "kill_task" => "xcircle",
        "todo" => "list",
        "invoke_subagent" | "check_subagent" => "bot",
        "ask_user" => "sparkles",
        "git" => "git",
        _ => "terminal",
    }
    .to_string()
}

/// 思考文本的末行预览：折叠态单行显示"它在想什么"。
fn last_line_preview(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("");
    if line.chars().count() > 42 {
        let mut out: String = line.chars().take(42).collect();
        out.push('…');
        out
    } else {
        line.to_string()
    }
}

/// 持续时长文案。
///
/// - 工具：`durationMs` 是真实耗时，按 desktop-ui 的做法显示毫秒值；
/// - 思考：只有起止时间差 ≥1 秒才显示——恢复出来的历史会话里 `at` 与
///   `endedAt` 往往是同一个时间戳，显示"0 秒"只是噪声，不如不显示。
fn elapsed_label(value: &Value) -> String {
    if let Some(ms) = value.get("durationMs").and_then(Value::as_u64) {
        return format!("{ms}ms");
    }
    let start = value
        .get("at")
        .and_then(Value::as_u64)
        .or_else(|| value.get("createdAt").and_then(Value::as_u64));
    let end = value
        .get("endedAt")
        .and_then(Value::as_u64)
        .or_else(|| value.get("ended_at").and_then(Value::as_u64));
    match (start, end) {
        (Some(start), Some(end)) if end.saturating_sub(start) >= 1000 => {
            let secs = (end - start) / 1000;
            if secs >= 60 {
                format!("{} 分 {} 秒", secs / 60, secs % 60)
            } else {
                format!("{secs} 秒")
            }
        }
        _ => String::new(),
    }
}

/// 压缩卡片的正文是 `summary`（不是 `text`），并带上真实的 token 变化。
fn compact_text(value: &Value) -> String {
    let summary = str_at(value, "summary");
    let pre = value.get("preTokens").and_then(Value::as_u64);
    let post = value.get("postTokens").and_then(Value::as_u64);
    match (pre, post) {
        (Some(pre), Some(post)) => {
            format!("{summary}\n\n上下文 {pre} → {post} tokens")
        }
        _ => summary.to_string(),
    }
}

/// 把 agent 快照里的一条 item 归一化成组件库的行视图。
fn row_view(value: &Value) -> ui::RowView {
    let kind = str_at(value, "kind").to_string();
    let tool_name = str_at(value, "name").to_string();
    let raw_text = if kind == "compact" {
        compact_text(value)
    } else {
        str_at(value, "text").to_string()
    };
    let images = value
        .get("images")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let key = str_at(value, "id").to_string();
    let raw_output = value
        .get("output")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            // 工具结果也可能落在 `details`（结构化对象）里。
            value
                .get("details")
                .map(details_display)
                .unwrap_or_default()
        });
    let text = truncate(&raw_text, 8000);
    let output = truncate(&raw_output, 4000);
    let html = if kind == "assistant" {
        crate::markdown::to_html(&raw_text)
    } else {
        String::new()
    };
    let label = match kind.as_str() {
        "user" => "你".to_string(),
        "assistant" => "助手".to_string(),
        "thinking" => "思考".to_string(),
        "tool" => tool_label(&tool_name),
        "notice" => "提示".to_string(),
        "compact" => "压缩".to_string(),
        other => other.to_string(),
    };
    let raw_status = str_at(value, "status");
    let args_value = value
        .get("args")
        .cloned()
        .or_else(|| value.get("rawArgs").cloned())
        .map(normalize_args)
        .unwrap_or(Value::Null);
    let (target, dir) = tool_target(&tool_name, &args_value);
    let preview = if kind == "thinking" {
        last_line_preview(&text)
    } else {
        String::new()
    };
    // 工具输出：JSON 走等宽原文，其它按 Markdown 渲染（链接自动成链、列表有排版）。
    let output_html = if looks_like_json(&output) {
        String::new()
    } else {
        crate::markdown::to_html(&output)
    };
    // todo 工具：把任务清单读出来，工具行展开时直接排版显示。
    let (todo_steps, todo_notes) = if tool_name == "todo" {
        ui::read_todo_items(value).unwrap_or_default()
    } else {
        (Vec::new(), None)
    };
    let status_tone = match tool_status_tone(raw_status).as_str() {
        "warn" => Tone::Warn,
        "brand" => Tone::Brand,
        "danger" => Tone::Danger,
        "success" => Tone::Success,
        _ => Tone::Neutral,
    };
    ui::RowView {
        kind,
        key,
        images,
        text,
        html,
        label,
        target,
        dir,
        icon: tool_icon(&tool_name),
        status: tool_status_label(raw_status),
        status_tone,
        elapsed: elapsed_label(value),
        args: truncate(&args_display(&args_value), 2000),
        output,
        output_html,
        preview,
        level: str_at(value, "level").to_string(),
        todo_steps,
        todo_notes,
    }
}

/// 渲染一行对话（组件库）。
fn item_row(value: &Value, thread_id: &str) -> ItemRow {
    ItemRow {
        html: ui::render_row(&row_view(value), thread_id),
    }
}

/// 输出看起来是不是 JSON（对象或数组）——是就用等宽原文展示，不做 Markdown 解析。
fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim_start();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return false;
    }
    serde_json::from_str::<Value>(trimmed).is_ok()
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
fn model_of(snapshot: &Value, config: &Value) -> String {
    snapshot
        .get("config")
        .and_then(|value| value.get("model"))
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .or_else(|| {
            config
                .get("saved")
                .and_then(|saved| saved.get("model"))
                .and_then(Value::as_str)
        })
        .or_else(|| config.get("model").and_then(Value::as_str))
        .unwrap_or("未配置")
        .to_string()
}

/// 审批档位的真实字段是 `snapshot.config.approval`。
///
/// 历史实现找的是 `approvalMode`（不存在的字段），所以"完全访问"一直显示成
/// "自动批准"、危险档也不显红——这是安全信息，不能靠猜。
fn approval_mode(snapshot: &Value, config: &Value) -> String {
    snapshot
        .get("config")
        .and_then(|cfg| cfg.get("approval"))
        .or_else(|| config.get("saved").and_then(|saved| saved.get("approval")))
        .or_else(|| config.get("approval"))
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_string()
}

/// 审批档位的中文标签（与 desktop-ui `APPROVAL_OPTIONS` 同一套文案）。
fn approval_label(mode: &str) -> String {
    match mode {
        "full" | "yolo" => "完全访问",
        "ask" | "manual" => "每次询问",
        "readonly" => "严格只读",
        "auto" | "" => "自动批准",
        other => other,
    }
    .to_string()
}

/// 审批档位的语义色：完全访问是危险档（红），每次询问是提醒档（琥珀）。
fn approval_tone(mode: &str) -> String {
    match mode {
        "full" | "yolo" => "danger",
        "ask" | "manual" | "readonly" => "warn",
        _ => "neutral",
    }
    .to_string()
}

/// 工作区短名：路径最后一段（`E:/code/app` → `app`），供输入框展示。
fn workspace_label(workspace: &str) -> String {
    let trimmed = workspace.trim().trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

fn waited_label(value: &Value) -> String {
    let at = value.get("at").and_then(Value::as_u64).unwrap_or(0);
    if at == 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(at);
    let seconds = now.saturating_sub(at) / 1000;
    if seconds < 60 {
        format!("已等待 {seconds}s")
    } else if seconds < 3600 {
        format!("已等待 {}m {:02}s", seconds / 60, seconds % 60)
    } else if seconds < 86400 {
        format!("已等待 {}h {:02}m", seconds / 3600, (seconds % 3600) / 60)
    } else {
        format!("已等待 {}d {}h", seconds / 86400, (seconds % 86400) / 3600)
    }
}

fn approval_rows(raw_items: &[Value]) -> Vec<ApprovalRow> {
    let mut rows = Vec::new();
    for raw in raw_items.iter() {
        // 用原始 status 判定；渲染后的行里 status 已经中文化了。
        if str_at(raw, "kind") != "tool" || str_at(raw, "status") != "waiting_approval" {
            continue;
        }
        let call_id = str_at(raw, "callId").to_string();
        if call_id.is_empty() {
            continue;
        }
        let args = raw
            .get("rawArgs")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                raw.get("args")
                    .map(|args| serde_json::to_string_pretty(args).unwrap_or_default())
            })
            .unwrap_or_default();
        let raw_name = str_at(raw, "name");
        rows.push(ApprovalRow {
            call_id,
            tool_name: if raw_name.is_empty() {
                "工具".to_string()
            } else {
                tool_label(raw_name)
            },
            args: truncate(&args, 1200),
            waited: waited_label(raw),
        });
    }
    rows
}

fn queue_rows(snapshot: &Value, thread_id: &str) -> Vec<QueueRow> {
    array_at(snapshot, "queue")
        .iter()
        .enumerate()
        .filter(|(_, item)| str_at(item, "threadId") == thread_id)
        .map(|(index, item)| QueueRow {
            index,
            order: index + 1,
            text: truncate(str_at(item, "text"), 240),
        })
        .collect()
}

/// 渲染好的对话片段（SSR 与 live 共用）。
fn transcript_html(items: Vec<ItemRow>) -> String {
    render_string(&TranscriptPartial {
        items,
        transcript_empty: ui::render_empty_thread(),
    })
}

/// 侧栏会话列表（SSR 与 live 共用）。
fn thread_rows(snapshot: &Value, active_thread_id: &str) -> Vec<ThreadRow> {
    let running = running_ids(snapshot);
    // 侧栏只保留有内容的会话，以及当前会话/正在运行的会话：
    // agent 会先落一个空“新会话”占位，全部列出来会淹没真正有内容的会话。
    array_at(snapshot, "threads")
        .iter()
        .map(|value| thread_row(value, &running))
        .filter(|thread| thread.item_count > 0 || thread.id == active_thread_id || thread.running)
        .collect()
}

/// 审批 + 排队托盘。返回 `(html, 待批准数, 排队数)`。
fn trays_html(snapshot: &Value, raw_items: &[Value], thread_id: &str) -> (String, usize, usize) {
    let approvals = approval_rows(raw_items);
    let approvals_html = approvals
        .iter()
        .map(|row| {
            ui::ApprovalTray {
                call_id: row.call_id.clone(),
                thread_id: thread_id.to_string(),
                tool_name: row.tool_name.clone(),
                args: row.args.clone(),
                waited: row.waited.clone(),
            }
            .render()
        })
        .collect::<Vec<_>>()
        .join("");
    let queue = queue_rows(snapshot, thread_id);
    let queue_html = if queue.is_empty() {
        String::new()
    } else {
        ui::QueueTray {
            items: queue
                .iter()
                .map(|row| ui::QueueItem {
                    index: row.index,
                    order: row.order,
                    text: row.text.clone(),
                    thread_id: thread_id.to_string(),
                })
                .collect(),
        }
        .render()
    };
    (
        format!("{approvals_html}{queue_html}"),
        approvals.len(),
        queue.len(),
    )
}

/// 任务规划：按 set/update/get 的动作推演当前计划（不是“倒序找最后一张卡片”）。
fn todo_html(raw_items: &[Value]) -> String {
    ui::derive_todo_plan(raw_items)
        .map(|plan| {
            ui::TodoPanel {
                plan,
                dom_id: "ada-todo".to_string(),
            }
            .render()
        })
        .unwrap_or_default()
}

fn render_string<T: Template>(template: &T) -> String {
    template.render().unwrap_or_else(|error| {
        tracing::error!("模板渲染失败: {error}");
        String::new()
    })
}

/// live 轮询要替换的各个区域。
struct LiveParts {
    transcript: String,
    trays: String,
    todo: String,
    topbar_status: String,
    topbar_actions: String,
    aux: String,
    approval_chip: String,
    model_chip: String,
}

fn live_parts(state: &AppState, snapshot: &Value, thread_id: &str, thread: &Value) -> LiveParts {
    let raw_items = thread
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let items: Vec<ItemRow> = raw_items
        .iter()
        .map(|item| item_row(item, thread_id))
        .collect();
    let item_count = items.len();
    let (trays, approval_count, queue_len) = trays_html(snapshot, raw_items, thread_id);
    let running = running_ids(snapshot).iter().any(|id| id == thread_id);
    let mode = approval_mode(snapshot, &Value::Null);
    let approval = approval_label(&mode);
    let tone = approval_tone(&mode);
    let model = model_of(snapshot, &Value::Null);
    let workspace = str_at(thread, "workspace");
    let (workspace_label, workspace_path) = if workspace.is_empty() {
        (
            workspace_label(&state.agent.workspace),
            state.agent.workspace.as_str(),
        )
    } else {
        (workspace_label(workspace), workspace)
    };
    // 这里只换辅助区，只需要 composer.footer 一个插槽；不必把十个插槽都渲染一遍。
    let slots = SlotContent {
        composer_footer: state.frontend.render_slot(Slot::ComposerFooter).html,
        ..SlotContent::default()
    };
    let args = ComposerArgs {
        thread_id,
        model: &model,
        approval: &approval,
        approval_tone: &tone,
        workspace_label: &workspace_label,
        workspace_path,
        connected: true,
        running,
        queue_len,
        approval_count,
        item_count,
    };
    LiveParts {
        transcript: transcript_html(items),
        trays,
        todo: todo_html(raw_items),
        topbar_status: topbar_status_html(running, true, &approval, &tone),
        topbar_actions: topbar_actions_html(thread_id, running),
        aux: composer_aux_html(&slots, &args),
        approval_chip: approval_chip_html(&approval, &tone),
        model_chip: model_chip_html(&model),
    }
}

async fn load_chat(
    state: &AppState,
    forced_thread: Option<String>,
    query: &PageQuery,
) -> Result<ChatPage, AppError> {
    let snapshot = match state.agent.snapshot().await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            // 连接失败也要给一个可读的壳，而不是直接 502：
            // 用户至少要知道“没连上”，并能看到重试/刷新入口。
            // 如果这次是某个动作失败后跳回来的，优先显示动作原因（那才是用户刚做的事）。
            let flash = flash_from_query(query).unwrap_or_else(|| Flash {
                kind: "error".to_string(),
                message: format!("无法连接 agent 网关：{}", public_reason(&error)),
            });
            return Ok(ChatPage {
                base: base(state, "未连接", "chat", Some(flash)),
                threads: Vec::new(),
                active: None,
                active_thread_id: String::new(),
                items: Vec::new(),
                transcript_empty: ui::render_empty_thread(),
                trays_html: String::new(),
                composer_html: String::new(),
                todo_html: String::new(),
                topbar_status: topbar_status_html(false, false, "未连接", "neutral"),
                topbar_actions: String::new(),
                connected: false,
            });
        }
    };
    let running = running_ids(&snapshot);
    let thread_values = array_at(&snapshot, "threads");
    // 显式的深链接不存在时如实 404，而不是悄悄显示“第一条会话”。
    if let Some(id) = forced_thread.as_deref() {
        if !thread_values.iter().any(|value| str_at(value, "id") == id) {
            return Err(AppError::new(
                StatusCode::NOT_FOUND,
                format!("会话不存在: {id}"),
            ));
        }
    }
    let wanted = forced_thread.or_else(|| query.thread.clone()).or_else(|| {
        snapshot
            .get("activeThreadId")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let active_value = wanted
        .as_deref()
        .and_then(|id| thread_values.iter().find(|value| str_at(value, "id") == id))
        .or_else(|| thread_values.first());
    let active = active_value.map(|value| thread_row(value, &running));
    let active_thread_id = active
        .as_ref()
        .map(|thread| thread.id.clone())
        .unwrap_or_default();
    let threads = thread_rows(&snapshot, &active_thread_id);
    let raw_items: &[Value] = active_value
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let items: Vec<ItemRow> = raw_items
        .iter()
        .map(|item| item_row(item, &active_thread_id))
        .collect();
    let item_count = items.len();
    let (trays, approval_count, queue_len) = trays_html(&snapshot, raw_items, &active_thread_id);

    let config = state.agent.config().await.unwrap_or(Value::Null);
    let model = model_of(&snapshot, &config);
    let mode = approval_mode(&snapshot, &config);
    let approval = approval_label(&mode);
    let approval_tone = approval_tone(&mode);
    let title = active
        .as_ref()
        .map(|thread| thread.title.clone())
        .unwrap_or_else(|| "新对话".to_string());
    let workspace_label = active
        .as_ref()
        .map(|thread| workspace_label(&thread.workspace))
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| workspace_label(&state.agent.workspace));
    let workspace_path = active
        .as_ref()
        .map(|thread| thread.workspace.clone())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| state.agent.workspace.clone());
    let is_running = running.contains(&active_thread_id);
    let slots = slot_content(&state.frontend);

    // 没有活动会话时不给输入框：提交到 `/threads//send` 只会得到一个错误。
    let composer_html = if active_thread_id.is_empty() {
        String::new()
    } else {
        composer_html(
            &slots,
            &ComposerArgs {
                thread_id: &active_thread_id,
                model: &model,
                approval: &approval,
                approval_tone: &approval_tone,
                workspace_label: &workspace_label,
                workspace_path: &workspace_path,
                connected: true,
                running: is_running,
                queue_len,
                approval_count,
                item_count,
            },
        )
    };

    Ok(ChatPage {
        base: base(state, &title, "chat", flash_from_query(query)),
        threads,
        active,
        active_thread_id: active_thread_id.clone(),
        items,
        transcript_empty: ui::render_empty_thread(),
        trays_html: trays,
        composer_html,
        todo_html: todo_html(raw_items),
        topbar_status: topbar_status_html(is_running, true, &approval, &approval_tone),
        topbar_actions: topbar_actions_html(&active_thread_id, is_running),
        connected: true,
    })
}

/// `hx-swap-oob` 片段：把这块 HTML 写进页面里同 id 的元素。
fn oob(id: &str, html: &str) -> String {
    format!(r#"<div id="{id}" hx-swap-oob="innerHTML">{html}</div>"#)
}

fn no_store_html(html: String) -> Response {
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        Html(html),
    )
        .into_response()
}

/// 网关断开时的 live 响应：只更新“未连接”状态，**不碰**对话流内容。
fn live_offline(state: &AppState, thread_id: &str) -> Response {
    let label = workspace_label(&state.agent.workspace);
    let slots = SlotContent {
        composer_footer: state.frontend.render_slot(Slot::ComposerFooter).html,
        ..SlotContent::default()
    };
    let args = ComposerArgs {
        thread_id,
        model: "未连接",
        approval: "未连接",
        approval_tone: "neutral",
        workspace_label: &label,
        workspace_path: &state.agent.workspace,
        connected: false,
        running: false,
        queue_len: 0,
        approval_count: 0,
        item_count: 0,
    };
    let body = [
        oob(
            "topbar-status",
            &topbar_status_html(false, false, "未连接", "neutral"),
        ),
        oob("topbar-actions", &topbar_actions_html(thread_id, false)),
        oob("composer-aux", &composer_aux_html(&slots, &args)),
        oob(
            "composer-approval",
            &approval_chip_html("未连接", "neutral"),
        ),
        oob("composer-model", &model_chip_html("未连接")),
    ]
    .concat();
    no_store_html(body)
}

/// 每 2 秒的局部刷新：一个响应里带回所有动态区域。
///
/// 为什么不是只刷新对话流：审批按钮、运行状态、任务规划都在对话流之外，
/// 只刷新 transcript 会让"等待批准"永久停在页面上（只能手工 F5 才出现按钮）。
pub async fn live(State(state): State<AppState>, Path(thread_id): Path<String>) -> Response {
    let snapshot = match state.agent.snapshot().await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            // 轮询每 2 秒一次：连接断开时不要刷爆日志，状态已经通过页面告诉用户。
            tracing::debug!("live 读取网关失败: {error:#}");
            return live_offline(&state, &thread_id);
        }
    };
    let Some(thread) = array_at(&snapshot, "threads")
        .iter()
        .find(|value| str_at(value, "id") == thread_id)
    else {
        return (StatusCode::NOT_FOUND, "会话不存在").into_response();
    };
    let parts = live_parts(&state, &snapshot, &thread_id, thread);
    let body = [
        oob("transcript", &parts.transcript),
        oob("ada-trays", &parts.trays),
        oob("ada-todo-dock", &parts.todo),
        oob("topbar-status", &parts.topbar_status),
        oob("topbar-actions", &parts.topbar_actions),
        oob("composer-aux", &parts.aux),
        oob("composer-approval", &parts.approval_chip),
        oob("composer-model", &parts.model_chip),
    ]
    .concat();
    no_store_html(body)
}

pub async fn chat(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let page = load_chat(&state, None, &query).await?;
    Ok(render(&page)?.into_response())
}

pub async fn chat_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let page = load_chat(&state, Some(thread_id), &query).await?;
    Ok(render(&page)?.into_response())
}

pub async fn transcript_partial(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Response, AppError> {
    let snapshot =
        state.agent.snapshot().await.map_err(|error| {
            AppError::upstream(format!("读取会话失败: {}", public_reason(&error)))
        })?;
    let value = array_at(&snapshot, "threads")
        .iter()
        .find(|value| str_at(value, "id") == thread_id)
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, format!("会话不存在: {thread_id}")))?;
    let items: Vec<ItemRow> = value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item_row(item, &thread_id))
                .collect()
        })
        .unwrap_or_default();
    Ok(render(&TranscriptPartial {
        items,
        transcript_empty: ui::render_empty_thread(),
    })?
    .into_response())
}

async fn dashboard_view(state: &AppState) -> Result<DashboardView, AppError> {
    let snapshot = state.agent.snapshot().await.map_err(|error| {
        AppError::upstream(format!("读取 agent 快照失败: {}", public_reason(&error)))
    })?;
    let running = running_ids(&snapshot);
    let threads: Vec<ThreadRow> = array_at(&snapshot, "threads")
        .iter()
        .map(|value| thread_row(value, &running))
        .collect();
    let recent = threads.iter().take(8).cloned().collect();

    let config = state.agent.config().await.unwrap_or(Value::Null);
    let providers = state.agent.providers().await.unwrap_or(Value::Null);
    let model = config
        .get("saved")
        .and_then(|saved| saved.get("model"))
        .and_then(Value::as_str)
        .or_else(|| config.get("model").and_then(Value::as_str))
        .unwrap_or("未配置");
    let provider_count = array_at(&providers, "providers").len();
    let frontend_count = state.frontend.list().len();

    Ok(DashboardView {
        stats: vec![
            Stat {
                label: "会话".to_string(),
                value: threads.len().to_string(),
                hint: "本地会话总数".to_string(),
                tone: "brand".to_string(),
            },
            Stat {
                label: "运行中".to_string(),
                value: running.len().to_string(),
                hint: "当前有轮次在执行".to_string(),
                tone: if running.is_empty() { "muted" } else { "warn" }.to_string(),
            },
            Stat {
                label: "模型".to_string(),
                value: model.to_string(),
                hint: format!("{} 个供应商", provider_count),
                tone: "muted".to_string(),
            },
            Stat {
                label: "前端插件".to_string(),
                value: frontend_count.to_string(),
                hint: "运行时扫描 frontend-plugins/".to_string(),
                tone: "brand".to_string(),
            },
        ],
        widgets: state.frontend.dashboard_widgets(),
        recent,
    })
}

pub async fn dashboard(State(state): State<AppState>) -> Result<Response, AppError> {
    let view = dashboard_view(&state).await?;
    let page = DashboardPage {
        base: base(&state, "概览", "dashboard", None),
        view,
    };
    Ok(render(&page)?.into_response())
}

pub async fn dashboard_partial(State(state): State<AppState>) -> Result<Response, AppError> {
    let view = dashboard_view(&state).await?;
    Ok(render(&DashboardPartial {
        view,
        base: base(&state, "概览", "dashboard", None),
    })?
    .into_response())
}

pub async fn create_thread(
    State(state): State<AppState>,
    Form(form): Form<CreateThreadForm>,
) -> Result<Response, AppError> {
    let title = form.title.trim();
    if title.is_empty() {
        return Err(AppError::bad("会话标题不能为空"));
    }
    let result = match state.agent.create_thread(title).await {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!("创建会话失败: {error:#}");
            let message = format!("创建会话失败：{}", public_reason(&error));
            return Ok(redirect(&format!("/?error={}", encode_query(&message))));
        }
    };
    let Some(id) = result
        .get("threadId")
        .and_then(Value::as_str)
        .or_else(|| result.get("id").and_then(Value::as_str))
    else {
        return Ok(redirect(&format!(
            "/?error={}",
            encode_query("创建会话失败：网关没有返回新会话 id")
        )));
    };
    Ok(redirect(&format!("/threads/{id}")))
}

pub async fn send_message(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Form(form): Form<SendForm>,
) -> Result<Response, AppError> {
    let text = form.text.trim();
    if text.is_empty() {
        return Ok(redirect(&format!("/threads/{thread_id}")));
    }
    match state.agent.send_message(&thread_id, text).await {
        Ok(_) => Ok(redirect(&format!("/threads/{thread_id}?notice=sent"))),
        Err(error) => Ok(action_failed(&thread_id, "发送消息失败", error)),
    }
}

pub async fn abort_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Response, AppError> {
    match state.agent.abort_thread(&thread_id).await {
        Ok(_) => Ok(redirect(&format!("/threads/{thread_id}?notice=aborted"))),
        Err(error) => Ok(action_failed(&thread_id, "中止失败", error)),
    }
}

pub async fn decide_approval(
    State(state): State<AppState>,
    Path(call_id): Path<String>,
    Form(form): Form<ApprovalForm>,
) -> Result<Response, AppError> {
    if call_id.trim().is_empty() {
        return Err(AppError::bad("审批缺少 callId"));
    }
    let approved = matches!(
        form.approved.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "on" | "yes"
    );
    match state.agent.decide_approval(&call_id, approved).await {
        Ok(_) => {
            let notice = if approved { "approved" } else { "rejected" };
            Ok(redirect(&format!(
                "/threads/{}?notice={notice}",
                form.thread_id
            )))
        }
        Err(error) => {
            if form.thread_id.trim().is_empty() {
                tracing::warn!("提交审批失败: {error:#}");
                let message = format!("提交审批失败：{}", public_reason(&error));
                Ok(redirect(&format!("/?error={}", encode_query(&message))))
            } else {
                Ok(action_failed(&form.thread_id, "提交审批失败", error))
            }
        }
    }
}

pub async fn retry_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Response, AppError> {
    match state.agent.retry_thread(&thread_id).await {
        Ok(_) => Ok(redirect(&format!("/threads/{thread_id}?notice=retried"))),
        Err(error) => Ok(action_failed(&thread_id, "重试失败", error)),
    }
}

pub async fn promote_queue(
    State(state): State<AppState>,
    Path((thread_id, index)): Path<(String, usize)>,
) -> Result<Response, AppError> {
    match state.agent.promote_queue(index, &thread_id).await {
        Ok(_) => Ok(redirect(&format!(
            "/threads/{thread_id}?notice=queue_promoted"
        ))),
        Err(error) => Ok(action_failed(&thread_id, "提升排队消息失败", error)),
    }
}

pub async fn remove_queue(
    State(state): State<AppState>,
    Path((thread_id, index)): Path<(String, usize)>,
) -> Result<Response, AppError> {
    match state.agent.remove_queue(index).await {
        Ok(_) => Ok(redirect(&format!(
            "/threads/{thread_id}?notice=queue_removed"
        ))),
        Err(error) => Ok(action_failed(&thread_id, "删除排队消息失败", error)),
    }
}

pub async fn clear_queue(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Response, AppError> {
    match state.agent.clear_queue(&thread_id).await {
        Ok(_) => Ok(redirect(&format!(
            "/threads/{thread_id}?notice=queue_cleared"
        ))),
        Err(error) => Ok(action_failed(&thread_id, "清空排队消息失败", error)),
    }
}

pub async fn providers(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let config = state.agent.config().await.unwrap_or(Value::Null);
    let values = state.agent.providers().await.unwrap_or(Value::Null);
    let active_id = values
        .get("activeProviderId")
        .and_then(Value::as_str)
        .or_else(|| config.get("activeProviderId").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let rows: Vec<ProviderRow> = array_at(&values, "providers")
        .iter()
        .map(|value| {
            let id = str_at(value, "id").to_string();
            let model = str_at(value, "model");
            let model = if model.is_empty() {
                value
                    .get("models")
                    .and_then(Value::as_array)
                    .and_then(|models| models.first())
                    .and_then(|model| model.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            } else {
                model.to_string()
            };
            ProviderRow {
                active: !active_id.is_empty() && active_id == id,
                id,
                name: str_at(value, "name").to_string(),
                base_url: str_at(value, "baseUrl").to_string(),
                model,
            }
        })
        .collect();
    let model = config
        .get("saved")
        .and_then(|saved| saved.get("model"))
        .and_then(Value::as_str)
        .or_else(|| config.get("model").and_then(Value::as_str))
        .unwrap_or("未配置")
        .to_string();
    let base_url = config
        .get("saved")
        .and_then(|saved| saved.get("baseUrl"))
        .and_then(Value::as_str)
        .or_else(|| config.get("baseUrl").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let page = ProvidersPage {
        base: base(&state, "供应商", "providers", flash_from_query(&query)),
        providers: rows,
        model,
        base_url,
    };
    Ok(render(&page)?.into_response())
}

fn frontend_rows(state: &AppState) -> Vec<FrontendPluginRow> {
    state
        .frontend
        .list()
        .into_iter()
        .map(|plugin| FrontendPluginRow {
            id: plugin.id,
            name: plugin.name,
            version: plugin.version,
            description: plugin.description,
            nav_count: plugin.nav.len(),
            page_count: plugin.pages.len(),
            slot_count: plugin.slots.len(),
            first_page: plugin.pages.first().map(|page| page.slug.clone()),
            has_dashboard: plugin.dashboard.is_some(),
            has_assets: plugin.assets_dir.is_some(),
            enabled: plugin.enabled,
            status: plugin.status.as_str().to_string(),
            errors: plugin.errors,
        })
        .collect()
}

fn agent_rows(value: &Value) -> Vec<AgentPluginRow> {
    array_at(value, "plugins")
        .iter()
        .map(|plugin| AgentPluginRow {
            id: str_at(plugin, "id").to_string(),
            name: str_at(plugin, "name").to_string(),
            scope: str_at(plugin, "scope").to_string(),
            enabled: plugin
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            status: str_at(plugin, "status").to_string(),
            tools: array_at(plugin, "tools").len(),
            error: str_at(plugin, "error").to_string(),
        })
        .collect()
}

pub async fn extensions(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let frontend = frontend_rows(&state);
    let agent_value = state.agent.plugins().await.unwrap_or(Value::Null);
    let agent = agent_rows(&agent_value);
    let page = ExtensionsPage {
        base: base(&state, "扩展", "extensions", flash_from_query(&query)),
        frontend_count: frontend.len(),
        agent_count: agent.len(),
        frontend,
        agent,
    };
    Ok(render(&page)?.into_response())
}

pub async fn reload_frontend(State(state): State<AppState>) -> Result<Response, AppError> {
    let report = match state.frontend.reload() {
        Ok(report) => report,
        Err(error) => {
            tracing::warn!("重载前端插件失败: {error:#}");
            let message = format!("重载前端插件失败：{}", public_reason(&error));
            return Ok(redirect(&format!(
                "/extensions?error={}",
                encode_query(&message)
            )));
        }
    };
    Ok(redirect(&format!(
        "/extensions?notice=frontend_reloaded&count={}&failed={}",
        report.total,
        report.failed.len()
    )))
}

/// 启用插件（热插拔：装载 → 挂载 → 立刻出现在页面上）。
pub async fn enable_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> Result<Response, AppError> {
    if let Err(error) = state.frontend.set_enabled(&plugin_id, true) {
        tracing::warn!("启用前端插件失败: {error:#}");
        let message = format!("启用插件失败：{}", public_reason(&error));
        return Ok(redirect(&format!(
            "/extensions?error={}",
            encode_query(&message)
        )));
    }
    Ok(redirect(&format!(
        "/extensions?notice=plugin_enabled&id={plugin_id}"
    )))
}

/// 停用插件（卸载：贡献消失，界面回到默认实现）。
pub async fn disable_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> Result<Response, AppError> {
    if let Err(error) = state.frontend.set_enabled(&plugin_id, false) {
        tracing::warn!("停用前端插件失败: {error:#}");
        let message = format!("停用插件失败：{}", public_reason(&error));
        return Ok(redirect(&format!(
            "/extensions?error={}",
            encode_query(&message)
        )));
    }
    Ok(redirect(&format!(
        "/extensions?notice=plugin_disabled&id={plugin_id}"
    )))
}

/// 插件状态快照：客户端与服务端看到同一份事实。
pub async fn plugins_api(State(state): State<AppState>) -> Result<Response, AppError> {
    let slots: Vec<serde_json::Value> = crate::plugins::Slot::all()
        .iter()
        .map(|slot| {
            let output = state.frontend.render_slot(*slot);
            serde_json::json!({
                "slot": slot.as_str(),
                "contributors": output.contributors,
                "failed": output.failed,
            })
        })
        .collect();
    let body = serde_json::json!({
        "plugins": state.frontend.snapshot(),
        "slots": slots,
    });
    Ok((StatusCode::OK, axum::Json(body)).into_response())
}

pub async fn reload_agent(State(state): State<AppState>) -> Result<Response, AppError> {
    if let Err(error) = state.agent.reload_agent_plugins().await {
        tracing::warn!("agent 重载插件失败: {error:#}");
        let message = format!("agent 重载插件失败：{}", public_reason(&error));
        return Ok(redirect(&format!(
            "/extensions?error={}",
            encode_query(&message)
        )));
    }
    Ok(redirect("/extensions?notice=agent_reloaded"))
}

pub async fn plugin_page(
    State(state): State<AppState>,
    Path((plugin_id, slug)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let plugin = state.frontend.find(&plugin_id).ok_or_else(|| {
        AppError::new(
            StatusCode::NOT_FOUND,
            format!("前端插件不存在: {plugin_id}"),
        )
    })?;
    let (page_title, html) = state
        .frontend
        .read_page(&plugin, &slug)
        .map_err(|error| AppError::new(StatusCode::NOT_FOUND, format!("{error:#}")))?;
    let page = PluginPage {
        base: base(
            &state,
            &format!("{} · {}", plugin.name, page_title),
            "extensions",
            None,
        ),
        plugin_name: plugin.name,
        page_title,
        html,
    };
    Ok(render(&page)?.into_response())
}

pub async fn plugin_asset(
    State(state): State<AppState>,
    Path((plugin_id, path)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let (file, mime) = state
        .frontend
        .asset_path(&plugin_id, &path)
        .map_err(|error| AppError::new(StatusCode::NOT_FOUND, format!("{error:#}")))?;
    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|error| AppError::new(StatusCode::NOT_FOUND, format!("读取资源失败: {error}")))?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str(&mime)
                    .unwrap_or(HeaderValue::from_static("application/octet-stream")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        bytes,
    )
        .into_response())
}

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
    use serde_json::json;

    #[test]
    fn approval_mode_reads_snapshot_config_approval() {
        let snapshot = json!({"config":{"approval":"full","model":"deepseek-chat"}});
        assert_eq!(approval_mode(&snapshot, &Value::Null), "full");
        assert_eq!(approval_label("full"), "完全访问");
        assert_eq!(approval_tone("full"), "danger");
    }

    #[test]
    fn approval_mode_falls_back_to_saved_config() {
        let snapshot = json!({});
        let config = json!({"saved":{"approval":"ask"}});
        assert_eq!(approval_mode(&snapshot, &config), "ask");
        assert_eq!(approval_mode(&snapshot, &Value::Null), "auto");
    }

    #[test]
    fn compact_row_uses_summary_not_text() {
        let row = row_view(&json!({
            "kind":"compact",
            "id":"c1",
            "summary":"压缩了 6 轮对话",
            "preTokens":90000,
            "postTokens":12000
        }));
        assert_eq!(row.key, "c1");
        assert!(row.text.contains("压缩了 6 轮对话"));
        assert!(row.text.contains("90000"));
        assert!(row.text.contains("12000"));
        assert!(ui::render_row(&row, "t1").contains("压缩了 6 轮对话"));
    }

    #[test]
    fn user_row_marks_images_instead_of_silently_dropping_them() {
        let row = row_view(&json!({
            "kind":"user",
            "id":"u1",
            "text":"看一下这张图",
            "images":["data:image/png;base64,AA"]
        }));
        assert_eq!(row.images, 1);
        assert!(ui::render_row(&row, "t1").contains("含 1 张图片"));
    }

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
