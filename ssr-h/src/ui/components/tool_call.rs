//! 工具调用卡片：折叠行（做了什么 + 得到什么）+ 输入参数 + 分型输出。
//!
//! 折叠行是扫读单位，所以结果尾标与耗时都在行内；展开后**输出在前、参数在后**——
//! 用户点开一张工具卡是为了看结果，不是为了读参数 JSON。

use crate::ui::theme::escape;
use crate::ui::Tone;

use super::{Disclosure, Panel, Render};
use crate::ui::todo::{TodoList, TodoStep};

/// 折叠态预览行数：再多就交给「展开全部」，避免一展开就铺满整屏。
const PREVIEW_LINES: usize = 10;

pub struct ToolCall<'a> {
    pub icon: &'a str,
    pub label: &'a str,
    pub target: &'a str,
    pub dir: &'a str,
    pub elapsed: &'a str,
    pub status: &'a str,
    pub status_tone: Tone,
    pub args: &'a str,
    pub output: &'a str,
    pub output_html: &'a str,
    pub todo_steps: &'a [TodoStep],
    pub todo_notes: Option<&'a str>,
    /// 结果尾标（`167 行` / `+18 -4` / `exit 0`）。
    pub result: &'a str,
    pub result_tone: Tone,
    /// 执行中：左侧色条呼吸 + 客户端实时计时。
    pub running: bool,
    /// 计时基点（epoch 毫秒）。
    pub started_at: Option<i64>,
    /// 输出总行数（按截断前原文统计）。
    pub total_lines: usize,
    /// 稳定身份（item id）→ details 的 data-key。**没有它，展开态无法跨
    /// 流式替换保留**：components.js 按 data-key 记录/回填 openKeys，空 key
    /// 的行在 item 更新（状态/耗时/输出变化）替换节点时必然被收起。
    pub key: &'a str,
}

impl<'a> ToolCall<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        icon: &'a str,
        label: &'a str,
        target: &'a str,
        dir: &'a str,
        elapsed: &'a str,
        status: &'a str,
        status_tone: Tone,
        args: &'a str,
        output: &'a str,
        output_html: &'a str,
        todo_steps: &'a [TodoStep],
        todo_notes: Option<&'a str>,
    ) -> Self {
        Self {
            icon,
            label,
            target,
            dir,
            elapsed,
            status,
            status_tone,
            args,
            output,
            output_html,
            todo_steps,
            todo_notes,
            result: "",
            result_tone: Tone::Neutral,
            running: false,
            started_at: None,
            total_lines: 0,
            key: "",
        }
    }

    /// 稳定身份（item id）——流式期间行会被反复替换，展开态靠它恢复。
    pub fn key(mut self, key: &'a str) -> Self {
        self.key = key;
        self
    }

    /// 结果尾标：折叠态回答"这一步得到了什么"。
    pub fn result(mut self, result: &'a str, tone: Tone) -> Self {
        self.result = result;
        self.result_tone = tone;
        self
    }

    /// 执行中：色条呼吸 + 每秒计时。
    pub fn running(mut self, running: bool) -> Self {
        self.running = running;
        self
    }

    /// 计时基点（epoch 毫秒）。
    pub fn started_at(mut self, ms: Option<i64>) -> Self {
        self.started_at = ms;
        self
    }

    /// 输出总行数（截断前）：按钮上要写真实规模。
    pub fn total_lines(mut self, lines: usize) -> Self {
        self.total_lines = lines;
        self
    }

    /// 语义阶段：运行中 / 待批 / 失败。完成态不给色——那才是默认。
    fn phase(&self) -> Option<&'static str> {
        if self.running {
            return Some("running");
        }
        match self.status_tone {
            Tone::Danger => Some("danger"),
            Tone::Warn => Some("warn"),
            _ => None,
        }
    }
}

impl Render for ToolCall<'_> {
    fn render(&self) -> String {
        let mut body = String::new();
        if !self.todo_steps.is_empty() {
            body.push_str(
                &TodoList {
                    steps: self.todo_steps.to_vec(),
                    notes: self.todo_notes.map(str::to_string),
                }
                .render(),
            );
        }
        // 输出在前：点开卡片的人想看"它到底做了什么"，参数是补证据时才翻的。
        if !self.output.is_empty() {
            body.push_str(&self.output_panel());
        }
        if !self.args.is_empty() {
            body.push_str(
                &Panel::new(self.args)
                    .title("输入参数")
                    .copy(self.args)
                    .render(),
            );
        }
        if body.is_empty() {
            body.push_str(
                &Panel::new(String::new())
                    .html("<p>这个工具没有返回可展示的内容。</p>")
                    .render(),
            );
        }

        let mut row = Disclosure::new("tool", self.icon, self.label);
        if !self.key.is_empty() {
            row = row.key(self.key);
        }
        if !self.target.is_empty() {
            row = row.detail(self.target);
        }
        if !self.dir.is_empty() {
            row = row.sub(self.dir);
        }
        if !self.result.is_empty() {
            row = row.result(self.result, self.result_tone);
        }
        if let Some(phase) = self.phase() {
            row = row.phase(phase);
        }
        if let Some(ms) = self.started_at {
            row = row.started_at(ms);
        }
        if !self.elapsed.is_empty() {
            row = row.meta(self.elapsed);
        }
        if !self.status.is_empty() && self.status_tone != Tone::Neutral {
            row = row.badge(self.status, self.status_tone);
        }
        row.body(body).render()
    }
}

impl ToolCall<'_> {
    fn output_panel(&self) -> String {
        let line_count = self.total_lines.max(self.output.lines().count());
        // 短输出直接铺开；长输出只给预览 + 一层「展开全部」——
        // 两层折叠等于要用户点两次才看到全文。
        if line_count <= PREVIEW_LINES {
            return self.output_block(line_count, None);
        }
        let preview = if self.output_html.is_empty() {
            let lines: Vec<&str> = self.output.lines().take(PREVIEW_LINES).collect();
            format!(
                r#"<pre class="ada-tool-output__preview">{}</pre>"#,
                escape(&lines.join("\n"))
            )
        } else {
            self.output_html.to_string()
        };
        self.output_block(line_count, Some(preview))
    }

    /// 输出面板外壳：`preview` 为 `Some` 时渲染"预览 + 展开全部"。
    fn output_block(&self, line_count: usize, preview: Option<String>) -> String {
        let plain = self.output_html.is_empty();
        let body_class = if plain {
            "ada-panel__body ada-tool-output__scroll"
        } else {
            "ada-panel__body ada-panel__body--html ada-tool-output__scroll"
        };
        let inner = match preview {
            Some(preview) => {
                let full = if plain {
                    format!(
                        r#"<pre class="ada-tool-output__full">{}</pre>"#,
                        escape(self.output)
                    )
                } else {
                    // Markdown 不能安全切片：折叠交给 CSS 高度，展开只改高度。
                    String::new()
                };
                format!("{preview}{full}")
            }
            None => {
                if plain {
                    format!(
                        r#"<pre class="ada-tool-output__preview">{}</pre>"#,
                        escape(self.output)
                    )
                } else {
                    self.output_html.to_string()
                }
            }
        };
        let toggle = if line_count > PREVIEW_LINES {
            format!(
                r#"<button class="ada-tool-output__toggle" type="button" data-ada-expand="1" aria-expanded="false">展开全部 {line_count} 行</button>"#
            )
        } else {
            String::new()
        };
        let copy = if self.output.is_empty() {
            String::new()
        } else {
            format!(
                r#"<button class="ada-panel__copy" type="button" data-copy="{text}" data-copy-label="复制">复制</button>"#,
                text = escape(self.output)
            )
        };
        format!(
            r#"<div class="ada-panel ada-tool-output" data-expanded="{expanded}" data-body="{body}">
  <div class="ada-panel__head">
    <span class="ada-panel__title">执行输出</span>
    <span class="ada-tool-output__meta">共 {line_count} 行</span>
    {copy}
  </div>
  <div class="{body_class}">{inner}</div>
  {toggle}
</div>"#,
            // `shown` = 短输出直接铺开（没有全文块，也就没有可折叠的东西）；
            // 只有长输出才有 true/false 两种折叠态。
            expanded = if line_count > PREVIEW_LINES {
                "false"
            } else {
                "shown"
            },
            body = if plain { "text" } else { "html" },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(output: &str) -> ToolCall<'_> {
        ToolCall::new(
            "i-terminal",
            "执行命令",
            "cargo test",
            "",
            "1.2s",
            "完成",
            Tone::Neutral,
            r#"{"command":"cargo test"}"#,
            output,
            "",
            &[],
            None,
        )
    }

    #[test]
    fn short_output_is_rendered_inline() {
        let html = call("ok").render();
        assert!(html.contains("执行输出"), "{html}");
        assert!(!html.contains("data-ada-expand"), "{html}");
    }

    /// 稳定身份（item id）必须落到 details 的 data-key 上——流式期间行被反复
    /// 替换，components.js 靠它恢复展开态；没有它展开的工具行会被自动收起。
    #[test]
    fn tool_row_carries_data_key_for_fold_state_restoration() {
        let with_key = call("ok").key("item_7").render();
        assert!(with_key.contains(r#"data-key="item_7""#), "{with_key}");
        // 没给 key 的调用方（历史行为）不输出空 data-key
        let without = call("ok").render();
        assert!(!without.contains("data-key"), "{without}");
    }

    #[test]
    fn long_output_gets_single_layer_expand() {
        let output = (1..=20)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let html = call(&output).total_lines(20).render();
        assert!(html.contains("共 20 行"), "{html}");
        assert!(html.contains("展开全部 20 行"), "{html}");
        assert!(html.contains(r#"data-body="text""#), "{html}");
        // 只有一层折叠：不再出现嵌套的 details/summary。
        assert_eq!(html.matches("<details").count(), 1, "{html}");
        let preview = html
            .split("ada-tool-output__preview")
            .nth(1)
            .expect("preview")
            .split("</pre>")
            .next()
            .expect("preview body");
        assert!(preview.contains("line 10"), "{preview}");
        assert!(!preview.contains("line 11"), "预览只留前 10 行: {preview}");
        assert!(html.contains(r#"data-copy="line 1"#), "{html}");
    }

    #[test]
    fn output_comes_before_args() {
        let html = call("ok").render();
        let output_at = html.find("执行输出").expect("output");
        let args_at = html.find("输入参数").expect("args");
        assert!(output_at < args_at, "输出应排在参数前面: {html}");
    }

    #[test]
    fn result_tail_lands_on_the_folded_row() {
        let html = call("ok").result("exit 0", Tone::Success).render();
        assert!(
            html.contains(r#"ada-disclosure__result" data-tone="success">exit 0"#),
            "{html}"
        );
    }

    #[test]
    fn running_row_carries_phase_and_start_time() {
        let html = call("")
            .running(true)
            .started_at(Some(1_700_000_000_000))
            .render();
        assert!(html.contains(r#"data-phase="running""#), "{html}");
        assert!(
            html.contains(r#"data-started-at="1700000000000""#),
            "{html}"
        );
    }
}
