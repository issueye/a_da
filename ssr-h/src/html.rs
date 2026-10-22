//! 内置 HTML 元素白名单、属性名/URL/class 校验与通用元素渲染辅助。
//!
//! TSX 里的小写标签（`<div>` / `<a>` / `<img>` …）走这里。它替代插件作者手写
//! 任意 HTML：`script` / `style` / `iframe` 不开放，表单类标签（`form` / `input` /
//! `button` / `label`）只按各自的属性白名单开放。
//!
//! 🔴 三道闸门缺一不可：
//! - **标签**：只有 [`builtin_elements`] 里登记过的标签；
//! - **属性名**：[`valid_attr_name`] + 元素白名单——名字不校验的话，
//!   `{"data-x\" onmouseover=\"alert(1)": 1}` 这种展开属性能把属性名"撑开"，
//!   值转义得再好也没用（HTML 属性名里出现引号 = 注入新属性）；
//! - **属性值**：`class` / `href` / `src` / `action` 走各自的规范化，其余转义。

/// 一个允许的 HTML 元素描述。
#[derive(Debug, Clone, Copy)]
pub struct ElementDef {
    pub tag: &'static str,
    pub allowed_attrs: &'static [&'static str],
    pub required_attrs: &'static [&'static str],
    pub void: bool,
}

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
const BUTTON_ATTRS: &[&str] = &["type", "name", "value", "disabled"];
const FORM_ATTRS: &[&str] = &["action", "method", "novalidate"];
const INPUT_ATTRS: &[&str] = &[
    "type",
    "name",
    "value",
    "placeholder",
    "disabled",
    "autocomplete",
    "inputmode",
    "maxlength",
    "min",
    "max",
    "step",
    "required",
    "readonly",
];
const LABEL_ATTRS: &[&str] = &["for"];

/// 全部内置元素（进程内只构造一次）。
pub fn builtin_elements() -> &'static [ElementDef] {
    static ELEMENTS: std::sync::OnceLock<Vec<ElementDef>> = std::sync::OnceLock::new();
    ELEMENTS.get_or_init(build_builtin_elements)
}

fn build_builtin_elements() -> Vec<ElementDef> {
    let mut elements = Vec::new();
    macro_rules! element {
        ($tag:literal) => {
            elements.push(ElementDef {
                tag: $tag,
                allowed_attrs: &[],
                required_attrs: &[],
                void: false,
            });
        };
        ($tag:literal, $attrs:expr) => {
            elements.push(ElementDef {
                tag: $tag,
                allowed_attrs: $attrs,
                required_attrs: &[],
                void: false,
            });
        };
        ($tag:literal, $attrs:expr, void) => {
            elements.push(ElementDef {
                tag: $tag,
                allowed_attrs: $attrs,
                required_attrs: &[],
                void: true,
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
    element!("button", BUTTON_ATTRS);
    element!("form", FORM_ATTRS);
    element!("input", INPUT_ATTRS, void);
    element!("label", LABEL_ATTRS);
    // TSX 组件库使用的自定义元素（行为在 static/ada-select.js / ada-modal.js）。
    element!("ada-select");
    element!("ada-modal");

    // 给少数元素补 required 属性。img.alt 必须显式给（装饰图给空串）。
    for element in &mut elements {
        if element.tag == "img" {
            element.required_attrs = &["src", "alt"];
        }
    }
    elements
}

/// 元素是否允许该属性（全局属性、`aria-*` / `data-*` 单独判断）。
///
/// `hx-*` 刻意**不**在这里按前缀放行：它走 [`hx_attr_allowed`] 的显式名单。
pub fn element_allows_attr(element: &ElementDef, attr: &str) -> bool {
    if GLOBAL_ATTRS.contains(&attr) {
        return true;
    }
    if attr.starts_with("aria-") || attr.starts_with("data-") {
        return true;
    }
    element.allowed_attrs.contains(&attr)
}

/// 允许的 HTMX 属性。
///
/// 🔴 不再按 `hx-` 前缀裸放行：`hx-on:*` 等价于内联脚本，`hx-vals='js:…'` /
/// `hx-headers='js:…'` / `hx-trigger='… js:…'` 会被 HTMX 在浏览器里求值。
/// 少了这张名单，"服务端渲染不出脚本"就只是一句口号。
const HX_ATTRS: &[&str] = &[
    "hx-get",
    "hx-post",
    "hx-put",
    "hx-patch",
    "hx-delete",
    "hx-target",
    "hx-swap",
    "hx-trigger",
    "hx-include",
    "hx-indicator",
    "hx-confirm",
    "hx-push-url",
    "hx-select",
    "hx-select-oob",
    "hx-swap-oob",
    "hx-vals",
    "hx-headers",
    "hx-params",
    "hx-encoding",
    "hx-disabled-elt",
];

/// 这些属性的值可以被 HTMX 当 JS 求值（`js:` 前缀），需要额外校验。
const HX_JS_VALUE_ATTRS: &[&str] = &["hx-vals", "hx-headers", "hx-trigger"];

/// 是否是允许的 HTMX 属性（大小写不敏感——浏览器会把 HTML 属性名小写化）。
pub fn hx_attr_allowed(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    HX_ATTRS.contains(&lower.as_str())
}

/// HTMX 属性值是否安全：`hx-vals` / `hx-headers` / `hx-trigger` 拒绝 `js:` 内联脚本。
pub fn hx_value_allowed(name: &str, value: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if !HX_JS_VALUE_ATTRS.contains(&lower.as_str()) {
        return true;
    }
    !value.to_ascii_lowercase().contains("js:")
}

/// 属性名校验：只接受不会被 HTML 解析器"截断"或"撑开"的名字。
///
/// 规则对齐 HTML 属性名的事实取值范围：首字符是字母 / `_` / `:`，其余是
/// 字母数字或 `-` `_` `:` `.`。据此 `"`、空格、`=`、`/`、`>` 全部被挡在外面。
pub fn valid_attr_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == ':') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':' | '.'))
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
    // 反斜杠：浏览器会把 `\\evil.com` 规整成 `//evil.com`（协议相对地址），
    // 等于放行一个没写 scheme 的外站跳转。
    if value.contains('\\') {
        return Err("URL 里不允许反斜杠".to_string());
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

/// 校验元素必填属性（在渲染前调用）。
pub fn check_required(element: &ElementDef, present: &[String]) -> Result<(), String> {
    for required in element.required_attrs {
        if !present.iter().any(|name| name == required) {
            return Err(format!(
                "元素 `<{}>` 缺少必填属性 `{required}`",
                element.tag
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_names_that_would_break_out_are_rejected() {
        assert!(valid_attr_name("data-plugin-id"));
        assert!(valid_attr_name("aria-labelledby"));
        assert!(valid_attr_name("xml:lang"));
        // 借属性名注入另一个属性 / 提前闭合标签的写法。
        assert!(!valid_attr_name("data-x\" onmouseover=\"alert(1)"));
        assert!(!valid_attr_name("data-x onload"));
        assert!(!valid_attr_name("data-x=1"));
        assert!(!valid_attr_name("id>x"));
        assert!(!valid_attr_name("1data"));
        assert!(!valid_attr_name(""));
    }

    #[test]
    fn htmx_attributes_are_an_explicit_allowlist() {
        assert!(hx_attr_allowed("hx-post"));
        assert!(hx_attr_allowed("HX-Post"));
        // 内联脚本口子。
        assert!(!hx_attr_allowed("hx-on"));
        assert!(!hx_attr_allowed("hx-on:click"));
        assert!(!hx_attr_allowed("hx-ON:click"));
        assert!(!hx_attr_allowed("hx-evil"));
    }

    #[test]
    fn htmx_js_prefixed_values_are_rejected() {
        assert!(hx_value_allowed("hx-vals", r#"{"a":1}"#));
        assert!(!hx_value_allowed("hx-vals", "js:alert(1)"));
        assert!(!hx_value_allowed("hx-vals", r#"{"a":"js:alert(1)"}"#));
        assert!(!hx_value_allowed("hx-headers", "js:{x:1}"));
        assert!(!hx_value_allowed("hx-trigger", "click[js:alert(1)]"));
        // 非 JS 属性不管值。
        assert!(hx_value_allowed("hx-post", "js:whatever"));
    }

    #[test]
    fn urls_with_backslashes_are_rejected() {
        assert!(sanitize_url("\\\\evil.example").is_err());
        assert!(sanitize_url("/ext/hello/app.js").is_ok());
        assert!(sanitize_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn element_table_is_built_once_and_reused() {
        let first = builtin_elements();
        let second = builtin_elements();
        assert!(std::ptr::eq(first, second));
        assert!(first.iter().any(|element| element.tag == "div"));
    }
}
