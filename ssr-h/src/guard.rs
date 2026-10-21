//! 入口防护：Host / Origin 校验、可选访问令牌、安全响应头。
//!
//! 为什么必须有这一层：`ssr-h` 把网关 token 代理在服务端，浏览器侧没有任何鉴权。
//! 只要它是 `http://127.0.0.1:8788` 上一个**不校验来源**的普通 Web 服务：
//!
//! - 任何网页都能向它提交表单，触发发送消息 / 批准工具 / 中止轮次（CSRF）；
//! - DNS rebinding 还能让攻击者的站点以同源身份读到会话内容（Host 未校验）；
//! - `--host 0.0.0.0` 会把整个 agent 连同网关凭据暴露到局域网。
//!
//! 这里的三道闸门分别对应上面三条：
//!
//! 1. **Host 白名单**：只接受本次绑定的主机名 + 回环别名；
//! 2. **Origin / Sec-Fetch-Site**：跨站写请求直接 403；
//! 3. **可选访问令牌**：非回环绑定必须设置（启动时就拒绝），回环也可主动开启。
//!
//! 令牌走 `SameSite=Strict` 的 cookie；首次可以用 `?token=…` 换取 cookie，
//! 随后 303 重定向把 URL 里的 token 去掉（避免留在历史记录/Referer 里）。

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{Form, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use crate::handlers::AppState;

/// 访问令牌 cookie 名。
const COOKIE: &str = "ssr_h_token";
/// 令牌有效期（秒）：本地面板，一周。
const COOKIE_MAX_AGE: u64 = 7 * 24 * 3600;

/// 入口防护配置。
pub struct Access {
    /// `None` = 不做令牌校验（仅回环且用户没主动设置时）。
    pub token: Option<String>,
    /// 允许的 Host 主机名（不含端口，已去方括号/小写）。
    pub hosts: Vec<String>,
    /// 绑定到 `0.0.0.0` / `::` 时无法枚举本机地址，只能放行任意 Host；
    /// 此时访问令牌是真正的闸门（远程绑定强制要求）。
    pub allow_any_host: bool,
    pub port: u16,
}

impl Access {
    pub fn for_host(host: IpAddr, port: u16, token: Option<String>) -> Self {
        let mut hosts = vec![normalize_host(&host.to_string())];
        if host.is_loopback() {
            for alias in ["127.0.0.1", "localhost", "::1"] {
                let alias = alias.to_string();
                if !hosts.iter().any(|existing| existing == &alias) {
                    hosts.push(alias);
                }
            }
        }
        let allow_any_host = host.is_unspecified();
        Self {
            token,
            hosts,
            allow_any_host,
            port,
        }
    }
}

/// 把 `[::1]` / `::1` 统一成可比较的主机名。
fn normalize_host(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

/// 拆分 `Host: host:port`（IPv6 带方括号）。缺端口按 80 处理。
fn split_hostport(raw: &str) -> Option<(String, u16)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':').map(str::trim).unwrap_or("80");
        return Some((normalize_host(host), port.parse().ok()?));
    }
    match raw.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            Some((normalize_host(host), port.trim().parse().ok()?))
        }
        Some(_) => None,
        None => Some((normalize_host(raw), 80)),
    }
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
}

/// Host 白名单：主机名必须在白名单里，端口必须与监听端口一致。
pub fn host_allowed(host_header: &str, access: &Access) -> bool {
    let Some((host, port)) = split_hostport(host_header) else {
        return false;
    };
    if port != access.port {
        return false;
    }
    if access.allow_any_host {
        return true;
    }
    access.hosts.iter().any(|allowed| allowed == &host)
}

/// Origin 必须与请求的 Host 完全一致（含端口）。`Origin: null` 一律拒绝。
pub fn origin_allowed(origin: &str, host_header: &str) -> bool {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    if authority.contains('@') || authority.contains('/') {
        return false;
    }
    authority.eq_ignore_ascii_case(host_header.trim())
}

/// 定长比较，避免按字节提前返回泄露前缀。
fn token_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn is_unsafe(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

fn cookie_token(headers: &HeaderMap, expected: &str) -> bool {
    let Some(raw) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    raw.split(';').any(|pair| {
        let Some((name, value)) = pair.trim().split_once('=') else {
            return false;
        };
        name == COOKIE && token_eq(value.trim(), expected)
    })
}

fn query_token(query: Option<&str>) -> Option<&str> {
    query?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token").then_some(value)
    })
}

/// 去掉 `?token=…`，保留其它查询参数。
fn strip_token(uri: &axum::http::Uri) -> String {
    let mut path = uri.path().to_string();
    if let Some(query) = uri.query() {
        let kept: Vec<&str> = query
            .split('&')
            .filter(|pair| !pair.is_empty() && !pair.starts_with("token="))
            .collect();
        if !kept.is_empty() {
            path.push('?');
            path.push_str(&kept.join("&"));
        }
    }
    path
}

fn with_cookie(location: &str, token: &str) -> Response {
    let cookie =
        format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={COOKIE_MAX_AGE}");
    let headers = [
        (
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie)
                .unwrap_or_else(|_| HeaderValue::from_static("ssr_h_token=; Max-Age=0")),
        ),
        (
            header::LOCATION,
            HeaderValue::from_str(location).unwrap_or(HeaderValue::from_static("/")),
        ),
    ];
    finish((StatusCode::SEE_OTHER, headers, ()).into_response())
}

fn auth_page(message: Option<&str>) -> Html<String> {
    let message = message
        .map(|text| format!(r#"<p class="error">{}</p>"#, escape(text)))
        .unwrap_or_default();
    Html(format!(
        r#"<!doctype html><meta charset="utf-8"><title>ssr-h 访问令牌</title>
<style>
body{{font:14px/1.6 system-ui;background:#18181b;color:#e4e4e7;display:grid;place-items:center;min-height:100vh;margin:0}}
form{{background:#27272a;border:1px solid #3f3f46;border-radius:12px;padding:24px;width:min(360px,86vw)}}
h1{{font-size:15px;margin:0 0 4px}}
p{{margin:0 0 14px;color:#a1a1aa;font-size:12px}}
input{{width:100%;box-sizing:border-box;padding:9px 11px;border-radius:8px;border:1px solid #52525b;background:#18181b;color:inherit;margin-bottom:12px}}
button{{width:100%;padding:9px 11px;border:0;border-radius:8px;background:#3b82f6;color:#fff;font-weight:600;cursor:pointer}}
.error{{color:#fca5a5}}
</style>
<form method="post" action="/__auth">
  <h1>ssr-h 需要访问令牌</h1>
  <p>这个实例开启了访问令牌，进入前请提供启动时设置的 <code>--ui-token</code>。</p>
  {message}
  <input type="password" name="token" placeholder="访问令牌" autocomplete="current-password" autofocus required>
  <button type="submit">进入</button>
</form>"#
    ))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unauthorized(message: Option<&str>) -> Response {
    finish(
        (
            StatusCode::UNAUTHORIZED,
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
            auth_page(message),
        )
            .into_response(),
    )
}

fn forbidden(message: &str) -> Response {
    finish(
        (
            StatusCode::FORBIDDEN,
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; charset=utf-8"),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
            message.to_string(),
        )
            .into_response(),
    )
}

/// 统一加安全响应头（所有返回路径，包括早退的 401/403）。
fn finish(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data: https:; style-src 'self' 'unsafe-inline'; \
             script-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; \
             base-uri 'none'; form-action 'self'",
        ),
    );
    response
}

/// 全局中间件。
pub async fn guard(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let access = state.access.clone();
    let path = request.uri().path().to_string();
    let host = header_str(request.headers(), header::HOST).unwrap_or_default();

    if !host_allowed(&host, &access) {
        return forbidden("拒绝访问：Host 头不在白名单内（只接受本机地址与配置的主机）。");
    }

    let method = request.method().clone();
    if is_unsafe(&method) {
        if let Some(origin) = header_str(request.headers(), header::ORIGIN) {
            if !origin_allowed(&origin, &host) {
                return forbidden("拒绝访问：跨站请求（Origin 与 Host 不一致）。");
            }
        }
        if let Some(site) = header_str(request.headers(), HeaderName::from_static("sec-fetch-site"))
        {
            if site.eq_ignore_ascii_case("cross-site") {
                return forbidden("拒绝访问：跨站请求被拒绝。");
            }
        }
    }

    if let Some(expected) = access.token.as_deref() {
        let needs_token = path != "/__auth" && !path.starts_with("/static/");
        if needs_token && !cookie_token(request.headers(), expected) {
            if method == Method::GET {
                if let Some(given) = query_token(request.uri().query()) {
                    if token_eq(given, expected) {
                        let location = strip_token(request.uri());
                        return with_cookie(&location, expected);
                    }
                }
            }
            return unauthorized(None);
        }
    }

    finish(next.run(request).await)
}

/// `GET /__auth`：手动输入令牌的入口（没有 cookie 时）。
pub async fn auth_form(State(state): State<AppState>) -> Response {
    match state.access.token.as_deref() {
        Some(_) => finish((StatusCode::OK, auth_page(None)).into_response()),
        None => finish(
            (
                StatusCode::SEE_OTHER,
                [(header::LOCATION, HeaderValue::from_static("/"))],
                (),
            )
                .into_response(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct AuthForm {
    token: String,
}

/// `POST /__auth`：校验令牌并下发 cookie。
pub async fn auth_submit(State(state): State<AppState>, Form(form): Form<AuthForm>) -> Response {
    match state.access.token.as_deref() {
        Some(expected) if token_eq(form.token.trim(), expected) => with_cookie("/", expected),
        Some(_) => unauthorized(Some("令牌不正确，请重试。")),
        None => finish(
            (
                StatusCode::SEE_OTHER,
                [(header::LOCATION, HeaderValue::from_static("/"))],
                (),
            )
                .into_response(),
        ),
    }
}

/// 令牌字符集：只允许 URL / cookie 都安全的字符，避免编码歧义。
pub fn validate_token(token: &str) -> Result<(), String> {
    if token.len() < 12 {
        return Err("--ui-token 至少 12 个字符".to_string());
    }
    if !token
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~'))
    {
        return Err("--ui-token 只能包含 ASCII 字母、数字与 - _ . ~".to_string());
    }
    Ok(())
}

/// 供 main 打印的提示。
pub fn access_summary(access: &Access) -> String {
    let auth = if access.token.is_some() {
        "访问令牌已开启"
    } else {
        "未开启访问令牌（仅限回环）"
    };
    format!("{auth}；允许的 Host: {}", access.hosts.join(", "))
}

/// 让 `AppState` 能直接持有。
pub type SharedAccess = Arc<Access>;

#[cfg(test)]
mod tests {
    use super::*;

    fn access() -> Access {
        Access::for_host("127.0.0.1".parse().unwrap(), 8788, None)
    }

    #[test]
    fn host_whitelist_accepts_loopback_aliases_with_matching_port() {
        let access = access();
        assert!(host_allowed("127.0.0.1:8788", &access));
        assert!(host_allowed("localhost:8788", &access));
        assert!(host_allowed("[::1]:8788", &access));
        assert!(!host_allowed("evil.example:8788", &access));
        assert!(!host_allowed("127.0.0.1:9999", &access));
        assert!(!host_allowed("", &access));
    }

    #[test]
    fn unspecified_bind_allows_any_host_but_not_any_port() {
        let access = Access::for_host(
            "0.0.0.0".parse().unwrap(),
            8788,
            Some("token-123456".into()),
        );
        assert!(host_allowed("192.168.1.5:8788", &access));
        assert!(host_allowed("evil.example:8788", &access));
        assert!(!host_allowed("192.168.1.5:9999", &access));
    }

    #[test]
    fn origin_must_match_host_exactly() {
        assert!(origin_allowed("http://127.0.0.1:8788", "127.0.0.1:8788"));
        assert!(origin_allowed("https://LOCALHOST:8788", "localhost:8788"));
        assert!(!origin_allowed("http://127.0.0.1:8788", "127.0.0.1:9999"));
        assert!(!origin_allowed("http://evil.example", "127.0.0.1:8788"));
        assert!(!origin_allowed("null", "127.0.0.1:8788"));
        assert!(!origin_allowed(
            "http://user@127.0.0.1:8788",
            "127.0.0.1:8788"
        ));
    }

    #[test]
    fn token_comparison_and_cookie_parsing() {
        assert!(token_eq("abc-123", "abc-123"));
        assert!(!token_eq("abc-123", "abc-124"));
        assert!(!token_eq("abc", "abc-123"));

        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; ssr_h_token=secret-token; b=2"),
        );
        assert!(cookie_token(&headers, "secret-token"));
        assert!(!cookie_token(&headers, "other"));
    }

    #[test]
    fn token_charset_is_restricted() {
        assert!(validate_token("0123456789ab").is_ok());
        assert!(validate_token("short").is_err());
        assert!(validate_token("0123456789a;").is_err());
        assert!(validate_token("中文令牌中文令牌").is_err());
    }

    #[test]
    fn strip_token_keeps_other_query_params() {
        let uri: axum::http::Uri = "/threads/t1?notice=sent&token=abc&x=1".parse().unwrap();
        assert_eq!(strip_token(&uri), "/threads/t1?notice=sent&x=1");
        let uri: axum::http::Uri = "/?token=abc".parse().unwrap();
        assert_eq!(strip_token(&uri), "/");
    }
}
