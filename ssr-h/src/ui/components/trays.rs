//! 浮层：审批托盘、提问托盘与排队托盘。

use crate::questions::PendingQuestion;
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
            r#"<div class="ada-tray ada-tray--approval"><div class="ada-tray__head"><span class="ada-tray__icon">{icon}</span><span class="ada-tray__title">需要批准 · {tool}</span><span class="ada-tray__meta">{clock}{waited}</span></div>{args}<div class="ada-tray__actions"><form method="post" action="/approvals/{call}/decision" class="ada-tray__form" data-partial><input type="hidden" name="thread_id" value="{thread}"><button class="ada-btn ada-btn--primary ada-btn--sm" type="submit" name="approved" value="true">{check}批准</button><button class="ada-btn ada-btn--secondary ada-btn--sm" type="submit" name="approved" value="false">{cross}拒绝</button></form></div></div>"#,
            icon = icon("i-shield-check", 14),
            tool = escape(&self.tool_name),
            clock = icon("i-clock", 12),
            waited = escape(&self.waited),
            args = args,
            call = escape(&self.call_id),
            thread = escape(&self.thread_id),
            check = icon("i-check", 12),
            cross = icon("i-x", 12),
        )
    }
}

/// 提问托盘：显示题目、选项（单选 radio / 多选 checkbox / 可选文本补充），
/// 作答只在这里——对话流里的 ask_user 卡只留记录（desktop-ui「操作面唯一」）。
///
/// 表单字段约定见 [`crate::questions::answer_params_from_form`]：单题
/// `choice`/`choices`/`text`，多题 `q_{id}__*` 前缀；动作是核心路由
/// `POST /questions/{call_id}/answer`。
#[derive(Debug, Clone)]
pub struct QuestionTray {
    pub question: PendingQuestion,
    pub thread_id: String,
}

/// 一道题的输入名：单题用裸字段，多题按题号加前缀（解析端按前缀分派）。
fn question_field(question: &PendingQuestion, item_id: &str, field: &str) -> String {
    if question.is_single() {
        field.to_string()
    } else {
        format!("q_{item_id}__{field}")
    }
}

impl Render for QuestionTray {
    fn render(&self) -> String {
        let q = &self.question;
        let waited = waited_from_ms(q.asked_at);
        // 多题 = 一次一题翻页（desktop-ui 同款交互）：每题一个 `data-page-item`，
        // 翻页条服务端渲染成 `hidden`，由 `chat.js` 的 `data-question-pager` 契约激活；
        // 无 JS 时翻页条保持隐藏、全部题目平铺（阅读基线不丢）。
        let multi = q.items.len() > 1;
        let sections = q
            .items
            .iter()
            .map(|item| self.render_item(item, multi))
            .collect::<Vec<_>>()
            .join("");
        let pager = if multi { pager_bar(q.items.len()) } else { String::new() };
        // 无选项题（纯文本/确认）：给「继续」兜底按钮——desktop-ui 同款语义，
        // 等于 text="用户确认继续"，让不输入的用户也能一键放行。
        let continue_button = if q.items.len() == 1 && q.items[0].options.is_empty() {
            let name = question_field(q, &q.items[0].id, "text");
            format!(
                r#"<button class="ada-btn ada-btn--secondary ada-btn--sm" type="submit" name="{name}" value="用户确认继续">{arrow}继续</button>"#,
                name = escape(&name),
                arrow = icon("i-arrow-right", 12),
            )
        } else {
            String::new()
        };
        format!(
            r#"<div class="ada-tray ada-tray--question"><form method="post" action="/questions/{call}/answer" class="ada-q" data-question-pager data-page-key="{key}"><input type="hidden" name="thread_id" value="{thread}"><div class="ada-tray__head"><span class="ada-tray__icon">{icon}</span><span class="ada-tray__title">待答复 · 智能体的提问</span><span class="ada-tray__meta">{clock}{waited}</span></div>{sections}{pager}<div class="ada-tray__actions">{continue_button}<button class="ada-btn ada-btn--primary ada-btn--sm" type="submit">{send}提交回答</button></div></form></div>"#,
            call = escape(&q.call_id),
            key = escape(&q.call_id),
            thread = escape(&self.thread_id),
            icon = icon("i-chat", 14),
            clock = icon("i-clock", 12),
            waited = escape(&waited),
            pager = pager,
            send = icon("i-send", 12),
        )
    }
}

/// 翻页条：服务端渲染为 `hidden`（无 JS 不可见、不可点），`chat.js` 按需显示。
/// 按钮必须是 `type="button"`——在 form 里，默认类型会把整份回答误提交。
fn pager_bar(count: usize) -> String {
    format!(
        r#"<div class="ada-q__pager" data-page-bar hidden><button type="button" class="ada-q__pager-btn" data-page-prev>上一题</button><span class="ada-q__pager-status" data-page-status>第 1 / {count} 题</span><button type="button" class="ada-q__pager-btn" data-page-next>下一题</button></div>"#,
    )
}

impl QuestionTray {
    /// `page_item`：多题时为 true，题外层多带一个 `data-page-item`（翻页契约的页）。
    fn render_item(&self, item: &crate::questions::QuestionItem, page_item: bool) -> String {
        let q = &self.question;
        let page_attr = if page_item { " data-page-item" } else { "" };
        let mut html = format!(
            r#"<div class="ada-q__item"{page_attr}><p class="ada-q__prompt">{prompt}</p>"#,
            prompt = escape(&item.question),
        );
        if !item.options.is_empty() {
            let input_type = if item.multi_select { "checkbox" } else { "radio" };
            let name = question_field(
                q,
                &item.id,
                if item.multi_select { "choices" } else { "choice" },
            );
            let options = item
                .options
                .iter()
                .map(|option| {
                    let description = option
                        .description
                        .as_deref()
                        .map(|text| {
                            format!(
                                r#"<span class="ada-q__option-desc">{}</span>"#,
                                escape(text)
                            )
                        })
                        .unwrap_or_default();
                    format!(
                        r#"<label class="ada-q__option ada-focusable"><input type="{input_type}" name="{name}" value="{value}"><span class="ada-q__option-label">{label}</span>{description}</label>"#,
                        value = escape(&option.value),
                        label = escape(&option.label),
                    )
                })
                .collect::<Vec<_>>()
                .join("");
            html.push_str(&format!(r#"<div class="ada-q__options">{options}</div>"#));
        }
        if item.allow_text {
            let name = question_field(q, &item.id, "text");
            let placeholder = if item.options.is_empty() {
                "输入你的回答"
            } else {
                "补充说明（可选）"
            };
            html.push_str(&format!(
                r#"<textarea class="ada-q__text ada-focusable" name="{name}" rows="2" placeholder="{placeholder}"></textarea>"#,
                name = escape(&name),
                placeholder = escape(placeholder),
            ));
        }
        html.push_str("</div>");
        html
    }
}

/// 等待时长文案。入参是**绝对毫秒时间戳**（`startedAt`）——不是排序用的 `at`；
/// 0 / 缺失表示不知道，返回空串（宁缺毋错，不拿序号估时长）。
pub fn waited_from_ms(when: u64) -> String {
    if when == 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(when);
    let seconds = now.saturating_sub(when) / 1000;
    if seconds < 60 {
        format!("已等待 {seconds}s")
    } else if seconds < 3600 {
        format!("已等待 {}m {:02}s", seconds / 60, seconds % 60)
    } else if seconds < 86400 {
        format!("已等待 {}h {:02}m", seconds / 3600, (seconds % 3600) / 60)
    } else {
        format!("已等待 {}d {}h", seconds / 86400, (seconds % 86400) / 3600)
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
    use crate::questions::normalize_question;
    use serde_json::json;

    #[test]
    fn multi_question_tray_renders_paging_contract() {
        let raw = json!({
            "question": "首题", "choices": [], "allow_text": true,
            "questions": [
                {"id": "q1", "question": "一", "choices": [{"id": "c1", "label": "A"}], "allow_text": true},
                {"id": "q2", "question": "二", "choices": [], "allow_text": true},
            ],
        });
        let question = normalize_question(&raw, "call_x").expect("question");
        let html = QuestionTray {
            question,
            thread_id: "t1".to_string(),
        }
        .render();
        assert!(html.contains("data-question-pager"), "{html}");
        assert_eq!(html.matches("data-page-item").count(), 2, "{html}");
        assert!(html.contains("data-page-bar hidden"), "{html}");
        // 翻页按钮在 form 里，必须是 type="button"（默认类型会把回答误提交）
        assert!(html.contains("type=\"button\""), "{html}");
        assert!(html.contains("q_q1__choice"), "{html}");
    }

    #[test]
    fn single_question_tray_has_no_pager() {
        let raw = json!({"question": "选一个", "choices": [{"id": "c1", "label": "A"}]});
        let question = normalize_question(&raw, "call_y").expect("question");
        let html = QuestionTray {
            question,
            thread_id: "t1".to_string(),
        }
        .render();
        assert!(!html.contains("data-page-bar"), "{html}");
        assert!(!html.contains("data-page-item"), "{html}");
        assert!(html.contains("name=\"choice\""), "{html}");
    }

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
