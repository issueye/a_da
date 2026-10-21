//! 浮层：审批托盘与排队托盘。

use crate::ui::icons::icon;
use crate::ui::theme::{
    attr, class_list, escape, space_attrs, ButtonKind, ButtonVariant, Size, Tone,
};

use super::Render;

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
            format!("<pre class=\"ada-tray__args\">{}</pre>", escape(&self.args))
        };
        format!(
            r#"<div class="ada-tray ada-tray--approval"><div class="ada-tray__head"><span class="ada-tray__icon">{icon}</span><span class="ada-tray__title">需要批准 · {tool}</span><span class="ada-tray__meta">{clock}{waited}</span></div>{args}<div class="ada-tray__actions"><form method="post" action="/approvals/{call}/decision" class="ada-tray__form"><input type="hidden" name="thread_id" value="{thread}"><button class="ada-btn ada-btn--primary ada-btn--sm" type="submit" name="approved" value="true">{check}批准</button><button class="ada-btn ada-btn--secondary ada-btn--sm" type="submit" name="approved" value="false">{cross}拒绝</button></form></div></div>"#,
            icon = icon("i-shield-check", 14),
            tool = escape(&self.tool_name),
            clock = icon("i-clock", 11),
            waited = escape(&self.waited),
            args = args,
            call = escape(&self.call_id),
            thread = escape(&self.thread_id),
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
                    r#"<div class="ada-tray__row"><span class="ada-tray__order">{order}</span><span class="ada-tray__text" title="{text}">{text}</span><form method="post" action="/threads/{thread}/queue/{index}/promote" class="ada-tray__form" data-confirm="提升会中断当前回答并立即执行这条消息，确定吗？"><button class="ada-tray__link" type="submit" title="提升到队首（会中断当前轮次）">提升</button></form><form method="post" action="/threads/{thread}/queue/{index}/remove" class="ada-tray__form" data-confirm="确定删除这条排队消息吗？"><button class="ada-tray__link ada-tray__link--danger" type="submit" title="删除">删除</button></form></div>"#,
                    order = item.order,
                    text = escape(&item.text),
                    thread = escape(&item.thread_id),
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
                    r#"<form method="post" action="/threads/{thread}/queue/clear" class="ada-tray__form" data-confirm="确定清空当前会话的排队消息吗？"><button class="ada-tray__link" type="submit">清空</button></form>"#,
                    thread = escape(&item.thread_id)
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
