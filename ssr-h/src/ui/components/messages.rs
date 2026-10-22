//! 消息行、提示卡与空状态。

use super::button::Button;

use crate::ui::icons::icon;
use crate::ui::theme::{
    attr, class_list, escape, space_attrs, ButtonKind, ButtonVariant, Size, Tone,
};

use super::Render;

// ── 消息行 ───────────────────────────────────────────────────────────────

/// 用户消息气泡。
#[derive(Debug, Clone)]
pub struct UserMessage {
    pub text: String,
    /// 附带图片数量（当前 SSR 版不渲染图片内容，但不能装作没有）。
    pub images: usize,
}

impl Render for UserMessage {
    fn render(&self) -> String {
        let images = if self.images > 0 {
            format!(
                r#"<span class="ada-msg__images" title="ssr-h 暂不显示图片内容">含 {} 张图片</span>"#,
                self.images
            )
        } else {
            String::new()
        };
        format!(
            r#"<div class="ada-msg ada-msg--user"><div class="ada-bubble">{}{}</div></div>"#,
            escape(&self.text),
            images
        )
    }
}

/// 助手消息：头像 + Markdown 正文。
#[derive(Debug, Clone)]
pub struct AssistantMessage {
    /// 已清洗的 Markdown HTML。
    pub html: String,
}

impl Render for AssistantMessage {
    fn render(&self) -> String {
        format!(
            r#"<div class="ada-msg ada-msg--assistant"><span class="ada-avatar" title="助手">{avatar}</span><div class="ada-msg__body markdown-body ada-markdown">{body}</div></div>"#,
            avatar = icon("i-bot", 14),
            body = self.html,
        )
    }
}

/// 提示卡（失败 / 警告 / 系统提示），可带一个动作。
#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    pub tone: Tone,
    /// 动作按钮（重试等）。
    pub action: Option<Button>,
}

impl Notice {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            action: None,
        }
    }

    pub fn action(mut self, action: Button) -> Self {
        self.action = Some(action);
        self
    }
}

impl Render for Notice {
    fn render(&self) -> String {
        let action = self
            .action
            .as_ref()
            .map(|button| {
                format!(
                    r#"<div class="ada-notice__action">{}</div>"#,
                    button.render()
                )
            })
            .unwrap_or_default();
        format!(
            r#"<div class="ada-notice ada-notice--{tone}"><div class="ada-notice__row">{icon}<span>{text}</span></div>{action}</div>"#,
            tone = self.tone.class(),
            icon = icon("i-alert", 14),
            text = escape(&self.text),
            action = action,
        )
    }
}

/// 空状态。
#[derive(Debug, Clone)]
pub struct EmptyState {
    pub icon: String,
    pub title: String,
    pub description: Option<String>,
    pub action: Option<Button>,
}

impl Render for EmptyState {
    fn render(&self) -> String {
        let description = self
            .description
            .as_deref()
            .map(|text| format!("<p class=\"ada-empty__desc\">{}</p>", escape(text)))
            .unwrap_or_default();
        let action = self
            .action
            .as_ref()
            .map(|button| format!("<div class=\"ada-empty__action\">{}</div>", button.render()))
            .unwrap_or_default();
        format!(
            r#"<div class="ada-empty"><span class="ada-empty__mark">{icon}</span><h2 class="ada-empty__title">{title}</h2>{description}{action}</div>"#,
            icon = icon(&self.icon, 18),
            title = escape(&self.title),
            description = description,
            action = action,
        )
    }
}
