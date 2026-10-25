//! 助手正文的 Markdown 渲染。
//!
//! 模型输出属于不可信文本：先用 pulldown-cmark 转 HTML，再交给 ammonia
//! 白名单清洗，最后才以 `|safe` 交给模板。这样既保留标题/代码块/表格，
//! 又不会把模型生成的 `<script>` 直接注进页面。

use ammonia::Builder;
use pulldown_cmark::{html, Options, Parser};

pub fn to_html(input: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    // GFM 的删除线/表格等。**注意**：pulldown-cmark 0.12 的 GFM 开关
    // 不包含"裸 URL 自动成链"，所以下面先做一遍保守的 URL 预处理。
    options.insert(Options::ENABLE_GFM);

    let prepared = linkify_bare_urls(input);
    let parser = Parser::new_ext(&prepared, options);
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);

    let mut cleaner = Builder::default();
    cleaner.link_rel(Some("noopener noreferrer"));
    cleaner.clean(&rendered).to_string()
}

/// 把裸 URL 包成 CommonMark 自动链接 `<https://…>`。
///
/// pulldown-cmark 0.12 只认 `<https://…>` 形式的自动链接，不认 GFM 的裸 URL；
/// 而工具输出里"标题 + 裸链接"是常态，不处理的话链接永远点不动。
/// 这里刻意保守：跳过 fenced code、行内代码、缩进代码块，以及已经处在
/// `[text](url)` / `<url>` 里的 URL；末尾的英文/中文标点不吞进链接。
fn linkify_bare_urls(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 32);
    let mut fence: Option<&'static str> = None;
    for line in input.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(marker) = fence {
            out.push_str(line);
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some("```");
            out.push_str(line);
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some("~~~");
            out.push_str(line);
            continue;
        }
        // 缩进代码块（4 空格 / Tab）不碰。
        if line.starts_with("    ") || line.starts_with('\t') {
            out.push_str(line);
            continue;
        }
        out.push_str(&linkify_line(line));
    }
    out
}

fn linkify_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    while cursor < line.len() {
        let rest = &line[cursor..];
        let next = match (rest.find("http://"), rest.find("https://")) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some(offset) = next else {
            out.push_str(rest);
            break;
        };
        let start = cursor + offset;
        let prev = line[..start].chars().next_back();
        let already = matches!(prev, Some('(' | '<' | '[' | '`'));
        let in_word = prev
            .map(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '/' | ':'))
            .unwrap_or(false);
        if already || in_word || is_inside_inline_code(line, start) {
            // 已经处理过或位于代码里：原样跳过这个前缀，继续找下一个。
            out.push_str(&line[cursor..start + 1]);
            cursor = start + 1;
            continue;
        }
        out.push_str(&line[cursor..start]);
        let end = line[start..]
            .find(|ch: char| {
                ch.is_whitespace()
                    || matches!(
                        ch,
                        '<' | '>'
                            | '"'
                            | '\''
                            | ')'
                            | ']'
                            | '`'
                            | '，'
                            | '。'
                            | '；'
                            | '：'
                            | '！'
                            | '？'
                            | '、'
                            | '（'
                            | '）'
                            | '【'
                            | '】'
                            | '《'
                            | '》'
                            | '“'
                            | '”'
                            | '‘'
                            | '’'
                    )
            })
            .map(|index| start + index)
            .unwrap_or(line.len());
        let end = trim_trailing_punctuation(line, start, end);
        if end <= start + "https://".len() {
            out.push_str(&line[start..start + 1]);
            cursor = start + 1;
            continue;
        }
        let url = &line[start..end];
        if url.starts_with("http://") || url.starts_with("https://") {
            out.push('<');
            out.push_str(url);
            out.push('>');
        } else {
            out.push_str(url);
        }
        cursor = end;
    }
    out
}

fn trim_trailing_punctuation(line: &str, start: usize, end: usize) -> usize {
    let mut remaining = end;
    while remaining > start {
        let ch = line[..remaining].chars().next_back().unwrap_or(' ');
        if matches!(
            ch,
            '.' | ',' | ';' | ':' | '!' | '?' | '。' | '，' | '；' | '：' | '！' | '？'
        ) {
            remaining -= ch.len_utf8();
        } else {
            break;
        }
    }
    remaining
}

fn is_inside_inline_code(line: &str, position: usize) -> bool {
    line[..position].matches('`').count() % 2 == 1
}

#[cfg(test)]
mod tests {
    #[test]
    fn strips_scripts_javascript_urls_and_event_handlers() {
        let html = super::to_html(
            "<script>alert(1)</script>\n\n[点我](javascript:alert(1))\n\n<img src=\"x\" onerror=\"alert(1)\">",
        );
        assert!(!html.contains("<script"), "script 必须被清掉: {html}");
        assert!(
            !html.contains("javascript:"),
            "javascript: URL 必须被清掉: {html}"
        );
        assert!(!html.contains("onerror"), "内联事件必须被清掉: {html}");
        assert!(html.contains("点我"));
    }

    #[test]
    fn keeps_tables_and_linkifies_bare_urls() {
        let html = super::to_html("| a | b |\n|---|---|\n| 1 | 2 |\n\nhttps://example.com");
        assert!(html.contains("<table>"));
        assert!(html.contains("href=\"https://example.com\""), "{html}");
    }

    #[test]
    fn linkification_skips_code_and_existing_links() {
        let input = "\
裸链接 https://a.example/x?y=1。
代码 `https://code.example` 与
```
https://fenced.example
```
已有 [链接](https://linked.example)。
";
        let html = super::to_html(input);
        assert!(html.contains("href=\"https://a.example/x?y=1\""), "{html}");
        assert!(!html.contains("href=\"https://code.example\""), "{html}");
        assert!(!html.contains("href=\"https://fenced.example\""), "{html}");
        assert!(html.contains("href=\"https://linked.example\""), "{html}");
        assert!(
            html.contains("<a href=\"https://a.example/x?y=1\" rel=\"noopener noreferrer\">https://a.example/x?y=1</a>"),
            "{html}"
        );
    }

    #[test]
    fn linkification_does_not_eat_trailing_punctuation() {
        let html = super::to_html("见 https://example.com/a，然后继续。");
        assert!(html.contains("href=\"https://example.com/a\""), "{html}");
        assert!(!html.contains("href=\"https://example.com/a，"), "{html}");
    }
}
