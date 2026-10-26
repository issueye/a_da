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
    /// 附带图片数量。
    pub images: usize,
    /// 可渲染的图片 data URI（渲染侧已按格式白名单过滤）。
    pub image_srcs: Vec<String>,
}

impl Render for UserMessage {
    fn render(&self) -> String {
        // 图片在上、文字在下：和 原桌面端 的用户消息一致。
        let images = if !self.image_srcs.is_empty() {
            format!(
                r#"<div class="ada-msg__image-grid">{}</div>"#,
                self.image_srcs
                    .iter()
                    .enumerate()
                    .map(|(index, src)| format!(
                        r#"<a class="ada-msg__image" href="{src}" target="_blank" rel="noreferrer"><img src="{src}" alt="图片 {number}" loading="lazy"></a>"#,
                        src = escape(src),
                        number = index + 1,
                    ))
                    .collect::<String>()
            )
        } else if self.images > 0 {
            // 数据在但不是可渲染的 data URI（历史/坏数据）：如实说数量。
            format!(
                r#"<span class="ada-msg__images">含 {} 张图片（当前无法预览）</span>"#,
                self.images
            )
        } else {
            String::new()
        };
        let text = if self.text.trim().is_empty() {
            String::new()
        } else {
            escape(&self.text)
        };
        format!(
            r#"<div class="ada-msg ada-msg--user"><div class="ada-bubble">{}{}</div></div>"#,
            images, text
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
