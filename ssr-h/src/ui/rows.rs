//! 对话流"行"的视图模型与渲染。
//!
//! 领域数据（agent 快照里的 item）先归一化成 [`RowView`]，
//! 再由组件库渲染成 HTML。模板因此只剩"循环输出行"这一件事，
//! 行内结构、状态语义、复制按钮等细节全部由组件库统一负责。

use super::components::{
    AssistantMessage, Button, Disclosure, EmptyState, Notice, Panel, Render, ToolCall, UserMessage,
};
use super::icons::icon;
use super::theme::{escape, ButtonVariant, Size, Tone};

/// 一行对话内容（已归一化）。
#[derive(Debug, Clone, Default)]
pub struct RowView {
    /// `user` / `assistant` / `thinking` / `tool` / `notice` / `compact`。
    pub kind: String,
    /// 快照里的 item id：live 轮询替换 DOM 后按它恢复展开状态。
    pub key: String,
    /// 用户消息附带的图片数量。
    pub images: usize,
    /// 可渲染的图片 data URI（已按格式白名单过滤；数量与 `images` 可能不同）。
    pub image_srcs: Vec<String>,
    /// 纯文本内容。
    pub text: String,
    /// 助手正文的 Markdown HTML。
    pub html: String,
    /// 工具中文标签。
    pub label: String,
    /// 工具目标对象。
    pub target: String,
    /// 目标对象的目录部分。
    pub dir: String,
    /// 图标键。
    pub icon: String,
    /// 状态文案。
    pub status: String,
    /// 状态语义色。
    pub status_tone: Tone,
    /// 折叠态的结果尾标：`167 行` / `+18 -4` / `exit 0`。
    ///
    /// 用户扫读对话时最想知道的不是"它做了什么"（标签已经说了），
    /// 而是"这一步得到了什么"——成败与规模必须留在折叠行上。
    pub result: String,
    /// 结果尾标的语义色（只有异常与增删数着色，普通计数保持中性）。
    pub result_tone: Tone,
    /// 是否仍在执行（驱动左侧色条与客户端每秒计时）。
    pub running: bool,
    /// 实时计时基点（epoch 毫秒）：只有执行中才有值。
    pub started_at: Option<i64>,
    /// 输出总行数（按截断前的原文统计）：长输出按钮的文案依据。
    pub output_lines: usize,
    /// 耗时。
    pub elapsed: String,
    /// 原始参数。
    pub args: String,
    /// 原始输出。
    pub output: String,
    /// 输出渲染后的 HTML（Markdown）。
    pub output_html: String,
    /// 思考末行预览。
    pub preview: String,
    /// 提示级别：`error` / `warn` / 其它。
    pub level: String,
    /// `todo` 工具的任务清单（其它工具为空）。
    pub todo_steps: Vec<super::todo::TodoStep>,
    /// `todo` 工具的备注。
    pub todo_notes: Option<String>,
}

impl RowView {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            ..Default::default()
        }
    }
}

/// 渲染一行。
pub fn render_row(view: &RowView, thread_id: &str) -> String {
    match view.kind.as_str() {
        "user" => UserMessage {
            text: view.text.clone(),
            images: view.images,
            image_srcs: view.image_srcs.clone(),
        }
        .render(),
        "assistant" => AssistantMessage {
            html: view.html.clone(),
        }
        .render(),
        "thinking" => {
            let mut row = Disclosure::new("thinking", "i-brain", "思考");
            if !view.key.is_empty() {
                row = row.key(view.key.clone());
            }
            if !view.elapsed.is_empty() {
                row = row.meta(format!("· {}", view.elapsed));
            }
            if !view.preview.is_empty() {
                row = row.detail(format!("“{}”", view.preview)).detail_muted(true);
            }
            row = row.body(
                Panel::new(view.text.clone())
                    .title("推理分析")
                    .copy(view.text.clone())
                    .render(),
            );
            row.render()
        }
        "tool" => ToolCall::new(
            &view.icon,
            &view.label,
            &view.target,
            &view.dir,
            &view.elapsed,
            &view.status,
            view.status_tone,
            &view.args,
            &view.output,
            &view.output_html,
            &view.todo_steps,
            view.todo_notes.as_deref(),
        )
        .key(&view.key)
        .result(&view.result, view.result_tone)
        .running(view.running)
        .started_at(view.started_at)
        .total_lines(view.output_lines)
        .render(),
        "notice" => {
            let tone = match view.level.as_str() {
                "error" => Tone::Danger,
                "warn" => Tone::Warn,
                _ => Tone::Brand,
            };
            let mut notice = Notice::new(view.text.clone(), tone);
            if view.level == "error" && !thread_id.is_empty() {
                notice = notice.action(
                    Button::new("重试这一轮")
                        .variant(ButtonVariant::Secondary)
                        .size(Size::Sm)
                        .icon("i-refresh")
                        .action(format!("/threads/{thread_id}/retry")),
                );
            }
            notice.render()
        }
        "compact" => {
            let mut row = Disclosure::new("compact", "i-layers", "上下文压缩");
            if !view.key.is_empty() {
                row = row.key(view.key.clone());
            }
            row.body(
                Panel::new(String::new())
                    .html(format!("<p>{}</p>", super::theme::escape(&view.text)))
                    .render(),
            )
            .render()
        }
        _ => format!(
            r#"<div class="ada-msg ada-msg--plain"><div class="ada-plain">{}</div></div>"#,
            super::theme::escape(&view.text)
        ),
    }
}

/// 空会话状态（live 换帧时用；不带页内控件）。
pub fn render_empty_thread() -> String {
    EmptyState {
        icon: "i-chat".to_string(),
        title: "这个会话还没有消息".to_string(),
        description: Some("在下方输入内容并发送，agent 的回复会出现在这里。".to_string()),
        action: None,
    }
    .render()
}

/// 新会话空状态里的一个工作区选项。
#[derive(Debug, Clone, Default)]
pub struct WorkspaceChoice {
    pub path: String,
    pub name: String,
    pub count: usize,
    pub current: bool,
}

/// 新会话空状态：品牌位 + 模式分段 + 工作区选择。
///
/// 取代原先「新建会话」弹窗里的两项选择（弹窗已移除）：点侧栏「新会话」直接
/// 建一条空会话，模式与工作区就在这条会话的空状态里选，选完即时生效。
/// **只在这个会话还没有任何消息时渲染**（`chat.html` 按 `items.is_empty()` 分支）；
/// 发送第一条消息后整页跳回，控件自然消失。模式的"开始后不可切换"由网关的
/// `thread.setMode` 兜底，工作区同理只在空状态暴露入口。
pub fn render_new_conversation(
    thread_id: &str,
    mode: &str,
    workspace_name: &str,
    workspace_path: &str,
    workspaces: &[WorkspaceChoice],
) -> String {
    let modes = [
        ("code", "编码", "i-code"),
        ("plan", "规划", "i-list"),
        ("create", "创造", "i-sparkles"),
        ("pm", "PM", "i-layers"),
    ];
    let current_mode = match mode.trim().to_ascii_lowercase().as_str() {
        "plan" => "plan",
        "create" => "create",
        "pm" => "pm",
        _ => "code",
    };
    let mode_html = modes
        .iter()
        .map(|(value, label, icon_key)| {
            let selected = *value == current_mode;
            format!(
                r#"<form class="ada-newconv__mode-form" method="post" action="/threads/{id}/mode"><input type="hidden" name="mode" value="{value}"><button class="ada-wspick__mode{current}" type="submit"{disabled} title="{label}">{glyph}<span>{label}</span></button></form>"#,
                id = escape(thread_id),
                value = value,
                label = label,
                glyph = icon(icon_key, 12),
                current = if selected { " is-current" } else { "" },
                disabled = if selected {
                    r#" disabled aria-pressed="true""#
                } else {
                    r#" aria-pressed="false""#
                },
            )
        })
        .collect::<String>();

    let items = if workspaces.is_empty() {
        format!(
            r#"<p class="ada-newconv__ws-empty">还没有可选的工作区；先<strong>选择工作区</strong>添加一个目录。</p>"#
        )
    } else {
        workspaces
            .iter()
            .map(|choice| {
                format!(
                    r#"<form method="post" action="/threads/{id}/workspace"><input type="hidden" name="workspace" value="{path}"><button class="ada-newconv__ws-item{current}" type="submit"{disabled} title="{path}">{check}<span class="ada-newconv__ws-name">{name}</span><span class="ada-newconv__ws-count">{count}</span></button></form>"#,
                    id = escape(thread_id),
                    path = escape(&choice.path),
                    current = if choice.current { " is-current" } else { "" },
                    disabled = if choice.current {
                        r#" disabled aria-current="true""#
                    } else {
                        ""
                    },
                    check = icon("i-check", 12),
                    name = escape(&choice.name),
                    count = choice.count,
                )
            })
            .collect::<String>()
    };

    format!(
        r#"<div class="ada-empty ada-empty--newconv">
  <span class="ada-empty__mark"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 2.5 21.5 12 12 21.5 2.5 12Z"/></svg></span>
  <h2 class="ada-empty__title">开始一段新的对话</h2>
  <p class="ada-empty__desc">模式在开始后不可切换；工作区只绑定这条会话，不切换当前项目。</p>
  <div class="ada-newconv__controls">
    <div class="ada-wspick__modes" role="group" aria-label="会话模式">{modes}</div>
    <div class="ada-newconv__ws-row">
      <div class="ada-newconv__ws-group">
        <details class="ada-popover ada-newconv__ws" id="newconv-workspace">
          <summary class="ada-newconv__ws-trigger" title="当前工作区：{path}">{folder}<span class="ada-newconv__ws-label">{name}</span>{caret}</summary>
          <div class="ada-newconv__ws-menu" role="menu">{items}<a class="ada-newconv__ws-more" href="/workspaces?bind={id}">浏览全部工作区…</a></div>
        </details>
        <a class="ada-newconv__ws-select" href="/workspaces?bind={id}" data-modal-open="workspace-picker">{folder}{select_label}</a>
      </div>
    </div>
  </div>
</div>"#,
        modes = mode_html,
        path = escape(workspace_path),
        name = escape(workspace_name),
        folder = icon("i-folder", 14),
        caret = icon("i-chevron-down", 12),
        select_label = "选择工作区",
        items = items,
        id = escape(thread_id),
    )
}

/// 侧栏/主区的通用空状态。
pub fn render_empty(title: &str, description: &str, icon: &str) -> String {
    EmptyState {
        icon: icon.to_string(),
        title: title.to_string(),
        description: Some(description.to_string()),
        action: None,
    }
    .render()
}
