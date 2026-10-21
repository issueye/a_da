//! 状态药丸（模式 / 审批档位 / 模型）。

use crate::ui::icons::icon;
use crate::ui::theme::{
    attr, class_list, escape, space_attrs, ButtonKind, ButtonVariant, Size, Tone,
};

use super::Render;

// ── 药丸 ─────────────────────────────────────────────────────────────────

/// 药丸：图标 + 文本，用于只读状态展示（模式、审批档位、模型）。
#[derive(Debug, Clone)]
pub struct Chip {
    pub text: String,
    pub icon: Option<String>,
    pub tone: Tone,
    /// 末尾是否带一个下拉箭头（表达"本可展开"，但当前只读）。
    pub caret: bool,
    pub title: Option<String>,
    pub extra_class: Option<String>,
}

impl Chip {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            icon: None,
            tone,
            caret: false,
            title: None,
            extra_class: None,
        }
    }

    pub fn icon(mut self, key: impl Into<String>) -> Self {
        self.icon = Some(key.into());
        self
    }

    pub fn caret(mut self, caret: bool) -> Self {
        self.caret = caret;
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn class(mut self, class: impl Into<String>) -> Self {
        self.extra_class = Some(class.into());
        self
    }
}

impl Render for Chip {
    fn render(&self) -> String {
        let classes = class_list(&[
            "ada-chip",
            &format!("ada-chip--{}", self.tone.class()),
            self.extra_class.as_deref().unwrap_or(""),
        ]);
        let icon_html = self
            .icon
            .as_deref()
            .map(|key| icon(key, 13))
            .unwrap_or_default();
        let caret = if self.caret {
            icon("i-chevron-down", 12)
        } else {
            String::new()
        };
        let title = attr("title", self.title.as_deref().unwrap_or(""));
        format!(
            r#"<span class="{classes}"{title}>{icon_html}<span class="ada-chip__text">{}</span>{caret}</span>"#,
            escape(&self.text)
        )
    }
}
