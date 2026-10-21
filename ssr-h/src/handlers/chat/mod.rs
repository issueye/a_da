//! 对话页：首屏 SSR、live 轮询、OOB 片段，以及会话级动作。
//!
//! 行渲染（`row_view` / 工具行 / 审批档位等纯函数）拆在 [`rows`]；
//! 本文件只留页面装配、轮询与动作端点。

mod rows;

use rows::*;

use axum::extract::{Form, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde_json::Value;

use super::*;
use crate::plugins::Slot;
use crate::ui::{
    self, AuxItem, Button, ButtonKind, ButtonVariant, Chip, Composer, ComposerAux, IconButton,
    Render, SendButton, Size, Tone,
};
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
fn action_failed(thread_id: &str, prefix: &str, error: anyhow::Error) -> Response {
    tracing::warn!("{prefix}: {error:#}");
    let message = format!("{prefix}：{}", public_reason(&error));
    redirect(&format!(
        "/threads/{thread_id}?error={}",
        encode_query(&message)
    ))
}

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
    let context = super::render_context(state, "chat");
    let slots = slot_content(&state.frontend, &context);

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
