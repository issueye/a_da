//! 下拉选择（`<ada-select>`）：非原生 `<select>`，弹层自己画。

use crate::ui::icons::icon;
use crate::ui::theme::{
    attr, class_list, escape, space_attrs, ButtonKind, ButtonVariant, Size, Tone,
};

use super::Render;

// ── 下拉选择 ─────────────────────────────────────────────────────────────

/// 下拉选择：原生 `<select>` 的弹层由浏览器/系统渲染——样式不受控、各家键盘行为不同、
/// 深色主题下更是直接一处白色方块。这里用"触发按钮 + listbox + 隐藏 input"自己实现：
/// 表单照常提交 `name=value`，交互由 `components.js` 负责（点击、方向键、Home/End、
/// Enter/Space、Esc、点外部关闭）。
#[derive(Debug, Clone, Default)]
pub struct Select {
    pub name: String,
    pub value: String,
    /// (value, label)
    pub options: Vec<(String, String)>,
    pub title: Option<String>,
    pub extra_class: Option<String>,
}

impl Select {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn option(mut self, value: impl Into<String>, label: impl Into<String>) -> Self {
        self.options.push((value.into(), label.into()));
        self
    }

    pub fn options(mut self, options: impl IntoIterator<Item = (String, String)>) -> Self {
        self.options.extend(options);
        self
    }

    /// 当前值。不在选项里（例如后端新增了协议）时回落到第一项，避免出现空显示。
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
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

impl Render for Select {
    fn render(&self) -> String {
        let current = if self.options.iter().any(|(value, _)| value == &self.value) {
            self.value.clone()
        } else {
            self.options
                .first()
                .map(|(value, _)| value.clone())
                .unwrap_or_default()
        };
        let current_label = self
            .options
            .iter()
            .find(|(value, _)| value == &current)
            .map(|(_, label)| label.clone())
            .unwrap_or_default();
        let mut items = String::new();
        for (value, label) in &self.options {
            let selected = value == &current;
            items.push_str(&format!(
                r#"<li class="ada-select__option{selected_class}" role="option" data-value="{value}" aria-selected="{aria}" tabindex="-1">{label}</li>"#,
                selected_class = if selected { " is-selected" } else { "" },
                aria = if selected { "true" } else { "false" },
                value = escape(value),
                label = escape(label),
            ));
        }
        let classes = class_list(&["ada-select", self.extra_class.as_deref().unwrap_or("")]);
        format!(
            r#"<ada-select class="{classes}" data-select>
  <input type="hidden" name="{name}" value="{current}" data-select-input>
  <button type="button" class="ada-select__trigger" aria-haspopup="listbox" aria-expanded="false"{title} data-select-trigger>
    <span class="ada-select__label" data-select-label>{current_label}</span>
    <svg class="ada-select__caret" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg>
  </button>
  <ul class="ada-select__menu" role="listbox" hidden data-select-menu>{items}</ul>
</ada-select>"#,
            name = escape(&self.name),
            current = escape(&current),
            current_label = escape(&current_label),
            title = attr("title", self.title.as_deref().unwrap_or("")),
        )
    }
}
