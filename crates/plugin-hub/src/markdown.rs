use pulldown_cmark::{Options, Parser, html};

/// README 渲染：先 Markdown → HTML，再用 ammonia 白名单清洗。
///
/// 插件 README 是用户输入，不能直接 `|safe` 输出。
pub fn render(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    let parser = Parser::new_ext(src, opts);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    ammonia::Builder::default()
        .link_rel(Some("noopener noreferrer nofollow"))
        .clean(&out)
        .to_string()
}
