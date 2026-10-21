//! 组件库的设计规范：语义色、尺寸、类名与焦点态。
//!
//! 组件库不自带颜色，只引用 `--ph-*` 变量（见 `static/components.css`）。
//! 这样深浅主题是同一套信息层级的两种呈现，而不是两套设计。
//!
//! 类名规范：组件库统一 `ada-` 前缀，遵循 `ada-<block>__<element>--<modifier>`；
//! 状态一律用 `data-*` 属性表达（`data-state="open"`，而不是拼 class），
//! 这样插件与业务代码都靠属性选择器协作，不必依赖类名细节。

/// 语义色调。决定前景色 + 浅底 + 弱描边三者怎么配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    /// 中性：次要信息、只读展示。
    #[default]
    Neutral,
    /// 品牌：主操作、当前选中、链接。
    Brand,
    /// 提醒：等待批准、需要注意但不危险。
    Warn,
    /// 危险：失败、停止、完全访问这类高权限状态。
    Danger,
    /// 成功：完成、已连接。
    Success,
}

impl Tone {
    /// 主题里的语义 class 后缀。
    pub fn class(&self) -> &'static str {
        match self {
            Tone::Neutral => "neutral",
            Tone::Brand => "brand",
            Tone::Warn => "warn",
            Tone::Danger => "danger",
            Tone::Success => "success",
        }
    }
}

/// 按钮层级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    /// 主操作：实心品牌色。
    Primary,
    /// 次要操作：描边。
    Secondary,
    /// 轻操作：透明底，hover 才有底。
    Ghost,
    /// 危险操作：软底红字。
    Danger,
}

impl ButtonVariant {
    pub fn class(&self) -> &'static str {
        match self {
            ButtonVariant::Primary => "primary",
            ButtonVariant::Secondary => "secondary",
            ButtonVariant::Ghost => "ghost",
            ButtonVariant::Danger => "danger",
        }
    }
}

/// 尺寸档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Sm,
    Md,
    Lg,
}

impl Size {
    pub fn class(&self) -> &'static str {
        match self {
            Size::Sm => "sm",
            Size::Md => "md",
            Size::Lg => "lg",
        }
    }
}

/// 按钮的 `type`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// 普通按钮（不提交表单）。
    Button,
    /// 提交按钮。
    Submit,
}

impl ButtonKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ButtonKind::Button => "button",
            ButtonKind::Submit => "submit",
        }
    }
}

impl std::fmt::Display for ButtonKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 焦点态规范（与 `static/components.css` 中的 `--ada-focus-*` 对应）。
///
/// 统一焦点态是组件库存在的意义之一：以前每个控件各写各的，
/// 现在只有这一套——2px 品牌描边 + 3px 半透明光环，深浅主题共用。
pub mod focus {
    /// 组件根 class：所有可聚焦组件都带上，用于统一外环。
    pub const CLASS: &str = "ada-focusable";
    /// 焦点环的 CSS 变量名（供 CSS 与文档引用）。
    pub const RING_VAR: &str = "--ada-focus-ring";
}

/// HTML 转义：组件库自己拼 HTML，所有动态文本必须过这里。
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// 拼接 class，跳过空片段。
pub fn class_list(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 生成 HTML 属性（`key="value"`），值为空时返回空串。
pub fn attr(name: &str, value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    format!(" {name}=\"{}\"", escape(value))
}

/// 把调用方给的裸属性串补上前导空格（空串仍是空串）。
///
/// 组件拼标签时总是写成 `...{extra}>`，如果 `extra` 自带内容却不带空格，
/// 就会拼出 `action="/x"data-confirm="…"` 这种非法 HTML（浏览器可能勉强解析，
/// 但属性归属就不可靠了）。
pub fn space_attrs(attrs: Option<&str>) -> String {
    match attrs.map(str::trim).filter(|attrs| !attrs.is_empty()) {
        Some(attrs) => format!(" {attrs}"),
        None => String::new(),
    }
}
