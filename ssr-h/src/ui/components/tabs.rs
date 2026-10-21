//! 页签（Tabs）：把同一实体的多块内容收进一个视图，按需切换。
//!
//! 分工是刻意的：
//!
//! - **本组件只渲染 tablist**（导航条）：`role="tablist"` / `role="tab"`、
//!   `aria-selected`、`aria-controls`、roving `tabindex` 这些契约都在这里，页面不再手写；
//! - **面板由页面模板摆放**：面板里是页面自己的结构（表单、表格），组件不该知道它长什么样；
//!   模板必须遵守 id 约定 `{id}-panel-{key}`，与组件的 `aria-controls` 对齐（见 [`Tabs`]）。
//!
//! 没有 JS 时每个页签就是一条真实链接（`href` 由服务端按当前页面的 `?tab=` 生成），
//! 点击会整页刷新到该页签；`components.js` 只做渐进增强——就地切换面板并改写地址栏。

use crate::ui::theme::escape;

use super::Render;

// ── 页签 ─────────────────────────────────────────────────────────────────

/// 一个页签。
#[derive(Debug, Clone, Default)]
pub struct TabItem {
    /// 稳定 key（进 DOM id 与 `data-ada-tab`，只用 ASCII 短名，别放展示文案）。
    pub key: String,
    pub label: String,
    /// 无 JS 时的直达地址（含页面自身参数与 `tab=key`）。
    pub href: String,
    /// 标签右侧的计数（模型数这类"不进面板也该知道"的信息）；`None` 不渲染。
    pub count: Option<usize>,
}

impl TabItem {
    pub fn new(
        key: impl Into<String>,
        label: impl Into<String>,
        href: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            href: href.into(),
            count: None,
        }
    }

    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }
}

/// 页签导航条。
///
/// DOM 约定（`Tabs::new(id, …)` 的 `id` 是前缀）：
///
/// | 元素 | id |
/// |---|---|
/// | 当前页签 | `{id}-tab-{key}` |
/// | 对应面板 | `{id}-panel-{key}`（模板摆放，带 `role="tabpanel"` 与 `aria-labelledby="{id}-tab-{key}"`） |
///
/// 面板的显示/隐藏由服务端按 `active` 决定（无 JS 也正确），JS 只接管点击与方向键。
#[derive(Debug, Clone, Default)]
pub struct Tabs {
    /// DOM id 前缀，同时也是面板 id 的前缀。
    pub id: String,
    /// tablist 的无障碍名称（例如「供应商详情」）。
    pub label: String,
    pub items: Vec<TabItem>,
    /// 当前页签 key；不存在时回落到第一个（不渲染空白页签条）。
    pub active: String,
}

impl Tabs {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            items: Vec::new(),
            active: String::new(),
        }
    }

    pub fn item(mut self, item: TabItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn active(mut self, key: impl Into<String>) -> Self {
        self.active = key.into();
        self
    }
}

impl Render for Tabs {
    fn render(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }
        let active = if self.items.iter().any(|item| item.key == self.active) {
            self.active.as_str()
        } else {
            self.items[0].key.as_str()
        };
        let prefix = escape(&self.id);
        let mut out = format!(
            r#"<div class="ada-tabs__list" role="tablist" aria-label="{}" data-ada-tabs="{}">"#,
            escape(&self.label),
            prefix
        );
        for item in &self.items {
            let selected = item.key == active;
            let count = item
                .count
                .map(|count| format!(r#"<span class="ada-tabs__count">{count}</span>"#))
                .unwrap_or_default();
            out.push_str(&format!(
                r#"<a class="ada-tabs__tab" role="tab" id="{prefix}-tab-{key}" href="{href}" aria-controls="{prefix}-panel-{key}" aria-selected="{selected}" tabindex="{tabindex}" data-ada-tab="{key}"><span class="ada-tabs__label">{label}</span>{count}</a>"#,
                key = escape(&item.key),
                href = escape(&item.href),
                selected = if selected { "true" } else { "false" },
                tabindex = if selected { "0" } else { "-1" },
                label = escape(&item.label),
            ));
        }
        out.push_str("</div>");
        out
    }
}
