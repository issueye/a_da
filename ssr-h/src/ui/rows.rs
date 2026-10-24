//! 对话流"行"的视图模型与渲染。
//!
//! 领域数据（agent 快照里的 item）先归一化成 [`RowView`]，
//! 再由组件库渲染成 HTML。模板因此只剩"循环输出行"这一件事，
//! 行内结构、状态语义、复制按钮等细节全部由组件库统一负责。

use super::components::{
    AssistantMessage, Button, Disclosure, EmptyState, Notice, Panel, Render, ToolCall, UserMessage,
};
use super::theme::{ButtonVariant, Size, Tone};

/// 一行对话内容（已归一化）。
#[derive(Debug, Clone, Default)]
pub struct RowView {
    /// `user` / `assistant` / `thinking` / `tool` / `notice` / `compact`。
    pub kind: String,
    /// 快照里的 item id：live 轮询替换 DOM 后按它恢复展开状态。
    pub key: String,
    /// 用户消息附带的图片数量。
    pub images: usize,
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

/// 空会话状态。
pub fn render_empty_thread() -> String {
    EmptyState {
        icon: "i-chat".to_string(),
        title: "这个会话还没有消息".to_string(),
        description: Some("在下方输入内容并发送，agent 的回复会出现在这里。".to_string()),
        action: None,
    }
    .render()
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
