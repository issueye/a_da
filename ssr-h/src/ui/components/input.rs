//! 单行输入框。

use crate::ui::icons::icon;
use crate::ui::theme::{
    attr, class_list, escape, space_attrs, ButtonKind, ButtonVariant, Size, Tone,
};

use super::Render;

// ── 输入 ─────────────────────────────────────────────────────────────────

/// 单行输入框。
#[derive(Debug, Clone, Default)]
pub struct TextField {
    pub name: String,
    pub value: String,
    pub placeholder: String,
    pub label: Option<String>,
    /// 聚焦后展示的提示文案（焦点态的一部分）。
    pub hint: Option<String>,
    pub icon: Option<String>,
    pub input_type: Option<String>,
    pub extra_attrs: Option<String>,
}

impl TextField {
    pub fn new(name: impl Into<String>, placeholder: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            placeholder: placeholder.into(),
            ..Default::default()
        }
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn icon(mut self, key: impl Into<String>) -> Self {
        self.icon = Some(key.into());
        self
    }
}

impl Render for TextField {
    fn render(&self) -> String {
        let icon_html = self
            .icon
            .as_deref()
            .map(|key| format!("<span class=\"ada-field__icon\">{}</span>", icon(key, 14)))
            .unwrap_or_default();
        let label = self
            .label
            .as_deref()
            .map(|text| format!("<span class=\"ada-field__label\">{}</span>", escape(text)))
            .unwrap_or_default();
        let hint = self
            .hint
            .as_deref()
            .map(|text| format!("<span class=\"ada-field__hint\">{}</span>", escape(text)))
            .unwrap_or_default();
        let input_type = self.input_type.as_deref().unwrap_or("text");
        let attrs = self.extra_attrs.clone().unwrap_or_default();
        format!(
            r#"<label class="ada-field">{icon_html}{label}<input class="ada-input ada-focusable" type="{input_type}" name="{name}" value="{value}" placeholder="{placeholder}"{attrs}>{hint}</label>"#,
            name = escape(&self.name),
            value = escape(&self.value),
            placeholder = escape(&self.placeholder),
        )
    }
}
