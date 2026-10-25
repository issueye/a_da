//! 按钮类：文本按钮、图标按钮、发送按钮。

use crate::ui::icons::icon;
use crate::ui::theme::{
    attr, class_list, escape, space_attrs, ButtonKind, ButtonVariant, Size, Tone,
};

use super::Render;

// ── 按钮 ─────────────────────────────────────────────────────────────────

/// 按钮：链接与提交按钮共用一套外观。
#[derive(Debug, Clone, Default)]
pub struct Button {
    pub label: String,
    pub variant: Option<ButtonVariant>,
    pub size: Option<Size>,
    pub kind: Option<ButtonKind>,
    pub icon: Option<String>,
    /// 给了 `href` 就渲染成 `<a>`，否则渲染成 `<button>`。
    pub href: Option<String>,
    pub action: Option<String>,
    pub method: Option<String>,
    pub name: Option<String>,
    pub value: Option<String>,
    pub title: Option<String>,
    pub extra_class: Option<String>,
    /// 额外属性（`data-*` 等），原样拼在标签上。
    pub extra_attrs: Option<String>,
}

impl Button {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..Default::default()
        }
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = Some(variant);
        self
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = Some(size);
        self
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn icon(mut self, key: impl Into<String>) -> Self {
        self.icon = Some(key.into());
        self
    }

    pub fn href(mut self, href: impl Into<String>) -> Self {
        self.href = Some(href.into());
        self
    }

    pub fn action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
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

    /// 追加任意属性（`data-*` 等）。
    pub fn attrs(mut self, attrs: impl Into<String>) -> Self {
        self.extra_attrs = Some(attrs.into());
        self
    }
}

impl Render for Button {
    fn render(&self) -> String {
        let variant = self.variant.unwrap_or(ButtonVariant::Secondary);
        let size = self.size.unwrap_or(Size::Md);
        let classes = class_list(&[
            "ada-btn",
            &format!("ada-btn--{}", variant.class()),
            &format!("ada-btn--{}", size.class()),
            self.extra_class.as_deref().unwrap_or(""),
        ]);
        let icon_html = self
            .icon
            .as_deref()
            .map(|key| icon(key, 14))
            .unwrap_or_default();
        let body = format!(
            "{icon_html}<span class=\"ada-btn__label\">{}</span>",
            escape(&self.label)
        );
        let title = attr("title", self.title.as_deref().unwrap_or(""));

        if let Some(href) = &self.href {
            return format!(
                r#"<a class="{classes}" href="{href}"{title} rel="noreferrer">{body}</a>"#,
                href = escape(href)
            );
        }

        // 🔴 `action` 必须变成一个**真正的表单**：把 `action="/x"` 写到 <button> 上
        // 不是 HTML 语义，浏览器永远不会提交它（历史 bug：输入框下方的
        // 中止/清空队列/重试、Notice 里的「重试这一轮」点了都没反应）。
        if let Some(action) = &self.action {
            let method = self
                .method
                .as_deref()
                .filter(|method| !method.trim().is_empty())
                .unwrap_or("post");
            // `data-confirm` 等属性要落在 form 上：共享确认对话框监听的是 `form[data-confirm]`。
            let extra = space_attrs(self.extra_attrs.as_deref());
            return format!(
                r#"<form class="ada-action-form" method="{method}" action="{action}"{extra}><button class="{classes}" type="submit"{title}>{body}</button></form>"#,
                method = escape(method),
                action = escape(action),
            );
        }

        let name = attr("name", self.name.as_deref().unwrap_or(""));
        let value = attr("value", self.value.as_deref().unwrap_or(""));
        let kind = self.kind.unwrap_or(ButtonKind::Submit);
        let extra = space_attrs(self.extra_attrs.as_deref());
        format!(
            r#"<button class="{classes}" type="{kind}"{name}{value}{title}{extra}>{body}</button>"#
        )
    }
}

/// 图标按钮：必须有可读标签（`aria-label`），否则等于没有。
#[derive(Debug, Clone, Default)]
pub struct IconButton {
    pub icon: String,
    pub label: String,
    pub kind: Option<ButtonKind>,
    pub href: Option<String>,
    pub action: Option<String>,
    pub title: Option<String>,
    pub extra_class: Option<String>,
    pub extra_attrs: Option<String>,
}

impl IconButton {
    pub fn new(icon: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            icon: icon.into(),
            label: label.into(),
            ..Default::default()
        }
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn href(mut self, href: impl Into<String>) -> Self {
        self.href = Some(href.into());
        self
    }

    pub fn action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }

    pub fn class(mut self, class: impl Into<String>) -> Self {
        self.extra_class = Some(class.into());
        self
    }

    pub fn attrs(mut self, attrs: impl Into<String>) -> Self {
        self.extra_attrs = Some(attrs.into());
        self
    }
}

impl Render for IconButton {
    fn render(&self) -> String {
        let classes = class_list(&["ada-icon-btn", self.extra_class.as_deref().unwrap_or("")]);
        let title = attr("title", self.title.as_deref().unwrap_or(&self.label));
        let body = icon(&self.icon, 14);
        let extra = space_attrs(self.extra_attrs.as_deref());
        if let Some(href) = &self.href {
            return format!(
                r#"<a class="{classes}" href="{href}" aria-label="{label}"{title}{extra} rel="noreferrer">{body}</a>"#,
                href = escape(href),
                label = escape(&self.label)
            );
        }
        if let Some(action) = &self.action {
            return format!(
                r#"<form class="ada-action-form" method="post" action="{action}"{extra}><button class="{classes}" type="submit" aria-label="{label}"{title}>{body}</button></form>"#,
                action = escape(action),
                label = escape(&self.label),
            );
        }
        let kind = self.kind.unwrap_or(ButtonKind::Button);
        format!(
            r#"<button class="{classes}" type="{kind}" aria-label="{label}"{title}{extra}>{body}</button>"#,
            label = escape(&self.label)
        )
    }
}

/// 发送按钮：全页最重的主操作，圆形实心。
#[derive(Debug, Clone, Default)]
pub struct SendButton {
    pub label: String,
    pub title: Option<String>,
    /// 空内容时禁用。
    pub disabled: bool,
}

impl SendButton {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            title: None,
            // 渐进增强：SSR 基线必须能提交（`required` 会挡住空内容），
            // 有 JS 时由 components.js 按输入内容即时启停。
            disabled: false,
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

impl Render for SendButton {
    fn render(&self) -> String {
        let disabled = if self.disabled { " disabled" } else { "" };
        let title = attr("title", self.title.as_deref().unwrap_or(&self.label));
        format!(
            r#"<button class="ada-send" type="submit" aria-label="{label}"{title}{disabled}>{icon}</button>"#,
            label = escape(&self.label),
            icon = icon("i-send", 14),
        )
    }
}
