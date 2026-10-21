//! 对话流"行"的视图模型与渲染。
//!
//! 领域数据（agent 快照里的 item）先归一化成 [`RowView`]，
//! 再由组件库渲染成 HTML。模板因此只剩"循环输出行"这一件事，
//! 行内结构、状态语义、复制按钮等细节全部由组件库统一负责。

use super::components::{
    AssistantMessage, Button, Disclosure, EmptyState, Notice, Panel, Render, UserMessage,
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
        "tool" => {
            let mut row = Disclosure::new("tool", &view.icon, &view.label);
            if !view.key.is_empty() {
                row = row.key(view.key.clone());
            }
            if !view.target.is_empty() {
                row = row.detail(view.target.clone());
            }
            if !view.dir.is_empty() {
                row = row.sub(view.dir.clone());
            }
            if !view.elapsed.is_empty() {
                row = row.meta(view.elapsed.clone());
            }
            if !view.status.is_empty() && view.status_tone != Tone::Neutral {
                row = row.badge(view.status.clone(), view.status_tone);
            }
            let mut body = String::new();
            // todo 工具：先把任务清单排版出来，原始 JSON 退居其次。
            if !view.todo_steps.is_empty() {
                body.push_str(
                    &super::todo::TodoList {
                        steps: view.todo_steps.clone(),
                        notes: view.todo_notes.clone(),
                    }
                    .render(),
                );
            }
            if !view.args.is_empty() {
                body.push_str(
                    &Panel::new(view.args.clone())
                        .title("输入参数")
                        .copy(view.args.clone())
                        .render(),
                );
            }
            if !view.output.is_empty() {
                let panel = if view.output_html.is_empty() {
                    Panel::new(view.output.clone())
                } else {
                    Panel::new(String::new()).html(view.output_html.clone())
                };
                body.push_str(&panel.title("执行输出").copy(view.output.clone()).render());
            }
            if body.is_empty() {
                body.push_str(
                    &Panel::new(String::new())
                        .html("<p>这个工具没有返回可展示的内容。</p>")
                        .render(),
                );
            }
            row = row.body(body);
            row.render()
        }
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
