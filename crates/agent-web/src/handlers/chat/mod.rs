//! 对话页：首屏 SSR、live 轮询、OOB 片段，以及会话级动作。
//!
//! 行渲染（`row_view` / 工具行 / 审批档位等纯函数）拆在 [`rows`]；
//! 本文件只留页面装配、轮询与动作端点。

mod rows;
mod stream;

use rows::*;
pub(super) use rows::approval_rows;

pub use stream::live_stream;

/// 侧栏线程过滤（空占位会话按工作区裁剪）也供工作区选择页的"最近工作区"计数使用。
pub(super) use rows::thread_rows;

use axum::extract::{Form, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
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
    /// 审批档位原值（auto/ask/readonly/full）：弹层用它标出当前项。
    approval_mode: &'a str,
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
    // 左组：更多操作、图片、工作区、审批档位。
    let mut left = String::new();
    left.push_str(&plus_menu(args.thread_id));
    // 图片输入：选图 / 粘贴 / 拖入都由 chat.js 接管（`data-image-*` 契约），
    // 隐藏的 file input 让它保持无 JS 时可读（按钮不渲染成假按钮）。
    left.push_str(&format!(
        r#"<input class="ada-composer__file" type="file" accept="image/png,image/jpeg,image/webp,image/gif" multiple hidden data-image-input><button class="ada-icon-btn ada-composer__attach" type="button" data-image-pick aria-label="添加图片" title="添加图片（也可粘贴或拖入）">{}</button>"#,
        ui::icon("i-image", 14)
    ));
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
        approval_picker_html(args.approval_mode, args.approval, args.approval_tone, args.thread_id)
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
        context.push(
            AuxItem::new(format!("{speed}"), Tone::Neutral)
                .icon("i-wave")
                .title(format!("{speed} tok/s")),
        );
    }
    if let Some(hit) = args.stats.cache_hit {
        context.push(
            AuxItem::new(format!("{hit}%"), Tone::Neutral)
                .icon("i-layers")
                .title(format!("缓存命中 {hit}%")),
        );
    }
    if let Some(percent) = args.stats.context {
        // 接近窗口上限才变色：常态下它只是一个参考数。
        let tone = if percent >= 85 {
            Tone::Warn
        } else {
            Tone::Neutral
        };
        context.push(
            AuxItem::new(format!("{percent}%"), tone)
                .icon("i-chart")
                .title(format!("上下文占用 {percent}%")),
        );
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
                    .attrs("data-partial data-confirm=\"确定清空当前会话的排队消息吗？\"")
                    .class("ada-aux__action"),
            );
        }
    }

    ComposerAux {
        status,
        context,
        actions,
        hint: None,
        plugins: slots.composer_footer.clone(),
    }
}

fn composer_aux_html(slots: &SlotContent, args: &ComposerArgs<'_>) -> String {
    composer_aux(slots, args).render()
}

/// 输入框左上方（与 `composer.above` 插件同一带）的运行状态指示。
///
/// 只有一个**带旋转弧的 SVG**（文字通过 `aria-label` 提供给读屏）。
/// 顶栏与辅助区不再重复"运行中"文字——同一个状态摆三处只会抢注意力。
/// 空闲时返回空串——容器 `:empty` 不占位。
fn run_status_html(running: bool) -> String {
    if !running {
        return String::new();
    }
    r#"<span class="ada-run-status" role="status" aria-live="polite" aria-label="运行中" title="运行中"><svg class="ada-run-status__spin" width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-opacity=".25" stroke-width="2"/><path d="M21 12a9 9 0 0 0-9-9" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg></span>"#
        .to_string()
}

/// 顶栏状态：只放**全局且异常**的信号（未连接）。
///
/// 运行中不再用文字药丸：输入框上方的旋转弧与侧栏运行点已经表达；
/// 审批档位也不在这里——它在输入框工具栏里，离"发送"更近。
fn topbar_status_html(_running: bool, connected: bool) -> String {
    let mut out = String::new();
    if !connected {
        out.push_str(
            &Chip::new("未连接", Tone::Danger)
                .title("无法连接 agent 网关")
                .render(),
        );
    }
    out
}

/// 审批档位弹层：输入框工具行的药丸点开是四档选择（无 JS 可用）。
///
/// 当前档位打勾且不可再点；「完全访问」是安全降级动作，走共享确认弹窗
/// （data-confirm 在 form 上，app.js 拦截）。菜单随 `parts` 的
/// `composer-approval` 整块换新，`id` 供 replaceKeepingOpen 还原开合
/// （换帧不关菜单，与模型选择器同口径）。`mode` 为空（未连接）保持只读药丸。
fn approval_picker_html(mode: &str, label: &str, tone: &str, thread_id: &str) -> String {
    let (icon, modifier) = match tone {
        "danger" => ("i-shield-check", "ada-tool--danger"),
        "warn" => ("i-shield-check", "ada-tool--warn"),
        _ => ("i-shield", ""),
    };
    if mode.is_empty() {
        return format!(
            r#"<span class="ada-tool {modifier}" title="审批档位">{glyph}<span class="ada-tool__label">{label}</span></span>"#,
            glyph = ui::icon(icon, 14),
            label = escape(label),
        );
    }
    let options = [
        ("auto", "自动批准", "i-sparkles", ""),
        ("ask", "每次询问", "i-chat", ""),
        ("readonly", "严格只读", "i-file", ""),
        (
            "full",
            "完全访问",
            "i-shield-check",
            r#" data-confirm="切换到「完全访问」后，工具执行将不再逐一询问，确定吗？""#,
        ),
    ];
    let mut rows = String::new();
    for (value, option_label, icon, confirm) in options {
        let is_current = value == mode;
        let check = if is_current {
            format!(
                r#"<span class="ada-approval-picker__check">{}</span>"#,
                ui::icon("i-check", 12)
            )
        } else {
            String::new()
        };
        rows.push_str(&format!(
            r#"<form method="post" action="/approval/mode" data-partial{confirm}><input type="hidden" name="thread_id" value="{thread}"><input type="hidden" name="mode" value="{value}"><button class="ada-approval-picker__option" type="submit"{disabled}>{glyph}<span>{option_label}</span>{check}</button></form>"#,
            thread = escape(thread_id),
            value = value,
            confirm = confirm,
            disabled = if is_current { " disabled" } else { "" },
            glyph = ui::icon(icon, 14),
            check = check,
        ));
    }
    format!(
        r#"<details class="ada-popover ada-approval-picker" id="approval-picker"><summary class="ada-tool {modifier}" title="审批档位（点击切换）">{glyph}<span class="ada-tool__label">{label}</span>{caret}</summary><div class="ada-popover__panel ada-approval-picker__panel" role="menu">{rows}</div></details>"#,
        glyph = ui::icon(icon, 14),
        caret = ui::icon("i-chevron-down", 12),
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
    /// 输入框左上方的运行状态（旋转弧；空闲时为空）。
    run_status: String,
    topbar_status: String,
    topbar_actions: String,
    aux: String,
    /// 输入框上方的插件悬浮区（`composer.above`）。
    composer_above: String,
    approval_chip: String,
    model_chip: String,
    /// 侧栏会话树（`#thread-list` 的 innerHTML）。
    ///
    /// parts 指纹早已监视 threads/running（stream.rs `parts_fingerprint`）——
    /// 数据通了一半，这里补上渲染与 OOB 通道：其它客户端的新会话/改名/运行点
    /// 不再要手动刷新（STATUS TODO「侧栏会话列表增量刷新」）。
    thread_list: String,
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
    let mut chat_context = super::render_context_with_tokens(state, "chat", snapshot, Some(thread_id));
    super::attach_backend_actions(state, &mut chat_context).await;
    super::attach_tasks(state, &mut chat_context).await;
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
        chat_context.clone(),
    )
    .await;
    // 审批段：`thread.approval` 插槽输出非空时替换核心审批托盘；批准/拒绝仍走核心。
    let thread_approval = crate::plugins::render_slot_off_thread(
        std::sync::Arc::clone(&state.frontend),
        Slot::ThreadApproval,
        chat_context.clone(),
    )
    .await;
    // 提问段：`thread.tray` 插槽输出非空时替换核心提问托盘（D5 降级规则）。
    let thread_tray = crate::plugins::render_slot_off_thread(
        std::sync::Arc::clone(&state.frontend),
        Slot::ThreadTray,
        chat_context,
    )
    .await;
    let (trays, approval_count, queue_len) = trays_html(
        snapshot,
        raw_items,
        thread_id,
        Some(thread_approval.html.trim()),
        Some(thread_tray.html.trim()),
    );
    // 侧栏树与首屏同源（thread_rows 的空占位过滤 + workspace_groups 分组排序），
    // 分组锚点与首屏一致：当前会话的工作区，缺了回落 agent 的当前项目。
    let tree_workspace = if workspace.is_empty() {
        state.agent.workspace.as_str()
    } else {
        workspace
    };
    let thread_list = ui::sidebar::Sidebar::thread_list_inner(
        &workspace_groups(&thread_rows(snapshot, thread_id), tree_workspace),
        thread_id,
    );
    let slots = SlotContent {
        composer_footer: footer.html,
        ..SlotContent::default()
    };
    let args = ComposerArgs {
        thread_id,
        approval: &approval,
        approval_mode: &mode,
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
        run_status: run_status_html(running),
        topbar_status: topbar_status_html(running, true),
        topbar_actions: topbar_actions_html(thread_id, running),
        aux: composer_aux_html(&slots, &args),
        composer_above: above.html,
        approval_chip: approval_picker_html(&mode, &approval, &tone, thread_id),
        model_chip: model_chip_html(&model, &effort),
        thread_list,
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
                workspace_picker: String::new(),
                run_status: String::new(),
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

    // 托盘（审批/提问/排队）：`thread.approval` / `thread.tray` 插槽已在 base.slots
    // 里按同一份会话上下文渲染，输出非空时替换对应核心托盘。
    let (trays, approval_count, queue_len) = trays_html(
        &snapshot,
        raw_items,
        &active_thread_id,
        Some(base.slots.thread_approval.trim()),
        Some(base.slots.thread_tray.trim()),
    );

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
                approval_mode: &mode,
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

    // 空会话：把「模式 + 工作区」控件作为 transcript 的空状态交给模板。
    // SSE 首帧 / 兜底轮询会用同一个 helper 重建（见 stream::render_transcript
    // 与 live），否则连上流的一瞬间控件会被通用空状态覆盖。
    let transcript_empty = if items.is_empty() && !active_thread_id.is_empty() {
        empty_conversation_html(&snapshot, &active_thread_id).unwrap_or_else(ui::render_empty_thread)
    } else {
        ui::render_empty_thread()
    };
    // 「选择工作区」弹窗：目录片段由 htmx 在打开时懒加载（`ada-modal-load`），
    // 兜底链接仍指向 /workspaces?bind=…（无 JS 时整页浏览）。
    let workspace_picker = if items.is_empty() && !active_thread_id.is_empty() {
        let body = format!(
            r##"<button class="wp-picker__close" type="button" data-modal-close aria-label="关闭" title="关闭"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"/></svg></button>
<div id="workspace-picker-body" data-modal-src="workspace-picker" hx-get="/workspaces/picker?bind={bind}" hx-trigger="ada-modal-load" hx-target="#workspace-picker-body" hx-swap="innerHTML"><p class="wp-picker__loading">正在读取目录…</p></div>"##,
            bind = escape(&active_thread_id)
        );
        ui::Modal::new("workspace-picker", "选择工作区目录")
            .wide()
            .body(body)
            .render()
    } else {
        String::new()
    };

    Ok(ChatPage {
        base,
        threads,
        workspaces,
        current_workspace: active_workspace.clone(),
        active,
        active_thread_id: active_thread_id.clone(),
        items,
        transcript_empty,
        workspace_picker,
        run_status: run_status_html(is_running),
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
        approval_mode: "",
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
            &approval_picker_html("", "未连接", "neutral", ""),
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
    // 空会话：轮询也要给带「模式 + 工作区」控件的版本，否则 2 秒后的兜底帧
    // 会把页面上的控件换回通用空状态（SSE 首帧同理，见 stream::render_transcript）。
    let empty_html = if items.is_empty() {
        empty_conversation_html(&snapshot, &thread_id).unwrap_or_else(ui::render_empty_thread)
    } else {
        ui::render_empty_thread()
    };
    let body = [
        oob("transcript", &transcript_html(items, empty_html)),
        oob("ada-trays", &parts.trays),
        oob("chat-corner", &parts.corner),
        oob("run-status", &parts.run_status),
        oob("topbar-status", &parts.topbar_status),
        oob("topbar-actions", &parts.topbar_actions),
        // 侧栏会话树：与 parts 同源（其它客户端的增删改名随轮询进来）。
        oob("thread-list", &parts.thread_list),
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
    // 模式只在**新建时**可选（开始后不可切，与 原桌面端 的 ModeSlider 一致）；
    // 未知值按缺省处理，不因为一个畸形参数让建会话失败。
    let mode = form
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|mode| matches!(*mode, "code" | "plan" | "create" | "pm"));
    let result = match (workspace, mode) {
        (Some(workspace), Some(mode)) => {
            state.agent.create_thread_in(title, workspace, Some(mode)).await
        }
        (Some(workspace), None) => state.agent.create_thread_in(title, workspace, None).await,
        (None, Some(mode)) => {
            state.agent
                .create_thread_in(title, &state.agent.workspace, Some(mode))
                .await
        }
        (None, None) => state.agent.create_thread(title).await,
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

/// `notice_query` 是完整查询片段（如 `notice=model_selected`）：key 以 `notice=`
/// 字面量出现在调用点，才能被 `handlers::tests::notice_literals_are_translated`
/// 的源码扫描守门到。
fn back_to_thread(thread: Option<&str>, notice_query: &str) -> Response {
    match safe_thread(thread) {
        Some(id) => redirect(&format!("/threads/{id}?{notice_query}")),
        None => redirect(&format!("/?{notice_query}")),
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
    headers: HeaderMap,
    Form(form): Form<ModelSelectForm>,
) -> Result<Response, AppError> {
    let partial = wants_partial(&headers);
    let provider = form.provider.trim();
    let model = form.model.trim();
    if provider.is_empty() || model.is_empty() {
        let message = "切换模型缺少 provider / model";
        return if partial {
            Ok(partial_err(message))
        } else {
            Err(AppError::bad(message))
        };
    }
    match state.agent.set_active_provider(provider, Some(model)).await {
        Ok(_) if partial => Ok(partial_ok("model_selected")),
        Ok(_) => Ok(back_to_thread(
            form.thread.as_deref(),
            "notice=model_selected",
        )),
        Err(error) if partial => Ok(partial_err(format!(
            "切换模型失败：{}",
            public_reason(&error)
        ))),
        Err(error) => Ok(failed_before_redirect(
            form.thread.as_deref(),
            "切换模型失败",
            error,
        )),
    }
}

/// 推理等级：`config.setEffort`（与 原桌面端 设置项同源，网关落盘并广播快照）。
pub async fn select_effort(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<EffortSelectForm>,
) -> Result<Response, AppError> {
    let partial = wants_partial(&headers);
    let effort = form.effort.trim().to_ascii_lowercase();
    if !matches!(effort.as_str(), "low" | "medium" | "high" | "max") {
        let message = format!("未知的推理等级: {effort}");
        return if partial {
            Ok(partial_err(message))
        } else {
            Err(AppError::bad(message))
        };
    }
    match state.agent.set_effort(&effort).await {
        Ok(_) if partial => Ok(partial_ok("effort_selected")),
        Ok(_) => Ok(back_to_thread(
            form.thread.as_deref(),
            "notice=effort_selected",
        )),
        Err(error) if partial => Ok(partial_err(format!(
            "更新推理等级失败：{}",
            public_reason(&error)
        ))),
        Err(error) => Ok(failed_before_redirect(
            form.thread.as_deref(),
            "更新推理等级失败",
            error,
        )),
    }
}

/// 单张图片 data URI 的上限（base64 后约 8.7 MB 原图）：与前端上限一致。
const MAX_IMAGE_DATA_URI: usize = 12 * 1024 * 1024;
/// 一条消息最多附带的图片数。
const MAX_IMAGES: usize = 4;
/// 允许的图片类型（与前端 `accept` 一致；不认的格式一律拒收）。
const ALLOWED_IMAGE_PREFIXES: [&str; 5] = [
    "data:image/png;base64,",
    "data:image/jpeg;base64,",
    "data:image/jpg;base64,",
    "data:image/webp;base64,",
    "data:image/gif;base64,",
];

/// 是否是可渲染的图片 data URI（渲染与提交两侧共用）。
fn image_uri_allowed(uri: &str) -> bool {
    ALLOWED_IMAGE_PREFIXES
        .iter()
        .any(|prefix| uri.starts_with(prefix))
}

/// 从表单键值对里收集 `images`（同名多值）并做服务端校验。
///
/// 🔴 不信任前端：格式白名单 + 张数上限 + 单张大小上限都在这里再判一次。
/// 前端的 `accept`/大小检查只是提前反馈，绕过它也不能把任意内容塞进会话。
fn collect_images(pairs: &[(String, String)]) -> Result<Vec<String>, String> {
    let images: Vec<String> = pairs
        .iter()
        .filter(|(key, _)| key == "images")
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    if images.len() > MAX_IMAGES {
        return Err(format!("最多附带 {MAX_IMAGES} 张图片"));
    }
    for image in &images {
        if !image_uri_allowed(image) {
            return Err("图片格式不支持（仅 PNG / JPEG / WebP / GIF）".to_string());
        }
        if image.len() > MAX_IMAGE_DATA_URI {
            return Err("单张图片超过大小上限（8 MB）".to_string());
        }
    }
    Ok(images)
}

pub async fn send_message(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Form(form): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let text = form
        .iter()
        .find(|(key, _)| key == "text")
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default();
    let images = match collect_images(&form) {
        Ok(images) => images,
        Err(message) => {
            return Ok(redirect(&format!(
                "/threads/{thread_id}?error={}",
                encode_query(&format!("图片未发送：{message}"))
            )))
        }
    };
    // 纯图片消息允许发送（文本可以为空）——与 原桌面端 一致。
    if text.is_empty() && images.is_empty() {
        return Ok(redirect(&format!("/threads/{thread_id}")));
    }
    match state.agent.send_message(&thread_id, &text, &images).await {
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
    headers: HeaderMap,
    Form(form): Form<ApprovalForm>,
) -> Result<Response, AppError> {
    let partial = wants_partial(&headers);
    if call_id.trim().is_empty() {
        let message = "审批缺少 callId";
        return if partial {
            Ok(partial_err(message))
        } else {
            Err(AppError::bad(message))
        };
    }
    let approved = matches!(
        form.approved.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "on" | "yes"
    );
    match state.agent.decide_approval(&call_id, approved).await {
        Ok(_) if partial => Ok(partial_ok(if approved { "approved" } else { "rejected" })),
        Ok(_) => {
            // key 写成 `notice=` 字面量，供源码扫描守门对账（见 handlers::NOTICE_TEXTS）。
            let notice_query = if approved {
                "notice=approved"
            } else {
                "notice=rejected"
            };
            Ok(redirect(&format!(
                "/threads/{}?{notice_query}",
                form.thread_id
            )))
        }
        Err(error) if partial => Ok(partial_err(format!(
            "提交审批失败：{}",
            public_reason(&error)
        ))),
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

/// 重命名/删除会话的回跳目标：侧栏动作可能发生在**非当前**会话的行上，
/// `back` 记录提交时所在的会话，动作后回到那里；删的就是当前会话时回首页。
fn thread_action_back(form_back: Option<&str>, deleted_or_target: &str) -> String {
    match form_back.map(str::trim).filter(|back| !back.is_empty()) {
        Some(back) if back != deleted_or_target => format!("/threads/{back}"),
        _ => "/".to_string(),
    }
}

#[derive(Debug, Deserialize)]
pub struct RenameThreadForm {
    title: String,
    back: Option<String>,
}

/// `POST /threads/{id}/rename`：重命名会话（`thread.update` 的 title 字段）。
pub async fn rename_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Form(form): Form<RenameThreadForm>,
) -> Result<Response, AppError> {
    let title = form.title.trim();
    let back = thread_action_back(form.back.as_deref(), &thread_id);
    if title.is_empty() {
        return Ok(redirect(&format!(
            "{back}?error={}",
            encode_query("会话标题不能为空")
        )));
    }
    match state.agent.rename_thread(&thread_id, title).await {
        Ok(_) => Ok(redirect(&format!("{back}?notice=thread_renamed"))),
        Err(error) => {
            if back == "/" {
                tracing::warn!("重命名会话失败: {error:#}");
                let message = format!("重命名会话失败：{}", public_reason(&error));
                Ok(redirect(&format!("/?error={}", encode_query(&message))))
            } else {
                Ok(action_failed(&back, "重命名会话失败", error))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DeleteThreadForm {
    back: Option<String>,
}

/// `POST /threads/{id}/delete`：删除会话（后端连带删落盘文件；前端有 data-confirm）。
///
/// 删的就是当前会话（或没有回跳目标）时回首页——首页会自动选中下一条；
/// 删的是别的会话时回提交时所在的会话。
pub async fn delete_thread(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Form(form): Form<DeleteThreadForm>,
) -> Result<Response, AppError> {
    if thread_id.trim().is_empty() {
        return Err(AppError::bad("删除会话缺少 threadId"));
    }
    match state.agent.delete_thread(&thread_id).await {
        Ok(_) => Ok(redirect(&format!(
            "{}?notice=thread_deleted",
            thread_action_back(form.back.as_deref(), &thread_id)
        ))),
        Err(error) => {
            let back = thread_action_back(form.back.as_deref(), &thread_id);
            if back == "/" {
                tracing::warn!("删除会话失败: {error:#}");
                let message = format!("删除会话失败：{}", public_reason(&error));
                Ok(redirect(&format!("/?error={}", encode_query(&message))))
            } else {
                Ok(action_failed(&back, "删除会话失败", error))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ApprovalModeForm {
    mode: String,
    thread_id: Option<String>,
}

/// `POST /approval/mode`：页内切换审批档位（`config.setApproval`）。
///
/// 后端会落盘并广播——快照 `config.approval` 变化触发 parts 重发，弹层与
/// 药丸随之更新。「完全访问」在弹层里带 data-confirm，这里再做一次白名单校验。
pub async fn set_approval_mode(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ApprovalModeForm>,
) -> Result<Response, AppError> {
    let partial = wants_partial(&headers);
    let mode = form.mode.trim();
    if !matches!(mode, "auto" | "ask" | "readonly" | "full") {
        let message = format!("未知审批档位: {mode}");
        return if partial {
            Ok(partial_err(message))
        } else {
            Err(AppError::bad(message))
        };
    }
    let back = match form.thread_id.as_deref().map(str::trim) {
        Some(id) if !id.is_empty() => format!("/threads/{id}"),
        _ => "/".to_string(),
    };
    match state.agent.set_approval_mode(mode).await {
        Ok(_) if partial => Ok(partial_ok(&format!("approval_{mode}"))),
        Ok(_) => Ok(redirect(&format!("{back}?notice=approval_{}", encode_query(mode)))),
        Err(error) if partial => Ok(partial_err(format!(
            "切换审批档位失败：{}",
            public_reason(&error)
        ))),
        Err(error) => {
            if back == "/" {
                tracing::warn!("切换审批档位失败: {error:#}");
                let message = format!("切换审批档位失败：{}", public_reason(&error));
                Ok(redirect(&format!("/?error={}", encode_query(&message))))
            } else {
                Ok(action_failed(&back, "切换审批档位失败", error))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ThreadModeForm {
    mode: String,
}

/// `POST /threads/{id}/mode`：空会话里选模式（对齐 原桌面端 的 ModeSlider）。
///
/// 只允许空会话——网关对已开始的会话会拒绝（`thread.setMode`），这里把它的
/// 原因如实回给用户；成功后回跳会话页，顶栏模式徽标与空状态的选中态一起刷新。
pub async fn set_thread_mode(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Form(form): Form<ThreadModeForm>,
) -> Result<Response, AppError> {
    let mode = form.mode.trim().to_ascii_lowercase();
    if !matches!(mode.as_str(), "code" | "plan" | "create" | "pm") {
        return Ok(redirect(&format!(
            "/threads/{thread_id}?error={}",
            encode_query(&format!("未知的会话模式: {mode}"))
        )));
    }
    match state.agent.set_thread_mode(&thread_id, &mode).await {
        Ok(_) => Ok(redirect(&format!(
            "/threads/{thread_id}?notice=thread_mode_set"
        ))),
        Err(error) => Ok(action_failed(&thread_id, "切换会话模式失败", error)),
    }
}

#[derive(Debug, Deserialize)]
pub struct ThreadWorkspaceForm {
    /// 空状态药丸菜单提交 `workspace`；`/workspaces?bind=` 的选目录表单提交 `path`。
    workspace: Option<String>,
    path: Option<String>,
}

/// `POST /threads/{id}/workspace`：把空会话绑到另一个工作区。
///
/// 与 `/workspaces/select` 的区别：**不切换** agent 的当前项目，也不新建会话——
/// 只改这条会话的属性（`thread.setWorkspace`）。
pub async fn set_thread_workspace(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    Form(form): Form<ThreadWorkspaceForm>,
) -> Result<Response, AppError> {
    let path = form
        .workspace
        .as_deref()
        .or(form.path.as_deref())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string);
    let Some(path) = path else {
        return Ok(redirect(&format!(
            "/threads/{thread_id}?error={}",
            encode_query("切换工作区需要目录路径")
        )));
    };
    match state.agent.set_thread_workspace(&thread_id, &path).await {
        Ok(_) => Ok(redirect(&format!(
            "/threads/{thread_id}?notice=thread_workspace_set"
        ))),
        Err(error) => Ok(action_failed(&thread_id, "切换会话工作区失败", error)),
    }
}

/// 提问作答表单（核心托盘与 `thread.tray` 插件共用字段约定，见
/// `crate::questions::answer_params_from_form`）。
///
/// 字段是动态的（多选的同名多值、多题的 `q_*` 前缀），所以收成键值对序列
/// 而不是固定 struct；`thread_id` 只用来决定失败后跳回哪个会话。
pub async fn answer_question(
    State(state): State<AppState>,
    Path(call_id): Path<String>,
    Form(form): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    if call_id.trim().is_empty() {
        return Err(AppError::bad("提问缺少 callId"));
    }
    let thread_id = form
        .iter()
        .find(|(key, _)| key == "thread_id")
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_default();
    let back = |query: &str| {
        if thread_id.is_empty() {
            redirect(&format!("/{query}"))
        } else {
            redirect(&format!("/threads/{thread_id}?{query}"))
        }
    };
    let params = match crate::questions::answer_params_from_form(&form) {
        Ok(params) => params,
        Err(message) => return Ok(back(&format!("error={}", encode_query(&message)))),
    };
    match state.agent.answer_question(&call_id, params).await {
        // `resolved=false`：callId 不认识（已超时/已在别处作答）——如实告知，
        // 不假装成功；后端 dispatch 已经把它从 pending 列表摘掉了，托盘会自己消失。
        Ok(result) if result.get("resolved").and_then(Value::as_bool) == Some(true) => {
            Ok(back("notice=answered"))
        }
        Ok(_) => Ok(back(&format!(
            "error={}",
            encode_query("这条提问已失效（可能已超时或已作答）")
        ))),
        Err(error) => {
            if thread_id.is_empty() {
                tracing::warn!("提交回答失败: {error:#}");
                let message = format!("提交回答失败：{}", public_reason(&error));
                Ok(redirect(&format!("/?error={}", encode_query(&message))))
            } else {
                Ok(action_failed(&thread_id, "提交回答失败", error))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct QueueActionForm {
    /// 界面渲染那一帧的文本指纹（`queue_rows` 同源，截断 240 字符）。
    /// 缺省（旧页面 / 手写请求）时不做校验，只保护真实界面里的陈旧帧。
    expect: Option<String>,
}

/// 排队动作的陈旧帧防护：与服务端当前队列里同下标消息的文本比对。
/// 快照拿不到就放行——真正的门槛是紧接着的 RPC，网关挂着时它同样会失败。
async fn queue_expect_current(
    state: &AppState,
    thread_id: &str,
    index: usize,
    expect: Option<&str>,
) -> bool {
    let Some(expect) = expect else {
        return true;
    };
    let Ok(snapshot) = state.agent.snapshot().await else {
        return true;
    };
    queue_rows(&snapshot, thread_id)
        .into_iter()
        .find(|row| row.index == index)
        .map(|row| row.text == expect)
        .unwrap_or(false)
}

fn queue_stale(thread_id: &str, partial: bool) -> Response {
    let message = "排队已变化（可能已被其它客户端改动），请确认后再操作";
    if partial {
        return partial_err(message);
    }
    redirect(&format!(
        "/threads/{thread_id}?error={}",
        encode_query(message)
    ))
}

pub async fn promote_queue(
    State(state): State<AppState>,
    Path((thread_id, index)): Path<(String, usize)>,
    headers: HeaderMap,
    Form(form): Form<QueueActionForm>,
) -> Result<Response, AppError> {
    let partial = wants_partial(&headers);
    if !queue_expect_current(&state, &thread_id, index, form.expect.as_deref()).await {
        return Ok(queue_stale(&thread_id, partial));
    }
    match state.agent.promote_queue(index, &thread_id).await {
        Ok(_) if partial => Ok(partial_ok("queue_promoted")),
        Ok(_) => Ok(redirect(&format!(
            "/threads/{thread_id}?notice=queue_promoted"
        ))),
        Err(error) if partial => Ok(partial_err(format!(
            "提升排队消息失败：{}",
            public_reason(&error)
        ))),
        Err(error) => Ok(action_failed(&thread_id, "提升排队消息失败", error)),
    }
}

pub async fn remove_queue(
    State(state): State<AppState>,
    Path((thread_id, index)): Path<(String, usize)>,
    headers: HeaderMap,
    Form(form): Form<QueueActionForm>,
) -> Result<Response, AppError> {
    let partial = wants_partial(&headers);
    if !queue_expect_current(&state, &thread_id, index, form.expect.as_deref()).await {
        return Ok(queue_stale(&thread_id, partial));
    }
    match state.agent.remove_queue(index).await {
        Ok(_) if partial => Ok(partial_ok("queue_removed")),
        Ok(_) => Ok(redirect(&format!(
            "/threads/{thread_id}?notice=queue_removed"
        ))),
        Err(error) if partial => Ok(partial_err(format!(
            "删除排队消息失败：{}",
            public_reason(&error)
        ))),
        Err(error) => Ok(action_failed(&thread_id, "删除排队消息失败", error)),
    }
}

pub async fn clear_queue(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let partial = wants_partial(&headers);
    match state.agent.clear_queue(&thread_id).await {
        Ok(_) if partial => Ok(partial_ok("queue_cleared")),
        Ok(_) => Ok(redirect(&format!(
            "/threads/{thread_id}?notice=queue_cleared"
        ))),
        Err(error) if partial => Ok(partial_err(format!(
            "清空排队消息失败：{}",
            public_reason(&error)
        ))),
        Err(error) => Ok(action_failed(&thread_id, "清空排队消息失败", error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn collect_images_accepts_whitelisted_data_uris() {
        let images = collect_images(&pairs(&[
            ("text", "hi"),
            ("images", "data:image/png;base64,AA"),
            ("images", "data:image/jpeg;base64,BB"),
        ]))
        .expect("valid images");
        assert_eq!(images.len(), 2);
    }

    #[test]
    fn collect_images_rejects_other_schemes_and_too_many() {
        assert!(collect_images(&pairs(&[("images", "data:text/html;base64,AA")])).is_err());
        assert!(collect_images(&pairs(&[("images", "https://evil.invalid/a.png")])).is_err());
        let too_many: Vec<(String, String)> = (0..=MAX_IMAGES)
            .map(|_| ("images".to_string(), "data:image/png;base64,AA".to_string()))
            .collect();
        assert!(collect_images(&too_many).is_err());
    }

    #[test]
    fn collect_images_rejects_oversized_uri() {
        let oversized = format!(
            "data:image/png;base64,{}",
            "A".repeat(MAX_IMAGE_DATA_URI)
        );
        assert!(collect_images(&pairs(&[("images", oversized.as_str())])).is_err());
    }

    #[test]
    fn collect_images_ignores_blank_values() {
        assert!(collect_images(&pairs(&[("images", "  ")]))
            .expect("blank is fine")
            .is_empty());
    }

    #[test]
    fn approval_picker_options_have_svg_icons_and_current_check() {
        let html = approval_picker_html("ask", "每次询问", "warn", "t1");
        for label in ["自动批准", "每次询问", "严格只读", "完全访问"] {
            assert!(html.contains(label), "缺少档位：{label}");
        }
        // 4 个选项各一个图标 + 当前项的勾 + 药丸本体 + caret。
        assert_eq!(
            html.matches("<svg class=\"ada-icon\"").count(),
            7,
            "每个档位都应有 SVG 图标：{html}"
        );
        assert!(html.contains("ada-approval-picker__check"), "{html}");
        assert!(!html.contains("TSX 插槽"), "工具栏不应再有演示胶囊");
    }
}
