//! 输入框与辅助区（含插件插槽装配）。

use super::button::Button;

use crate::ui::icons::icon;
use crate::ui::theme::{
    attr, class_list, escape, space_attrs, ButtonKind, ButtonVariant, Size, Tone,
};

use super::Render;

// ── 输入框 ───────────────────────────────────────────────────────────────

/// 输入框下方辅助区的一条信息。
#[derive(Debug, Clone)]
pub struct AuxItem {
    pub icon: Option<String>,
    pub text: String,
    /// 悬停说明：视觉上只放"图标 + 数字"时，完整含义（单位/口径）在这里。
    pub title: Option<String>,
    pub tone: Tone,
}

impl AuxItem {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            icon: None,
            text: text.into(),
            title: None,
            tone,
        }
    }

    pub fn icon(mut self, key: impl Into<String>) -> Self {
        self.icon = Some(key.into());
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
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
                    let title = item
                        .title
                        .as_deref()
                        .map(|text| format!(r#" title="{}""#, escape(text)))
                        .unwrap_or_default();
                    format!(
                        r#"<span class="ada-aux__item ada-aux__item--{}"{title}>{glyph}<span>{}</span></span>"#,
                        item.tone.class(),
                        escape(&item.text)
                    )
                })
                .collect::<Vec<_>>()
                .join("")
        };
        // 状态、上下文、快捷操作、插件插槽——**一整行连续排列**（居中），
        // 不做左右分栏：分栏会把状态与遥测推成两段话。
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
    /// 工具行左侧（内置控件 + 插件插槽）。
    pub left: String,
    /// 工具行右侧（内置控件 + 插件插槽）。
    pub right: String,
    /// 输入框下方的辅助区。
    pub aux: Option<ComposerAux>,
}

impl Render for Composer {
    fn render(&self) -> String {
        let aux = self
            .aux
            .as_ref()
            .map(|aux| {
                // live 轮询用 `#composer-aux` 做 OOB 目标：只替换状态区，
                // 不重建 textarea（保住焦点与草稿）。
                format!(r#"<div id="composer-aux">{}</div>"#, aux.render())
            })
            .unwrap_or_default();
        // 快捷键提示只出现在下方辅助区一处：卡片里再来一行，
        // 聚焦时会变成同一句话上下各一遍。
        //
        // 图片预览区在输入卡最上方：选中的图片发送前可见、可逐张移除；
        // data URI 由 chat.js 写成同名的隐藏 `images` 字段随表单提交。
        format!(
            r#"<form class="ada-composer" method="post" action="{action}"><div class="ada-composer__card"><div class="ada-composer__images" id="composer-images" hidden></div><textarea class="ada-composer__input" name="text" rows="2" placeholder="{placeholder}" aria-label="消息输入框" required></textarea><div class="ada-composer__bar"><div class="ada-composer__group ada-composer__group--left">{left}</div><div class="ada-composer__group ada-composer__group--right">{right}</div></div></div></form>{aux}"#,
            action = escape(&self.action),
            placeholder = escape(&self.placeholder),
            left = self.left,
            right = self.right,
            aux = aux,
        )
    }
}
