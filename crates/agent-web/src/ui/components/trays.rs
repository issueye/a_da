//! 浮层：排队托盘。

use crate::ui::icons::icon;
use crate::ui::theme::escape;

use super::Render;

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
                // `expect` 是渲染那一帧的文本指纹（与 `queue_rows` 同源，已截断 240 字符）：
                // 服务端在真正执行前拿它和当前队列同下标的消息比对，识别陈旧帧，
                // 避免"别的客户端删了一条之后，这里一点就误伤相邻消息"。
                format!(
                    r#"<div class="ada-tray__row"><span class="ada-tray__order">{order}</span><span class="ada-tray__text" title="{text}">{text}</span><form method="post" action="/threads/{thread}/queue/{index}/promote" class="ada-tray__form" data-partial data-confirm="提升会中断当前回答并立即执行这条消息，确定吗？"><input type="hidden" name="expect" value="{text}"><button class="ada-tray__link" type="submit" title="提升到队首（会中断当前轮次）">提升</button></form><form method="post" action="/threads/{thread}/queue/{index}/remove" class="ada-tray__form" data-partial data-confirm="确定删除这条排队消息吗？"><input type="hidden" name="expect" value="{text}"><button class="ada-tray__link ada-tray__link--danger" type="submit" title="删除">删除</button></form></div>"#,
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
                    r#"<form method="post" action="/threads/{thread}/queue/clear" class="ada-tray__form" data-partial data-confirm="确定清空当前会话的排队消息吗？"><button class="ada-tray__link" type="submit">清空</button></form>"#,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn queue_rows_carry_text_fingerprint_for_stale_frame_guard() {
        let html = QueueTray {
            items: vec![QueueItem {
                index: 3,
                order: 1,
                text: "继续修一下 <面板> \"引号\"".to_string(),
                thread_id: "t1".to_string(),
            }],
        }
        .render();
        // 提升与删除两张表单都必须带同一份指纹（服务端靠它识别陈旧帧）。
        assert_eq!(html.matches("name=\"expect\"").count(), 2, "{html}");
        assert!(
            html.contains("value=\"继续修一下 &lt;面板&gt; &quot;引号&quot;\""),
            "{html}"
        );
    }
}
