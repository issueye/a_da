//! 对话页：首屏 SSR、live 轮询、OOB 片段，以及会话级动作。
//!
//! 行渲染（`row_view` / 工具行 / 审批档位等纯函数）拆在 [`rows`]；
//! 本文件只留页面装配、轮询与动作端点。

mod rows;
mod stream;

use rows::*;

pub use stream::live_stream;

/// 侧栏线程过滤（空占位会话按工作区裁剪）也供工作区选择页的"最近工作区"计数使用。
pub(super) use rows::thread_rows;

use axum::extract::{Form, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;

use super::*;
use crate::plugins::Slot;
use crate::ui::theme::escape;
use crate::ui::{
    self, AuxItem, Button, ButtonVariant, Chip, Composer, ComposerAux, IconButton, Render,
    SendButton, Size, Tone,
};
struct ComposerArgs<'a> {
    thread_id: &'a str,
    approval: &'a str,
    approval_tone: &'a str,
    workspace_label: &'a str,
    workspace_path: &'a str,
    connected: bool,
    running: bool,
    queue_len: usize,
    approval_count: usize,
    /// 最近一回合的速度 / 缓存命中 / 上下文占用。
    stats: TurnStats,
}

/// 工具行里的一项：无边框的图标 + 文字，只有**真有去处**时才带箭头。
///
/// 空箭头等于骗点击（UED 的硬约束）：模型能跳到供应商页，所以给箭头；
/// 审批档位只能在桌面端改，就不给。
fn toolbar_item(
    icon: &str,
    label: &str,
    title: &str,
    href: Option<&str>,
    extra_class: &str,
) -> String {
    let glyph = ui::icon(icon, 14);
    let caret = match href {
        Some(_) => ui::icon("i-chevron-down", 12),
        None => String::new(),
    };
    let body = format!(
        r#"{glyph}<span class="ada-tool__label">{}</span>{caret}"#,
        escape(label)
    );
    match href {
        Some(href) => format!(
            r#"<a class="ada-tool {extra_class}" href="{}" title="{}">{}</a>"#,
            escape(href),
            escape(title),
            body
        ),
        None => format!(
            r#"<span class="ada-tool {extra_class}" title="{}">{}</span>"#,
            escape(title),
            body
        ),
    }
}

/// 圆形「＋」：面板向上弹——输入框贴着视口底部，向下必然被裁掉。
fn plus_menu(thread_id: &str) -> String {
    format!(
        r#"<details class="ada-plus">
  <summary class="ada-plus__button" title="更多操作" aria-label="更多操作">{plus}</summary>
  <div class="ada-plus__panel">
    <form method="post" action="/threads"><input type="hidden" name="title" value="新会话"><button class="ada-plus__item" type="submit">{new}<span>新会话</span></button></form>
    <a class="ada-plus__item" href="/workspaces">{work}<span>切换工作区</span></a>
    <form method="post" action="/threads/{id}/retry"><button class="ada-plus__item" type="submit">{retry}<span>重试上一轮</span></button></form>
  </div>
</details>"#,
        plus = ui::icon("i-plus", 14),
        new = ui::icon("i-chat", 14),
        work = ui::icon("i-folder", 14),
        retry = ui::icon("i-refresh", 14),
        id = escape(thread_id),
    )
}

fn composer_html(slots: &SlotContent, args: &ComposerArgs<'_>, model_picker: &str) -> String {
    // 左组：更多操作、工作区、审批档位。
    let mut left = String::new();
    left.push_str(&plus_menu(args.thread_id));
    if !args.workspace_label.is_empty() {
        // 工作区名是"可以短一点"的信息（侧栏有全名），窄屏先让它让位。
        left.push_str(&toolbar_item(
            "i-git-branch",
            args.workspace_label,
            args.workspace_path,
            None,
            "ada-tool--workspace",
        ));
    }
    // 用独立容器包住：live 轮询只换这一个（不重建整条工具行）。
    left.push_str(&format!(
        r#"<span id="composer-approval">{}</span>"#,
        approval_chip_html(args.approval, args.approval_tone)
    ));
    left.push_str(&slots.composer_left);

    // 右组：模型选择（DeepSeek 式弹层）+ 发送。
    let mut right = String::new();
    right.push_str(model_picker);
    right.push_str(&slots.composer_right);
    right.push_str(&SendButton::new("发送").title("发送（Enter）").render());

    Composer {
        action: format!("/threads/{}/send", args.thread_id),
        placeholder: "向 a_da 提问，或指派一个任务…".to_string(),
        left,
        right,
        aux: Some(composer_aux(slots, args)),
    }
    .render()
}

/// 输入框下方这一行：状态（异常才出）+ 遥测 + 动作 + 一处提示。
///
/// 只留与「这一次发送」有关的东西：工作区名在工具栏已有一份、消息条数在侧栏一眼可见，
/// 这里不再重复——同一件事说两遍，用户会怀疑哪个才是真的。
fn composer_aux(slots: &SlotContent, args: &ComposerArgs<'_>) -> ComposerAux {
    let mut status = Vec::new();
    // 连上了不必说"已连接"：那是默认状态，天天显示只是噪音；断线才值得喊一声。
    if !args.connected {
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

    // 遥测：模型自己报回的 usage 与耗时算出来的，不估算。
    let mut context = Vec::new();
    if let Some(speed) = args.stats.tokens_per_sec {
        context.push(AuxItem::new(format!("{speed} tok/s"), Tone::Neutral).icon("i-wave"));
    }
    if let Some(hit) = args.stats.cache_hit {
        context.push(AuxItem::new(format!("缓存命中 {hit}%"), Tone::Neutral).icon("i-layers"));
    }
    if let Some(percent) = args.stats.context {
        // 接近窗口上限才变色：常态下它只是一个参考数。
        let tone = if percent >= 85 {
            Tone::Warn
        } else {
            Tone::Neutral
        };
        context.push(AuxItem::new(format!("上下文 {percent}%"), tone).icon("i-chart"));
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

/// 顶栏状态：只放**全局且异常**的信号（运行中 / 未连接）。
///
/// 审批档位不在这里——它在输入框工具栏里，离"发送"更近，才是它该出现的位置。
fn topbar_status_html(running: bool, connected: bool) -> String {
    let mut out = String::new();
    if running {
        out.push_str(
            r#"<span class="ada-chip ada-chip--warn"><span class="ada-dot ada-dot--warn"></span>运行中</span>"#,
        );
    }
    if !connected {
        out.push_str(
            &Chip::new("未连接", Tone::Danger)
                .title("无法连接 agent 网关")
                .render(),
        );
    }
    out
}

fn approval_chip_html(label: &str, tone: &str) -> String {
    // 只读项：不给箭头（切换在桌面端设置里），但语义色保留，便于一眼看出档位危险与否。
    let (icon, modifier) = match tone {
        "danger" => ("i-shield-check", "ada-tool--danger"),
        "warn" => ("i-shield-check", "ada-tool--warn"),
        _ => ("i-shield", ""),
    };
    format!(
        r#"<span class="ada-tool {modifier}" title="审批档位：在桌面端设置中调整">{glyph}<span class="ada-tool__label">{label}</span></span>"#,
        glyph = ui::icon(icon, 14),
        label = escape(label),
    )
}

/// 药丸内部的标签：模型名 + 推理等级（外层是模型选择器的 `<summary>`）。
///
/// live 轮询只替换 `#composer-model` 的内容，菜单本身（`<details>`）不会被关掉。
fn model_chip_html(model: &str, effort: &str) -> String {
    ui::model_chip_inner(model, effort)
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
    trays: String,
    /// 会话区右上角的插件插槽（任务规划卡片就在这一格）。
    corner: String,
    topbar_status: String,
    topbar_actions: String,
    aux: String,
    /// 输入框上方的插件悬浮区（`composer.above`）。
    composer_above: String,
    approval_chip: String,
    model_chip: String,
}

/// 渲染对话流**之外**的动态区域（托盘 / 任务规划 / 顶栏 / 输入框辅助区）。
///
/// 刻意不含 transcript：SSE 流式更新时正文是按 item 逐条替换的，
/// 每帧重渲染整段对话流既浪费又会破坏展开状态；整段同步由调用方单独触发。
async fn live_parts(
    state: &AppState,
    snapshot: &Value,
    thread_id: &str,
    thread: &Value,
) -> LiveParts {
    let raw_items = thread
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let (trays, approval_count, queue_len) = trays_html(snapshot, raw_items, thread_id);
    let running = running_ids(snapshot).iter().any(|id| id == thread_id);
    let mode = approval_mode(snapshot, &Value::Null);
    let approval = approval_label(&mode);
    let tone = approval_tone(&mode);
    let model = model_of(snapshot, &Value::Null);
    let effort = effort_of(snapshot, &Value::Null);
    let workspace = str_at(thread, "workspace");
    let (workspace_label, workspace_path) = if workspace.is_empty() {
        (
            workspace_label(&state.agent.workspace),
            state.agent.workspace.as_str(),
        )
    } else {
        (workspace_label(workspace), workspace)
    };
    // 这里只换辅助区、输入框上方与会话区右上角，不必把十一个插槽都渲染一遍。
    // 🔴 必须带真实上下文：早先用 `render_slot()`（默认 `RenderContext`），
    // 于是首屏渲染正常、2 秒后第一次轮询就把 `props.app.*` / `props.data.*` 刷成空。
    // 三个插槽共用同一份（带 `data.tokens` / `data.todo`）的上下文，首屏与轮询看到的数据才一致。
    let chat_context = super::render_context_with_tokens(state, "chat", snapshot, Some(thread_id));
    let footer = crate::plugins::render_slot_off_thread(
        std::sync::Arc::clone(&state.frontend),
        Slot::ComposerFooter,
        chat_context.clone(),
    )
    .await;
    let above = crate::plugins::render_slot_off_thread(
        std::sync::Arc::clone(&state.frontend),
        Slot::ComposerAbove,
        chat_context.clone(),
    )
    .await;
    // 任务规划这类常驻卡片：插件挂在会话区右上角，数据与轮询同源。
    let corner = crate::plugins::render_slot_off_thread(
        std::sync::Arc::clone(&state.frontend),
        Slot::ChatCorner,
        chat_context,
    )
    .await;
    let slots = SlotContent {
        composer_footer: footer.html,
        ..SlotContent::default()
    };
    let args = ComposerArgs {
        thread_id,
        approval: &approval,
        approval_tone: &tone,
        workspace_label: &workspace_label,
        workspace_path,
        connected: true,
        running,
        queue_len,
        approval_count,
        stats: turn_stats(snapshot, thread),
    };
    LiveParts {
        trays,
        corner: corner.html,
        topbar_status: topbar_status_html(running, true),
        topbar_actions: topbar_actions_html(thread_id, running),
        aux: composer_aux_html(&slots, &args),
        composer_above: above.html,
        approval_chip: approval_chip_html(&approval, &tone),
        model_chip: model_chip_html(&model, &effort),
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
                base: offline_base(state, Some(flash)).await,
                threads: Vec::new(),
                workspaces: Vec::new(),
                current_workspace: String::new(),
                active: None,
                active_thread_id: String::new(),
                items: Vec::new(),
                transcript_empty: ui::render_empty_thread(),
                trays_html: String::new(),
                composer_html: String::new(),
                topbar_status: topbar_status_html(false, false),
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
    // 当前工作区：优先"当前会话所属工作区"（与桌面端 `currentThreadWorkspace` 同口径），
    // 否则用 agent 的当前项目；侧栏树据此决定默认展开与"当前"标记。
    let agent_workspace = snapshot
        .get("workspace")
        .and_then(|workspace| workspace.get("project"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|workspace| !workspace.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.agent.workspace.clone());
    let active_workspace = active
        .as_ref()
        .map(|thread| thread.workspace.trim().to_string())
        .filter(|workspace| !workspace.is_empty())
        .unwrap_or_else(|| agent_workspace.clone());
    let workspaces = workspace_groups(&threads, &active_workspace);
    let raw_items: &[Value] = active_value
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let items: Vec<ItemRow> = raw_items
        .iter()
        .map(|item| item_row(item, &active_thread_id))
        .collect();
    let (trays, approval_count, queue_len) = trays_html(&snapshot, raw_items, &active_thread_id);

    let config = state.agent.config().await.unwrap_or(Value::Null);
    let model = model_of(&snapshot, &config);
    let effort = effort_of(&snapshot, &config);
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

    let mut base = base_with_snapshot(
        state,
        &title,
        "chat",
        flash_from_query(query),
        Some(&snapshot),
        // 插槽属于"正在渲染的这条会话"：深链接聚焦失败时也不能拿错计划/用量。
        Some(active_thread_id.as_str()),
    )
    .await;
    // 直接打开某条会话时，`base` 里的 active_workspace 是 agent 的当前项目；
    // 侧栏与"新会话"应以**当前会话**的工作区为准，这里覆盖一次。
    if !active_workspace.is_empty() {
        base.active_workspace = active_workspace.clone();
        base.active_workspace_name = workspace_name(&active_workspace);
    }
    // 侧栏整块重渲一次：base 里只有公共字段，工作区树与"当前会话"是本页才知道的。
    base.sidebar = ui::sidebar::Sidebar::from_base(&base)
        .with_tree(
            workspaces.clone(),
            active_thread_id.clone(),
            active_workspace.clone(),
        )
        .connected(Some(true))
        .render();

    // 模型选择器：按供应商分组的模型清单 + 推理等级（数据来自 `/api/providers`）。
    // 只有真要渲染输入框时才多打这一枪；切换动作本身走 `/model/select|effort` 表单。
    let model_picker = if active_thread_id.is_empty() {
        String::new()
    } else {
        let providers = state.agent.providers().await.unwrap_or(Value::Null);
        let active_provider_id = {
            let from_providers = str_at(&providers, "activeProviderId");
            if from_providers.is_empty() {
                config
                    .get("saved")
                    .and_then(|saved| saved.get("activeProviderId"))
                    .and_then(Value::as_str)
                    .or_else(|| config.get("activeProviderId").and_then(Value::as_str))
                    .unwrap_or("")
            } else {
                from_providers
            }
        };
        ui::ModelPicker::from_providers(
            &providers,
            active_provider_id,
            &model,
            &effort,
            &active_thread_id,
        )
        .render()
    };

    // 输入框的插槽直接复用 `base.slots`（同一份带 `data.tokens` / `data.todo` 的上下文）。
    // 早先这里另用 `render_context()` 渲染一遍：composer.above 的用量/计划会突然空掉，
    // 而且十一个插槽的 TSX 要跑两次——同一份事实不该有两个入口。
    // 没有活动会话时不给输入框：提交到 `/threads//send` 只会得到一个错误。
    let composer_html = if active_thread_id.is_empty() {
        String::new()
    } else {
        composer_html(
            &base.slots,
            &ComposerArgs {
                thread_id: &active_thread_id,
                approval: &approval,
                approval_tone: &approval_tone,
                workspace_label: &workspace_label,
                workspace_path: &workspace_path,
                connected: true,
                running: is_running,
                queue_len,
                approval_count,
                stats: active_value
                    .map(|value| turn_stats(&snapshot, value))
                    .unwrap_or_default(),
            },
            &model_picker,
        )
    };

    Ok(ChatPage {
        base,
        threads,
        workspaces,
        current_workspace: active_workspace.clone(),
        active,
        active_thread_id: active_thread_id.clone(),
        items,
        transcript_empty: ui::render_empty_thread(),
        trays_html: trays,
        composer_html,
        topbar_status: topbar_status_html(is_running, true),
        topbar_actions: topbar_actions_html(&active_thread_id, is_running),
        connected: true,
    })
}

/// 离线壳：侧栏照常渲染（没有会话树），但连接状态要如实显示"未连接"。
async fn offline_base(state: &AppState, flash: Option<Flash>) -> PageBase {
    let mut base = base(state, "未连接", "chat", flash).await;
    base.sidebar = ui::sidebar::Sidebar::from_base(&base)
        .connected(Some(false))
        .render();
    base
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
async fn live_offline(state: &AppState, thread_id: &str) -> Response {
    let label = workspace_label(&state.agent.workspace);
    // 断开时没有快照，用量与任务计划都无从谈起：插件会显示"无数据"/不渲染卡片，
    // 而不是停在旧数字上骗人。
    let context = super::render_context(state, "chat");
    let footer = crate::plugins::render_slot_off_thread(
        std::sync::Arc::clone(&state.frontend),
        Slot::ComposerFooter,
        context.clone(),
    )
    .await;
    let above = crate::plugins::render_slot_off_thread(
        std::sync::Arc::clone(&state.frontend),
        Slot::ComposerAbove,
        context.clone(),
    )
    .await;
    let corner = crate::plugins::render_slot_off_thread(
        std::sync::Arc::clone(&state.frontend),
        Slot::ChatCorner,
        context,
    )
    .await;
    let slots = SlotContent {
        composer_footer: footer.html,
        ..SlotContent::default()
    };
    let args = ComposerArgs {
        thread_id,
        approval: "未连接",
        approval_tone: "neutral",
        workspace_label: &label,
        workspace_path: &state.agent.workspace,
        connected: false,
        running: false,
        queue_len: 0,
        approval_count: 0,
        stats: TurnStats::default(),
    };
    let body = [
        oob("topbar-status", &topbar_status_html(false, false)),
        oob("topbar-actions", &topbar_actions_html(thread_id, false)),
        oob("chat-corner", &corner.html),
        oob("composer-above", &above.html),
        oob("composer-aux", &composer_aux_html(&slots, &args)),
        oob(
            "composer-approval",
            &approval_chip_html("未连接", "neutral"),
        ),
        oob("composer-model", &model_chip_html("未连接", "")),
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
            return live_offline(&state, &thread_id).await;
        }
    };
    let Some(thread) = array_at(&snapshot, "threads")
        .iter()
        .find(|value| str_at(value, "id") == thread_id)
    else {
        return (StatusCode::NOT_FOUND, "会话不存在").into_response();
    };
    let parts = live_parts(&state, &snapshot, &thread_id, thread).await;
    let items: Vec<ItemRow> = thread
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item_row(item, &thread_id))
                .collect()
        })
        .unwrap_or_default();
    let body = [
        oob("transcript", &transcript_html(items)),
        oob("ada-trays", &parts.trays),
        oob("chat-corner", &parts.corner),
        oob("topbar-status", &parts.topbar_status),
        oob("topbar-actions", &parts.topbar_actions),
        // 输入框上方的插件悬浮区：用量会随对话变化，跟其他区域一起每 2 秒刷新。
        oob("composer-above", &parts.composer_above),
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
    // 先聚焦：快照只给"激活 / 打开的 Tab / 运行中"会话带 items。
    // 侧栏树可以点到别的工作区的历史会话，不聚焦的话正文会是空的
    // （桌面端 `handleSelectThread` 也是先 `setActiveThread`）。
    if let Err(error) = state.agent.focus_thread(&thread_id).await {
        tracing::debug!("聚焦会话失败（继续按快照渲染）: {error:#}");
    }
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
    // 侧栏工作区树的"+"会带上目标工作区；顶部的"新会话"不带，
    // 由 agent 落到当前激活工作区（与桌面端 currentThreadWorkspace 的行为一致）。
    let workspace = form
        .workspace
        .as_deref()
        .map(str::trim)
        .filter(|workspace| !workspace.is_empty());
    let result = match workspace {
        Some(workspace) => state.agent.create_thread_in(title, workspace).await,
        None => state.agent.create_thread(title).await,
    };
    let result = match result {
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

#[derive(Debug, Deserialize)]
pub struct ModelSelectForm {
    provider: String,
    model: String,
    thread: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EffortSelectForm {
    effort: String,
    thread: Option<String>,
}

/// 表单里的 `thread` 只用于"切完回哪一页"：只接受合法会话 id，防止把跳转拼成任意路径。
fn safe_thread(thread: Option<&str>) -> Option<&str> {
    thread.map(str::trim).filter(|id| {
        !id.is_empty()
            && id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    })
}

fn back_to_thread(thread: Option<&str>, notice: &str) -> Response {
    match safe_thread(thread) {
        Some(id) => redirect(&format!("/threads/{id}?notice={notice}")),
        None => redirect(&format!("/?notice={notice}")),
    }
}

fn failed_before_redirect(thread: Option<&str>, prefix: &str, error: anyhow::Error) -> Response {
    match safe_thread(thread) {
        Some(id) => action_failed(id, prefix, error),
        None => {
            tracing::warn!("{prefix}: {error:#}");
            redirect(&format!(
                "/?error={}",
                encode_query(&format!("{prefix}：{}", public_reason(&error)))
            ))
        }
    }
}

/// 切换模型：`provider.setActive`（供应商 + 模型一起切，与供应商页的"使用"同一动作）。
pub async fn select_model(
    State(state): State<AppState>,
    Form(form): Form<ModelSelectForm>,
) -> Result<Response, AppError> {
    let provider = form.provider.trim();
    let model = form.model.trim();
    if provider.is_empty() || model.is_empty() {
        return Err(AppError::bad("切换模型缺少 provider / model"));
    }
    match state.agent.set_active_provider(provider, Some(model)).await {
        Ok(_) => Ok(back_to_thread(form.thread.as_deref(), "model_selected")),
        Err(error) => Ok(failed_before_redirect(
            form.thread.as_deref(),
            "切换模型失败",
            error,
        )),
    }
}

/// 推理等级：`config.setEffort`（与 desktop-ui 设置项同源，网关落盘并广播快照）。
pub async fn select_effort(
    State(state): State<AppState>,
    Form(form): Form<EffortSelectForm>,
) -> Result<Response, AppError> {
    let effort = form.effort.trim().to_ascii_lowercase();
    if !matches!(effort.as_str(), "low" | "medium" | "high" | "max") {
        return Err(AppError::bad(format!("未知的推理等级: {effort}")));
    }
    match state.agent.set_effort(&effort).await {
        Ok(_) => Ok(back_to_thread(form.thread.as_deref(), "effort_selected")),
        Err(error) => Ok(failed_before_redirect(
            form.thread.as_deref(),
            "更新推理等级失败",
            error,
        )),
    }
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
