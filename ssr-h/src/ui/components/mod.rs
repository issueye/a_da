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
//!
//! 按组件类型拆成子模块；新增组件放对应文件，别再堆回本文件。

/// 可渲染组件的统一接口。
pub trait Render {
    fn render(&self) -> String;
}

mod button;
mod chip;
mod composer;
mod disclosure;
mod input;
mod messages;
mod modal;
mod model_picker;
mod model_table;
mod select;
mod slot;
mod tabs;
mod tool_call;
mod trays;

pub use button::{Button, IconButton, SendButton};
pub use chip::Chip;
pub use composer::{AuxItem, Composer, ComposerAux};
pub use disclosure::{Disclosure, Panel};
pub use input::TextField;
pub use messages::{AssistantMessage, EmptyState, Notice, UserMessage};
pub use modal::{ConfirmDialog, Modal};
pub use model_picker::{
    effort_label, model_chip_inner, normalize_effort, ModelChoice, ModelGroup, ModelPicker,
};
pub use model_table::{ModelTable, ModelTableRow};
pub use select::Select;
pub use slot::{render_slot, Slot};
pub use tabs::{TabItem, Tabs};
pub use tool_call::ToolCall;
pub use trays::{waited_from_at, ApprovalTray, QueueItem, QuestionTray, QueueTray};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_button_renders_a_real_submit_form() {
        let html = Button::new("重试这一轮")
            .action("/threads/t1/retry")
            .attrs("data-confirm=\"确定重试吗？\"")
            .render();
        assert!(html.starts_with("<form"), "必须是真正的表单: {html}");
        assert!(html.contains("method=\"post\""));
        assert!(html.contains("action=\"/threads/t1/retry\""));
        assert!(html.contains("data-confirm=\"确定重试吗？\""));
        assert!(html.contains("type=\"submit\""));
        assert!(html.contains("</form>"));
    }

    #[test]
    fn send_button_is_enabled_in_the_ssr_baseline() {
        let html = SendButton::new("发送").render();
        assert!(!html.contains(" disabled"), "无 JS 时也必须能提交: {html}");
        assert!(html.contains("type=\"submit\""));
    }

    #[test]
    fn disclosure_carries_a_stable_key_for_live_restore() {
        let html = Disclosure::new("tool", "i-terminal", "执行命令")
            .key("item-42")
            .body("")
            .render();
        assert!(html.contains("data-key=\"item-42\""));
    }

    #[test]
    fn select_is_a_wrapped_dropdown_not_a_native_select() {
        let html = Select::new("protocol")
            .option("openai_chat", "OpenAI Chat")
            .option("anthropic", "Anthropic")
            .value("anthropic")
            .render();
        assert!(!html.contains("<select"), "不能退回原生 select: {html}");
        // 自定义元素：浏览器解析后自动升级成 <ada-select> 的行为。
        assert!(html.starts_with("<ada-select"), "必须是 ada-select: {html}");
        assert!(html.ends_with("</ada-select>"));
        // 表单值仍要能提交：隐藏 input 承载 name=value。
        assert!(html.contains(r#"<input type="hidden" name="protocol" value="anthropic""#));
        assert!(html.contains("data-select-trigger"));
        assert!(html.contains("data-select-menu"));
        assert_eq!(html.matches("is-selected").count(), 1);
        // 未知值回落到第一项，避免出现空标签（后端新增协议时不会渲染成空白）。
        let fallback = Select::new("x")
            .option("a", "A")
            .value("not-in-options")
            .render();
        assert!(fallback.contains(r#"value="a""#));
        assert!(fallback.contains(r#">A</span>"#));
    }

    #[test]
    fn modal_carries_data_attributes_and_escaped_values() {
        let html = Modal::new("model-3", "编辑模型")
            .wide()
            .body("<p>x</p>")
            .render();
        assert!(!html.contains("<dialog"));
        // 自定义元素：`<ada-modal>` 由 elements.js 注册，服务端只渲染结构与钩子。
        assert!(html.starts_with("<ada-modal"), "必须是 ada-modal: {html}");
        assert!(html.ends_with("</ada-modal>"));
        assert!(html.contains(r#"data-modal="model-3""#));
        assert!(html.contains("ada-modal__panel--wide"));
        assert!(html.contains(r#"aria-modal="true""#));
        assert!(html.contains("ada-modal-title-model-3"));
        // 标题走转义，避免值里带引号把属性截断。
        let escaped = Modal::new("m", "a\"b").render();
        assert!(escaped.contains("a&quot;b"));
    }

    #[test]
    fn confirm_dialog_is_a_plain_dom_component() {
        let html = ConfirmDialog::markup();
        // 不用原生 <dialog>（更不能是 window.confirm）：开合与焦点由 elements.js 管。
        assert!(!html.contains("<dialog"));
        assert!(html.contains("<ada-modal"));
        assert!(html.contains("data-confirm-dialog"));
        assert!(html.contains("data-confirm-message"));
        assert!(html.contains("data-confirm-ok"));
        assert!(html.contains("data-confirm-cancel"));
        assert!(html.contains("data-modal-close"));
        assert!(html.contains(r#"role="dialog""#));
        assert!(html.contains(r#"aria-modal="true""#));
    }

    #[test]
    fn tabs_render_a_roving_tablist_with_escaped_values() {
        let html = Tabs::new("provider-detail", "供应商详情")
            .active("models")
            .item(TabItem::new(
                "connection",
                "供应商信息",
                "/providers?provider=p1&tab=connection",
            ))
            .item(TabItem::new("models", "模型列表", "/providers?provider=p1&tab=models").count(5))
            .render();
        assert!(html.starts_with(r#"<div class="ada-tabs__list""#), "{html}");
        assert!(html.contains(r#"role="tablist""#));
        assert!(html.contains(r#"aria-label="供应商详情""#));
        assert!(html.contains(r#"data-ada-tabs="provider-detail""#));
        // 只有一个页签被选中，且只有它留在 Tab 键序里。
        assert_eq!(html.matches(r#"aria-selected="true""#).count(), 1);
        assert_eq!(html.matches(r#"tabindex="0""#).count(), 1);
        assert!(html.contains(r#"id="provider-detail-tab-models""#));
        assert!(html.contains(r#"aria-controls="provider-detail-panel-connection""#));
        // href 里的 & 必须转义；标签文案与计数都要出现。
        assert!(html.contains(r#"href="/providers?provider=p1&amp;tab=models""#));
        assert!(html.contains(r#"<span class="ada-tabs__label">模型列表</span>"#));
        assert!(html.contains(r#"<span class="ada-tabs__count">5</span>"#));

        // active 指向不存在的 key → 回落第一个，不渲染一条"没选中"的页签条。
        let fallback = Tabs::new("t", "页签")
            .active("missing")
            .item(TabItem::new("a", "A", "/a"))
            .item(TabItem::new("b", "B", "/b"))
            .render();
        assert!(fallback.contains(r#"id="t-tab-a""#));
        assert!(fallback.contains(r#"aria-controls="t-panel-a""#));
        assert!(fallback.contains(r#"aria-selected="true""#));

        // 没有页签就不渲染空壳。
        assert!(Tabs::new("t", "页签").render().is_empty());
    }

    #[test]
    fn user_message_shows_an_image_count() {
        let html = UserMessage {
            text: "看图".to_string(),
            images: 2,
        }
        .render();
        assert!(html.contains("含 2 张图片"));
    }
}
