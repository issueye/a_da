//! 工具调用卡片：折叠行 + 输入参数 + 长输出预览/展开。

use crate::ui::theme::escape;
use crate::ui::Tone;

use super::{Disclosure, Panel, Render};
use crate::ui::todo::{TodoList, TodoStep};

const PREVIEW_LINES: usize = 12;

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
        if !self.args.is_empty() {
            body.push_str(
                &Panel::new(self.args)
                    .title("输入参数")
                    .copy(self.args)
                    .render(),
            );
        }
        if !self.output.is_empty() {
            body.push_str(&self.output_panel());
        }
        if body.is_empty() {
            body.push_str(
                &Panel::new(String::new())
                    .html("<p>这个工具没有返回可展示的内容。</p>")
                    .render(),
            );
        }

        let mut row = Disclosure::new("tool", self.icon, self.label);
        if !self.target.is_empty() {
            row = row.detail(self.target);
        }
        if !self.dir.is_empty() {
            row = row.sub(self.dir);
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
        let line_count = self.output.lines().count();
        // 短输出直接展示；长输出只给预览，完整内容放进嵌套 details，避免一展开
        // 工具卡就铺满整屏。
        if line_count <= PREVIEW_LINES && self.output_html.is_empty() {
            return Panel::new(self.output)
                .title("执行输出")
                .copy(self.output)
                .render();
        }
        if line_count <= PREVIEW_LINES {
            return Panel::new(String::new())
                .html(self.output_html)
                .title("执行输出")
                .copy(self.output)
                .render();
        }

        let preview = if self.output_html.is_empty() {
            let lines: Vec<&str> = self.output.lines().take(PREVIEW_LINES).collect();
            format!(
                r#"<pre class="ada-panel__body ada-tool-output__preview">{}</pre>"#,
                escape(&lines.join("\n"))
            )
        } else {
            format!(
                r#"<div class="ada-panel__body ada-panel__body--html ada-tool-output__preview">{}</div>"#,
                self.output_html
            )
        };
        let full = if self.output_html.is_empty() {
            format!(
                r#"<pre class="ada-panel__body ada-tool-output__full">{}</pre>"#,
                escape(self.output)
            )
        } else {
            format!(
                r#"<div class="ada-panel__body ada-panel__body--html ada-tool-output__full">{}</div>"#,
                self.output_html
            )
        };
        format!(
            r#"<div class="ada-panel ada-tool-output">
  <div class="ada-panel__head">
    <span>执行输出</span>
    <span class="ada-tool-output__meta">共 {line_count} 行</span>
    <button class="ada-panel__copy" type="button" data-copy="{copy}" data-copy-label="复制">复制</button>
  </div>
  {preview}
  <details class="ada-tool-output__more">
    <summary>展开全文（共 {line_count} 行）</summary>
    {full}
  </details>
</div>"#,
            copy = escape(self.output),
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
        assert!(!html.contains("ada-tool-output__more"), "{html}");
    }

    #[test]
    fn long_output_gets_preview_and_expandable_full_text() {
        let output = (1..=20)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let html = call(&output).render();
        assert!(html.contains("共 20 行"), "{html}");
        assert!(html.contains("展开全文（共 20 行）"), "{html}");
        let preview = html
            .split("ada-tool-output__preview")
            .nth(1)
            .expect("preview")
            .split("</pre>")
            .next()
            .expect("preview body");
        assert!(preview.contains("line 12"), "{preview}");
        assert!(
            !preview.contains("line 13"),
            "预览只保留前 12 行: {preview}"
        );
        assert!(html.contains(r#"data-copy="line 1"#), "{html}");
    }
}
