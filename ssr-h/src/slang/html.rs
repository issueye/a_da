//! 内置 HTML 元素白名单、URL/class 校验与通用元素渲染辅助。
//!
//! Slang 的小写组件（`::div` / `::a` / `::img` …）走这里。它替代“插件作者手写
//! 任意 HTML”，但仍然允许常用的布局标签；`script` / `style` / `iframe` / `form`
//! 等不开放——表单控件请用 `::Button` / `::TextField` / `::Form` 等组件。

use super::component::ElementDef;
use super::error::{err, SlangResult, Span};

/// 全局属性（所有元素都可用的安全子集）。
pub const GLOBAL_ATTRS: &[&str] = &[
    "class", "id", "title", "role", "lang", "dir", "hidden", "tabindex",
];

const A_ATTRS: &[&str] = &["href", "target", "rel"];
const IMG_ATTRS: &[&str] = &["src", "alt", "width", "height", "loading"];
const TIME_ATTRS: &[&str] = &["datetime"];
const TD_ATTRS: &[&str] = &["colspan", "rowspan", "headers", "scope"];
const OL_ATTRS: &[&str] = &["start", "reversed", "type"];
const DETAILS_ATTRS: &[&str] = &["open"];
const PROGRESS_ATTRS: &[&str] = &["value", "max"];
const METER_ATTRS: &[&str] = &["value", "min", "max", "low", "high", "optimum"];

/// 全部内置元素。
pub fn builtin_elements() -> Vec<ElementDef> {
    let mut elements = Vec::new();
    macro_rules! element {
        ($tag:literal) => {
            elements.push(ElementDef {
                tag: $tag,
                allowed_attrs: &[],
                required_attrs: &[],
                void: false,
                primary_slot: "body",
            });
        };
        ($tag:literal, $attrs:expr) => {
            elements.push(ElementDef {
                tag: $tag,
                allowed_attrs: $attrs,
                required_attrs: &[],
                void: false,
                primary_slot: "body",
            });
        };
        ($tag:literal, $attrs:expr, void) => {
            elements.push(ElementDef {
                tag: $tag,
                allowed_attrs: $attrs,
                required_attrs: &[],
                void: true,
                primary_slot: "body",
            });
        };
    }

    element!("div");
    element!("span");
    element!("p");
    element!("h1");
    element!("h2");
    element!("h3");
    element!("h4");
    element!("h5");
    element!("h6");
    element!("strong");
    element!("em");
    element!("b");
    element!("i");
    element!("small");
    element!("mark");
    element!("del");
    element!("ins");
    element!("sub");
    element!("sup");
    element!("ul");
    element!("ol", OL_ATTRS);
    element!("li");
    element!("dl");
    element!("dt");
    element!("dd");
    element!("blockquote");
    element!("pre");
    element!("code");
    element!("kbd");
    element!("samp");
    element!("var");
    element!("table");
    element!("caption");
    element!("thead");
    element!("tbody");
    element!("tfoot");
    element!("tr");
    element!("th", TD_ATTRS);
    element!("td", TD_ATTRS);
    element!("figure");
    element!("figcaption");
    element!("hr", &[], void);
    element!("br", &[], void);
    element!("section");
    element!("article");
    element!("header");
    element!("footer");
    element!("nav");
    element!("main");
    element!("aside");
    element!("details", DETAILS_ATTRS);
    element!("summary");
    element!("time", TIME_ATTRS);
    element!("address");
    element!("abbr");
    element!("cite");
    element!("q");
    element!("a", A_ATTRS);
    element!("img", IMG_ATTRS, void);
    element!("progress", PROGRESS_ATTRS);
    element!("meter", METER_ATTRS);

    // 给少数元素补 required 属性。img.alt 必须显式给（装饰图给空串）。
    for element in &mut elements {
        if element.tag == "img" {
            element.required_attrs = &["src", "alt"];
        }
    }
    elements
}

/// 元素是否允许该属性（全局属性单独判断）。
pub fn element_allows_attr(element: &ElementDef, attr: &str) -> bool {
    if GLOBAL_ATTRS.contains(&attr) {
        return true;
    }
    if attr.starts_with("aria-") || attr.starts_with("data-") || attr.starts_with("hx-") {
        return true;
    }
    element.allowed_attrs.contains(&attr)
}

/// URL 校验：相对路径 / http(s) / mailto / tel；其余 scheme 拒绝。
pub fn sanitize_url(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '<' | '>' | '"' | '\'' | '`'))
    {
        return Err("URL 里包含不允许的字符".to_string());
    }
    let lower = value.to_ascii_lowercase();
    for scheme in ["javascript:", "vbscript:", "data:", "file:"] {
        if lower.starts_with(scheme) {
            return Err(format!("不允许的 URL scheme：{scheme}"));
        }
    }
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
        || lower.starts_with('/')
        || lower.starts_with('#')
        || lower.starts_with('?')
        || lower.starts_with("./")
        || lower.starts_with("../")
    {
        return Ok(value.to_string());
    }
    // 没有 scheme 的普通相对路径（例如 `ext/hello/page`）。
    if !value.contains(':') {
        return Ok(value.to_string());
    }
    Err(format!("不允许的 URL：{value}"))
}

/// class 校验/规范化：只保留安全的 token 字符，限制总长度。
pub fn sanitize_class(raw: &str) -> Result<String, String> {
    if raw.len() > 512 {
        return Err("class 太长（上限 512 字节）".to_string());
    }
    let mut tokens = Vec::new();
    for token in raw.split_whitespace() {
        if token.is_empty() {
            continue;
        }
        if !token.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '_' | '-' | ':' | '/' | '.' | '[' | ']' | '%' | '!' | '#'
                )
        }) {
            return Err(format!("class 里包含不允许的字符：`{token}`"));
        }
        tokens.push(token);
    }
    Ok(tokens.join(" "))
}

/// 给元素属性值做 HTML 转义。
pub fn escape_attr(value: &str) -> String {
    crate::ui::theme::escape(value)
}

/// 校验元素必填属性（在渲染前调用）。
pub fn check_required(element: &ElementDef, present: &[String], span: Span) -> SlangResult<()> {
    for required in element.required_attrs {
        if !present.iter().any(|name| name == required) {
            return err(
                "E3003",
                format!("元素 `::{}` 缺少必填属性 `{required}`", element.tag),
                span,
            );
        }
    }
    Ok(())
}
