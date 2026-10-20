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
    // GFM：裸 URL 自动成链、删除线用 ~~text~~。
    // 工具结果里大量是"标题 + 裸链接"，没有自动链接就是一坨断不开的 URL。
    options.insert(Options::ENABLE_GFM);

    let parser = Parser::new_ext(input, options);
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);

    let mut cleaner = Builder::default();
    cleaner.link_rel(Some("noopener noreferrer"));
    cleaner.clean(&rendered).to_string()
}
