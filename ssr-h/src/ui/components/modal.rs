//! 弹窗类：通用 `Modal` 与共享 `ConfirmDialog`（`<ada-modal>` 元素）。

use super::button::Button;

use crate::ui::icons::icon;
use crate::ui::theme::{
    attr, class_list, escape, space_attrs, ButtonKind, ButtonVariant, Size, Tone,
};

use super::Render;

// ── 弹窗 ─────────────────────────────────────────────────────────────────

/// 通用弹窗：标题 + 内容（通常是整张表单）+ 底部动作。
///
/// 开合/焦点圈/Esc/遮罩/滚动锁定由 `components.js` 的 `[data-modal]` 控制器统一管；
/// 触发用 `[data-modal-open="<id>"]`，关闭用 `[data-modal-close]`。
#[derive(Debug, Clone, Default)]
pub struct Modal {
    pub id: String,
    pub title: String,
    /// 可信 HTML（调用方负责转义动态值）。
    pub body_html: String,
    /// 可选：底部动作区（不放进 body 时用）。
    pub actions_html: String,
    pub wide: bool,
}

impl Modal {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn body(mut self, html: impl Into<String>) -> Self {
        self.body_html = html.into();
        self
    }

    pub fn actions(mut self, html: impl Into<String>) -> Self {
        self.actions_html = html.into();
        self
    }

    pub fn wide(mut self) -> Self {
        self.wide = true;
        self
    }
}

impl Render for Modal {
    fn render(&self) -> String {
        let title_id = format!("ada-modal-title-{}", self.id);
        let actions = if self.actions_html.trim().is_empty() {
            String::new()
        } else {
            format!(
                "<div class=\"ada-modal__actions\">{}</div>",
                self.actions_html
            )
        };
        format!(
            r#"<ada-modal class="ada-modal" data-modal="{id}" hidden>
  <div class="ada-modal__backdrop" data-modal-close></div>
  <div class="ada-modal__panel{wide}" role="dialog" aria-modal="true" aria-labelledby="{title_id}">
    <h2 class="ada-modal__title" id="{title_id}">{title}</h2>
    <div class="ada-modal__body">{body}</div>
    {actions}
  </div>
</ada-modal>"#,
            id = escape(&self.id),
            title_id = escape(&title_id),
            title = escape(&self.title),
            wide = if self.wide {
                " ada-modal__panel--wide"
            } else {
                ""
            },
            body = self.body_html,
        )
    }
}

/// 共享确认弹窗：`form[data-confirm]` / `a[data-confirm]` 的唯一出口。
///
/// 纯 DOM 组件——**不用 `<dialog>`、更不用 `window.confirm`**：原生对话框不可样式化、
/// 会阻塞整页脚本，而且各浏览器长得都不一样（历史 bug：二级页没有这个组件时退回原生
/// confirm，同一个"删除"在两处是两种观感）。开合、焦点圈、Esc 与遮罩点击统一由
/// `components.js` 的 `[data-modal]` 控制器处理（`data-modal="confirm"`）。
pub struct ConfirmDialog;

impl ConfirmDialog {
    pub fn markup() -> String {
        format!(
            r#"<ada-modal class="ada-modal" data-modal="confirm" data-confirm-dialog hidden>
  <div class="ada-modal__backdrop" data-modal-close data-confirm-cancel></div>
  <div class="ada-modal__panel" role="dialog" aria-modal="true" aria-labelledby="ada-confirm-title">
    <h2 class="ada-modal__title" id="ada-confirm-title">确认操作</h2>
    <p class="ada-modal__message" data-confirm-message>确认执行这个操作？</p>
    <div class="ada-modal__actions">{cancel}{ok}</div>
  </div>
</ada-modal>"#,
            cancel = Button::new("取消")
                .variant(ButtonVariant::Secondary)
                .size(Size::Sm)
                .kind(ButtonKind::Button)
                .attrs("data-modal-close data-confirm-cancel")
                .render(),
            ok = Button::new("确认")
                .variant(ButtonVariant::Primary)
                .size(Size::Sm)
                .kind(ButtonKind::Button)
                .attrs("data-confirm-ok data-modal-autofocus")
                .render(),
        )
    }
}

impl Render for ConfirmDialog {
    fn render(&self) -> String {
        Self::markup()
    }
}
