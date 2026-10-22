//! 折叠行与展开面板（工具 / 思考 / 压缩）。

use crate::ui::icons::icon;
use crate::ui::theme::{
    attr, class_list, escape, space_attrs, ButtonKind, ButtonVariant, Size, Tone,
};

use super::Render;

// ── 折叠（工具 / 思考 / 压缩） ────────────────────────────────────────────

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
    /// 结果尾标：折叠态回答"这一步得到了什么"（`167 行` / `exit 0`）。
    pub result: Option<String>,
    pub result_tone: Tone,
    /// 语义阶段（`running` / `warn` / `danger`）：驱动左侧色条与实时计时。
    pub phase: Option<&'static str>,
    /// 实时计时基点（epoch 毫秒）：客户端据此每秒刷新耗时文案。
    pub started_at: Option<i64>,
    /// 展开区内容（已渲染好的 HTML）。
    pub body: String,
    /// 行类型，用于样式与测试定位。
    pub kind: String,
    /// 行内额外属性。
    pub extra_attrs: Option<String>,
    /// 目标信息是否用弱化样式（思考预览用斜体弱色）。
    pub detail_muted: bool,
    /// 稳定标识（item id）：轮询替换 DOM 后靠它恢复展开状态。
    pub key: Option<String>,
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
            result: None,
            result_tone: Tone::Neutral,
            phase: None,
            started_at: None,
            body: String::new(),
            kind: kind.into(),
            extra_attrs: None,
            detail_muted: false,
            key: None,
        }
    }

    /// 稳定标识：live 轮询替换 DOM 后按它恢复展开状态。
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
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

    /// 结果尾标：紧跟在目标后面，回答"得到了什么"。
    pub fn result(mut self, result: impl Into<String>, tone: Tone) -> Self {
        self.result = Some(result.into());
        self.result_tone = tone;
        self
    }

    /// 语义阶段：运行中给品牌色条 + 呼吸，等待批准给警告色，失败给危险色。
    pub fn phase(mut self, phase: &'static str) -> Self {
        self.phase = Some(phase);
        self
    }

    /// 实时计时基点（epoch 毫秒）：客户端据此把「已 3s」接着走下去。
    pub fn started_at(mut self, ms: i64) -> Self {
        self.started_at = Some(ms);
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
            .map(|text| {
                format!(
                    "<span class=\"ada-disclosure__sub\">{}</span>",
                    escape(text)
                )
            })
            .unwrap_or_default();
        let meta = self
            .meta
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| {
                format!(
                    "<span class=\"ada-disclosure__meta\">{}</span>",
                    escape(text)
                )
            })
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
        let result = self
            .result
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| {
                format!(
                    "<span class=\"ada-disclosure__result\" data-tone=\"{}\">{}</span>",
                    self.result_tone.class(),
                    escape(text)
                )
            })
            .unwrap_or_default();
        let attrs = space_attrs(self.extra_attrs.as_deref());
        let data_key = attr("data-key", self.key.as_deref().unwrap_or(""));
        let data_phase = attr("data-phase", self.phase.unwrap_or(""));
        let data_started = self
            .started_at
            .map(|ms| format!(" data-started-at=\"{ms}\""))
            .unwrap_or_default();
        // 图标位有两个角色：常态是"这一行是什么"（工具/思考图标），
        // 鼠标悬浮或键盘聚焦时换成"可以展开"（箭头）。两者占同宽，切换不跳版。
        format!(
            r#"<details class="ada-disclosure" data-kind="{kind}" data-state="closed"{data_key}{data_phase}{data_started}{attrs}><summary class="ada-disclosure__summary"><span class="ada-disclosure__icon">{icon}</span><span class="ada-disclosure__caret">{caret}</span><span class="ada-disclosure__label">{label}</span>{detail}{sub}{result}{meta}{badge}</summary><div class="ada-disclosure__rail">{body}</div></details>"#,
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
            format!(
                r#"<div class="ada-panel__body ada-panel__body--html">{}</div>"#,
                self.content
            )
        };
        format!(r#"<div class="ada-panel">{head}{body}</div>"#)
    }
}
