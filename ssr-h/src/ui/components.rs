//! 服务端组件库：把页面里重复出现的 UI 抽象成**可复用的渲染单元**。
//!
//! 为什么要服务端渲染组件而不是只写 CSS 类：
//!
//! - 组件的 DOM 结构、状态属性、焦点语义是一份契约，模板与插件都应共用；
//! - 插件插槽需要在 SSR 期插入 HTML，服务端组件天然支持；
//! - 无 JS 时页面依然完整可读（SSR 基线），组件不做纯客户端渲染。
//!
//! 约定：
//!
//! - 组件只输出 `ada-*` class 与 `data-*` 状态，不写死颜色（颜色由 `--ph-*` 变量决定）；
//! - 所有动态文本经 [`crate::ui::theme::escape`] 转义；
//! - 组件可通过 [`Render::render`] 得到可直接 `|safe` 输出的 HTML 字符串。

use super::icons::icon;
use super::theme::{attr, class_list, escape, ButtonKind, ButtonVariant, Size, Tone};

/// 可渲染组件的统一接口。
pub trait Render {
    fn render(&self) -> String;
}

/// ── 按钮 ─────────────────────────────────────────────────────────────────

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
        let method = attr("method", self.method.as_deref().unwrap_or("post"));
        let name = attr("name", self.name.as_deref().unwrap_or(""));
        let value = attr("value", self.value.as_deref().unwrap_or(""));
        let kind = self.kind.unwrap_or(ButtonKind::Submit);
        let extra = self.extra_attrs.clone().unwrap_or_default();
        format!(
            r#"<button class="{classes}" type="{kind}"{method}{name}{value}{title}{extra}>{body}</button>"#
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
        let body = icon(&self.icon, 15);
        if let Some(href) = &self.href {
            return format!(
                r#"<a class="{classes}" href="{href}" aria-label="{label}"{title} rel="noreferrer">{body}</a>"#,
                href = escape(href),
                label = escape(&self.label)
            );
        }
        let action = attr("action", self.action.as_deref().unwrap_or(""));
        let kind = self.kind.unwrap_or(ButtonKind::Button);
        format!(
            r#"<button class="{classes}" type="{kind}" aria-label="{label}"{action}{title}>{body}</button>"#,
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
            disabled: true,
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
            icon = icon("i-send", 16),
        )
    }
}

/// ── 药丸 ─────────────────────────────────────────────────────────────────

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

/// ── 输入 ─────────────────────────────────────────────────────────────────

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
            .map(|text| {
                format!(
                    "<span class=\"ada-field__hint\">{}</span>",
                    escape(text)
                )
            })
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

/// ── 折叠（工具 / 思考 / 压缩） ────────────────────────────────────────────

/// 折叠面板：**整行可点**、没有展开箭头，靠背景与文字色区分开合状态。
#[derive(Debug, Clone)]
pub struct Disclosure {
    /// 行内图标键。
    pub icon: String,
    /// 行内主标签。
    pub label: String,
    /// 次要信息（目标对象 / 预览）。
    pub detail: Option<String>,
    /// 更次要的信息（目录）。
    pub sub: Option<String>,
    /// 尾部信息（耗时）。
    pub meta: Option<String>,
    /// 状态徽标文案。
    pub badge: Option<String>,
    pub badge_tone: Tone,
    /// 展开区内容（已渲染好的 HTML）。
    pub body: String,
    /// 行类型，用于样式与测试定位。
    pub kind: String,
    /// 行内额外属性。
    pub extra_attrs: Option<String>,
    /// 目标信息是否用弱化样式（思考预览用斜体弱色）。
    pub detail_muted: bool,
}

impl Disclosure {
    pub fn new(kind: impl Into<String>, icon: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            icon: icon.into(),
            label: label.into(),
            detail: None,
            sub: None,
            meta: None,
            badge: None,
            badge_tone: Tone::Neutral,
            body: String::new(),
            kind: kind.into(),
            extra_attrs: None,
            detail_muted: false,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// 弱化样式的信息（预览类）。
    pub fn detail_muted(mut self, muted: bool) -> Self {
        self.detail_muted = muted;
        self
    }

    pub fn sub(mut self, sub: impl Into<String>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    pub fn meta(mut self, meta: impl Into<String>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    pub fn badge(mut self, badge: impl Into<String>, tone: Tone) -> Self {
        self.badge = Some(badge.into());
        self.badge_tone = tone;
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn attrs(mut self, attrs: impl Into<String>) -> Self {
        self.extra_attrs = Some(attrs.into());
        self
    }
}

impl Render for Disclosure {
    fn render(&self) -> String {
        let detail = self
            .detail
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| {
                let extra = if self.detail_muted {
                    " ada-disclosure__detail--muted"
                } else {
                    ""
                };
                format!(
                    "<span class=\"ada-disclosure__detail{extra}\">{}</span>",
                    escape(text)
                )
            })
            .unwrap_or_default();
        let sub = self
            .sub
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| format!("<span class=\"ada-disclosure__sub\">{}</span>", escape(text)))
            .unwrap_or_default();
        let meta = self
            .meta
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| format!("<span class=\"ada-disclosure__meta\">{}</span>", escape(text)))
            .unwrap_or_default();
        let badge = self
            .badge
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| {
                format!(
                    "<span class=\"ada-badge ada-badge--{}\">{}</span>",
                    self.badge_tone.class(),
                    escape(text)
                )
            })
            .unwrap_or_default();
        let attrs = self.extra_attrs.clone().unwrap_or_default();
        // 图标位有两个角色：常态是"这一行是什么"（工具/思考图标），
        // 鼠标悬浮或键盘聚焦时换成"可以展开"（箭头）。两者占同宽，切换不跳版。
        format!(
            r#"<details class="ada-disclosure" data-kind="{kind}" data-state="closed"{attrs}><summary class="ada-disclosure__summary"><span class="ada-disclosure__icon">{icon}</span><span class="ada-disclosure__caret">{caret}</span><span class="ada-disclosure__label">{label}</span>{detail}{sub}{meta}{badge}</summary><div class="ada-disclosure__rail">{body}</div></details>"#,
            kind = escape(&self.kind),
            attrs = attrs,
            icon = icon(&self.icon, 13),
            caret = icon("i-chevron", 12),
            label = escape(&self.label),
            detail = detail,
            sub = sub,
            meta = meta,
            badge = badge,
            body = self.body,
        )
    }
}

/// 展开区里的浅底面板（带标题栏与复制按钮）。
#[derive(Debug, Clone)]
pub struct Panel {
    pub title: Option<String>,
    /// 复制按钮复制的原文；为空则不渲染复制按钮。
    pub copy_text: Option<String>,
    /// 面板内容（已渲染好的 HTML）。
    pub content: String,
    /// 内容是否按等宽原文展示（JSON/参数用）。
    pub preformatted: bool,
}

impl Panel {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            title: None,
            copy_text: None,
            content: content.into(),
            preformatted: true,
        }
    }

    pub fn html(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self.preformatted = false;
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn copy(mut self, text: impl Into<String>) -> Self {
        self.copy_text = Some(text.into());
        self
    }
}

impl Render for Panel {
    fn render(&self) -> String {
        let head = self
            .title
            .as_deref()
            .map(|title| {
                let copy = self
                    .copy_text
                    .as_deref()
                    .filter(|text| !text.is_empty())
                    .map(|text| {
                        format!(
                            r#"<button class="ada-panel__copy" type="button" data-copy="{text}" data-copy-label="复制">复制</button>"#,
                            text = escape(text)
                        )
                    })
                    .unwrap_or_default();
                format!(
                    r#"<div class="ada-panel__head"><span class="ada-panel__title">{}</span>{copy}</div>"#,
                    escape(title)
                )
            })
            .unwrap_or_default();
        let body = if self.preformatted {
            format!(
                r#"<pre class="ada-panel__body">{}</pre>"#,
                escape(&self.content)
            )
        } else {
            format!(r#"<div class="ada-panel__body ada-panel__body--html">{}</div>"#, self.content)
        };
        format!(r#"<div class="ada-panel">{head}{body}</div>"#)
    }
}

// ── 消息行 ───────────────────────────────────────────────────────────────

/// 用户消息气泡。
#[derive(Debug, Clone)]
pub struct UserMessage {
    pub text: String,
}

impl Render for UserMessage {
    fn render(&self) -> String {
        format!(
            r#"<div class="ada-msg ada-msg--user"><div class="ada-bubble">{}</div></div>"#,
            escape(&self.text)
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
            avatar = icon("i-bot", 15),
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
            .map(|button| {
                format!("<div class=\"ada-empty__action\">{}</div>", button.render())
            })
            .unwrap_or_default();
        format!(
            r#"<div class="ada-empty"><span class="ada-empty__mark">{icon}</span><h2 class="ada-empty__title">{title}</h2>{description}{action}</div>"#,
            icon = icon(&self.icon, 20),
            title = escape(&self.title),
            description = description,
            action = action,
        )
    }
}

// ── 输入框 ───────────────────────────────────────────────────────────────

/// 输入框下方辅助区的一条信息。
#[derive(Debug, Clone)]
pub struct AuxItem {
    pub icon: Option<String>,
    pub text: String,
    pub tone: Tone,
}

impl AuxItem {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            icon: None,
            text: text.into(),
            tone,
        }
    }

    pub fn icon(mut self, key: impl Into<String>) -> Self {
        self.icon = Some(key.into());
        self
    }
}

/// 输入框下方的辅助功能区：状态提示 + 上下文信息 + 快捷操作。
#[derive(Debug, Clone, Default)]
pub struct ComposerAux {
    /// 左组：状态提示（连接、运行中、排队）。
    pub status: Vec<AuxItem>,
    /// 右组：上下文信息（模型、会话、工作区）。
    pub context: Vec<AuxItem>,
    /// 快捷操作。
    pub actions: Vec<Button>,
    /// 常驻提示（快捷键等）。
    pub hint: Option<String>,
    /// 插件插槽（`composer.footer`）。
    pub plugins: String,
}

impl Render for ComposerAux {
    fn render(&self) -> String {
        let render_items = |items: &Vec<AuxItem>| -> String {
            items
                .iter()
                .map(|item| {
                    let glyph = item
                        .icon
                        .as_deref()
                        .map(|key| icon(key, 12))
                        .unwrap_or_default();
                    format!(
                        r#"<span class="ada-aux__item ada-aux__item--{}">{glyph}<span>{}</span></span>"#,
                        item.tone.class(),
                        escape(&item.text)
                    )
                })
                .collect::<Vec<_>>()
                .join("")
        };
        // 状态、上下文、快捷操作、提示、插件插槽——**一整行连续排列**（居中），
        // 不做左右分栏：分栏会把"Enter 发送"这类提示推走，读起来像两段话。
        let status = render_items(&self.status);
        let context = render_items(&self.context);
        let actions = self
            .actions
            .iter()
            .map(|button| button.render())
            .collect::<Vec<_>>()
            .join("");
        let hint = self
            .hint
            .as_deref()
            .map(|text| format!("<span class=\"ada-aux__hint\">{}</span>", escape(text)))
            .unwrap_or_default();
        let plugins = if self.plugins.is_empty() {
            String::new()
        } else {
            format!("<span class=\"ada-aux__plugins\">{}</span>", self.plugins)
        };
        format!(
            r#"<div class="ada-aux"><span class="ada-aux__group">{status}{context}</span><div class="ada-aux__actions">{actions}</div>{hint}{plugins}</div>"#
        )
    }
}

/// 输入框（Composer）：输入卡 + 工具行 + 辅助区。
///
/// 工具行左右两侧是**插件插槽**：插件可以往里塞自己的控件，
/// 插槽内容由插件注册表在 SSR 期填充。
#[derive(Debug, Clone)]
pub struct Composer {
    /// 表单提交地址。
    pub action: String,
    pub placeholder: String,
    /// 聚焦后出现的提示文案。
    pub focus_hint: Option<String>,
    /// 工具行左侧（内置控件 + 插件插槽）。
    pub left: String,
    /// 工具行右侧（内置控件 + 插件插槽）。
    pub right: String,
    /// 输入框下方的辅助区。
    pub aux: Option<ComposerAux>,
}

impl Render for Composer {
    fn render(&self) -> String {
        let hint = self
            .focus_hint
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| {
                format!(
                    r#"<div class="ada-composer__hint">{}</div>"#,
                    escape(text)
                )
            })
            .unwrap_or_default();
        let aux = self
            .aux
            .as_ref()
            .map(|aux| aux.render())
            .unwrap_or_default();
        format!(
            r#"<form class="ada-composer" method="post" action="{action}"><div class="ada-composer__card"><textarea class="ada-composer__input" name="text" rows="2" placeholder="{placeholder}" aria-label="消息输入框" required></textarea>{hint}<div class="ada-composer__bar"><div class="ada-composer__group ada-composer__group--left">{left}</div><div class="ada-composer__group ada-composer__group--right">{right}</div></div></div></form>{aux}"#,
            action = escape(&self.action),
            placeholder = escape(&self.placeholder),
            hint = hint,
            left = self.left,
            right = self.right,
            aux = aux,
        )
    }
}

// ── 插槽 ─────────────────────────────────────────────────────────────────

/// 插件插槽容器。
///
/// 插件失败时插槽回退到默认实现（`fallback`），主应用照常工作。
#[derive(Debug, Clone, Default)]
pub struct Slot {
    pub name: String,
    /// 插件贡献的 HTML（已信任的本地内容）。
    pub html: String,
    /// 失败回退内容。
    pub fallback: String,
    /// 参与贡献的插件数（用于调试与埋点）。
    pub contributors: usize,
}

impl Render for Slot {
    fn render(&self) -> String {
        if self.html.is_empty() {
            return self.fallback.clone();
        }
        format!(
            r#"<div class="ada-slot" data-slot="{name}" data-contributors="{count}">{html}</div>"#,
            name = escape(&self.name),
            count = self.contributors,
            html = self.html,
        )
    }
}

/// 渲染一个插槽（带默认回退）。
pub fn render_slot(name: &str, html: &str, contributors: usize, fallback: &str) -> String {
    Slot {
        name: name.to_string(),
        html: html.to_string(),
        fallback: fallback.to_string(),
        contributors,
    }
    .render()
}

// ── 浮层（审批 / 排队） ───────────────────────────────────────────────────

/// 审批托盘：显示工具名、等待时长与待批准原文，批准/拒绝只在这里。
#[derive(Debug, Clone)]
pub struct ApprovalTray {
    pub call_id: String,
    pub thread_id: String,
    pub tool_name: String,
    pub args: String,
    pub waited: String,
}

impl Render for ApprovalTray {
    fn render(&self) -> String {
        let args = if self.args.trim().is_empty() {
            "<span class=\"ada-tray__empty\">（这个工具没有提供参数预览）</span>".to_string()
        } else {
            format!(
                "<pre class=\"ada-tray__args\">{}</pre>",
                super::theme::escape(&self.args)
            )
        };
        format!(
            r#"<div class="ada-tray ada-tray--approval"><div class="ada-tray__head"><span class="ada-tray__icon">{icon}</span><span class="ada-tray__title">需要批准 · {tool}</span><span class="ada-tray__meta">{clock}{waited}</span></div>{args}<div class="ada-tray__actions"><form method="post" action="/approvals/{call}/decision" class="ada-tray__form"><input type="hidden" name="thread_id" value="{thread}"><button class="ada-btn ada-btn--primary ada-btn--sm" type="submit" name="approved" value="true">{check}批准</button><button class="ada-btn ada-btn--secondary ada-btn--sm" type="submit" name="approved" value="false">{cross}拒绝</button></form></div></div>"#,
            icon = icon("i-shield-check", 14),
            tool = super::theme::escape(&self.tool_name),
            clock = icon("i-clock", 11),
            waited = super::theme::escape(&self.waited),
            args = args,
            call = super::theme::escape(&self.call_id),
            thread = super::theme::escape(&self.thread_id),
            check = icon("i-check", 12),
            cross = icon("i-x", 12),
        )
    }
}

/// 排队托盘里的一条消息。
#[derive(Debug, Clone)]
pub struct QueueItem {
    pub index: usize,
    pub order: usize,
    pub text: String,
    pub thread_id: String,
}

/// 排队托盘：可见、可提升、可删除、可清空。
#[derive(Debug, Clone)]
pub struct QueueTray {
    pub items: Vec<QueueItem>,
}

impl Render for QueueTray {
    fn render(&self) -> String {
        let rows = self
            .items
            .iter()
            .map(|item| {
                format!(
                    r#"<div class="ada-tray__row"><span class="ada-tray__order">{order}</span><span class="ada-tray__text" title="{text}">{text}</span><form method="post" action="/threads/{thread}/queue/{index}/promote" class="ada-tray__form"><button class="ada-tray__link" type="submit" title="提升到队首">提升</button></form><form method="post" action="/threads/{thread}/queue/{index}/remove" class="ada-tray__form"><button class="ada-tray__link ada-tray__link--danger" type="submit" title="删除">删除</button></form></div>"#,
                    order = item.order,
                    text = super::theme::escape(&item.text),
                    thread = super::theme::escape(&item.thread_id),
                    index = item.index,
                )
            })
            .collect::<Vec<_>>()
            .join("");
        let clear = self
            .items
            .first()
            .map(|item| {
                format!(
                    r#"<form method="post" action="/threads/{thread}/queue/clear" class="ada-tray__form"><button class="ada-tray__link" type="submit">清空</button></form>"#,
                    thread = super::theme::escape(&item.thread_id)
                )
            })
            .unwrap_or_default();
        format!(
            r#"<div class="ada-tray ada-tray--queue"><div class="ada-tray__head"><span class="ada-tray__icon">{icon}</span><span class="ada-tray__title">排队中 · {count} 条</span>{clear}</div>{rows}</div>"#,
            icon = icon("i-layers", 14),
            count = self.items.len(),
            clear = clear,
            rows = rows,
        )
    }
}
