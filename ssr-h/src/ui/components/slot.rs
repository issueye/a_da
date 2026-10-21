//! 插件插槽容器。

use crate::ui::icons::icon;
use crate::ui::theme::{
    attr, class_list, escape, space_attrs, ButtonKind, ButtonVariant, Size, Tone,
};

use super::Render;

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
