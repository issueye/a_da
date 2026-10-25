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

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Form, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use crate::handlers::AppState;

/// 访问令牌 cookie 名（静态 `--web-token`）。
const COOKIE: &str = "ssr_h_token";
/// 会话 cookie 名（网关 token / 配对码登录后换发的随机 id）。
const SESSION_COOKIE: &str = "ssr_h_session";
/// 令牌有效期（秒）：本地面板，一周。
const COOKIE_MAX_AGE: u64 = 7 * 24 * 3600;

/// 浏览器凭据校验端口。
///
/// 网关挂载本 crate 时注入一个实现：接受网关 `--token` 或一次性配对码；
/// 校验通过后由 guard 换发随机 session id（**不把网关 token 写进 cookie**）。
pub trait CredentialVerifier: Send + Sync {
    fn verify(&self, credential: &str) -> Result<(), String>;
}

/// 反向代理暴露给浏览器的公网来源。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PublicOrigin {
    scheme: String,
    host: String,
    port: u16,
}

/// 入口防护配置。
pub struct Access {
    /// 静态 `--web-token`；`None` = 不做静态令牌校验。
    pub token: Option<String>,
    /// 网关凭据校验器；与 `token` 可同时存在（任一通过即可）。
    verifier: Option<Arc<dyn CredentialVerifier>>,
    /// 已登录会话：session id → 过期时间（内存态，重启即失效）。
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
    /// 允许的 Host 主机名（不含端口，已去方括号/小写）。
    pub hosts: Vec<String>,
    /// 绑定到 `0.0.0.0` / `::` 时无法枚举本机地址，只能放行任意 Host；
    /// 此时访问令牌是真正的闸门（远程绑定强制要求）。
    pub allow_any_host: bool,
    pub port: u16,
    /// 反向代理对外暴露的 `scheme://host[:port]`（Host/Origin 校验用）。
    public_origins: Vec<PublicOrigin>,
    /// 公网是 https 时，登录 cookie 必须带 `Secure`。
    secure_cookie: bool,
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
            verifier: None,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            hosts,
            allow_any_host,
            port,
            public_origins: Vec::new(),
            secure_cookie: false,
        }
    }

    /// 声明反向代理对外的公网来源（`--web-public-url https://example.com`）。
    ///
    /// 代理通常会把 `Host` 改成 `example.com`（不带内部端口），浏览器发的
    /// `Origin` 则是 `https://example.com`。两种形态都在这里显式放行，
    /// 但**只放行这个配置过的来源**，不是"随便什么 Host 都收"。
    pub fn with_public_url(mut self, raw: &str) -> Result<Self, String> {
        let origin = parse_public_origin(raw)?;
        if origin.scheme == "https" {
            self.secure_cookie = true;
        }
        if !self.public_origins.iter().any(|existing| existing == &origin) {
            self.public_origins.push(origin);
        }
        Ok(self)
    }

    /// 挂上网关凭据校验器（`--token` / 配对码）。
    pub fn with_verifier(mut self, verifier: Arc<dyn CredentialVerifier>) -> Self {
        self.verifier = Some(verifier);
        self
    }

    fn has_login(&self) -> bool {
        self.token.is_some() || self.verifier.is_some()
    }
}

/// 把 `[::1]` / `::1` 统一成可比较的主机名。
fn normalize_host(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

/// 拆分 `Host: host:port`（IPv6 带方括号）。
///
/// 第三个返回值表示"端口是否显式出现"：反向代理常把 `Host` 写成不带端口的
/// `example.com`，此时不能用默认 80 去和公网 https 的 443 比较。
fn split_hostport_parts(raw: &str) -> Option<(String, u16, bool)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let Some(port) = tail.strip_prefix(':') else {
            return Some((normalize_host(host), 80, false));
        };
        return Some((normalize_host(host), port.trim().parse().ok()?, true));
    }
    match raw.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => Some((
            normalize_host(host),
            port.trim().parse().ok()?,
            true,
        )),
        Some(_) => None,
        None => Some((normalize_host(raw), 80, false)),
    }
}

/// 拆分 `Host`（缺端口按 80 处理；保留给既有调用与测试）。
fn split_hostport(raw: &str) -> Option<(String, u16)> {
    split_hostport_parts(raw).map(|(host, port, _)| (host, port))
}

/// 解析 `--web-public-url`：只接受 `http(s)://host[:port]`，路径/查询忽略。
fn parse_public_origin(raw: &str) -> Result<PublicOrigin, String> {
    let raw = raw.trim();
    let (scheme, rest) = raw
        .split_once("://")
        .ok_or_else(|| format!("公网地址必须以 http:// 或 https:// 开头：{raw}"))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!("公网地址只支持 http/https：{raw}"));
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim();
    if authority.is_empty() || authority.contains('@') {
        return Err(format!("公网地址缺少合法主机名：{raw}"));
    }
    let (host, parsed_port, explicit) = split_hostport_parts(authority)
        .ok_or_else(|| format!("公网地址端口非法：{raw}"))?;
    if host.is_empty() {
        return Err(format!("公网地址缺少主机名：{raw}"));
    }
    let port = if explicit {
        parsed_port
    } else if scheme == "https" {
        443
    } else {
        80
    };
    Ok(PublicOrigin { scheme, host, port })
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
}

/// Host 白名单：本机监听端口，或显式配置的反向代理公网来源。
pub fn host_allowed(host_header: &str, access: &Access) -> bool {
    let Some((host, port, explicit)) = split_hostport_parts(host_header) else {
        return false;
    };
    // 反向代理：Host 可能是 `example.com`（无端口）或 `example.com:443`。
    if access
        .public_origins
        .iter()
        .any(|origin| origin.host == host && (origin.port == port || !explicit))
    {
        return true;
    }
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

/// Origin 是否等于显式配置的反向代理公网来源。
fn origin_matches_public(origin: &str, access: &Access) -> bool {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return false;
    }
    if authority.contains('@') || authority.contains('/') {
        return false;
    }
    let Some((host, port, explicit)) = split_hostport_parts(authority) else {
        return false;
    };
    let port = if explicit {
        port
    } else if scheme == "https" {
        443
    } else {
        80
    };
    access
        .public_origins
        .iter()
        .any(|configured| {
            configured.scheme == scheme && configured.host == host && configured.port == port
        })
}

/// 写请求的来源判定：同源 Host，或显式配置的反向代理公网来源。
pub fn write_origin_allowed_for(
    origin: &str,
    host: &str,
    sec_fetch_site: Option<&str>,
    access: &Access,
) -> bool {
    if origin_matches_public(origin, access) {
        return true;
    }
    write_origin_allowed(origin, host, sec_fetch_site)
}

/// 写请求的来源判定：Origin 必须与 Host 一致；`Origin: null` 只在浏览器声明同源导航时放行。
pub fn write_origin_allowed(origin: &str, host: &str, sec_fetch_site: Option<&str>) -> bool {
    if origin_allowed(origin, host) {
        return true;
    }
    // `Origin: null` 有两个来源：隐私策略/扩展把来源抹掉（Chromium 在 `no-referrer` 下
    // 对**表单 POST** 就是这么干的），以及 sandbox iframe 之类的**不透明来源**。
    // 后者必须拒绝，前者放行才不至于让站内动作全废——区分它们只能看浏览器自己写的
    // Sec-Fetch-Site（禁改头，脚本无法伪造）。
    origin.eq_ignore_ascii_case("null")
        && sec_fetch_site.is_some_and(|value| value.eq_ignore_ascii_case("same-origin"))
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

fn cookie_value<'a>(headers: &'a HeaderMap, wanted: &str) -> Option<&'a str> {
    let raw = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())?;
    raw.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == wanted).then_some(value.trim())
    })
}

fn cookie_token(headers: &HeaderMap, expected: &str) -> bool {
    cookie_value(headers, COOKIE)
        .map(|value| token_eq(value, expected))
        .unwrap_or(false)
}

/// 会话 cookie 是否有效；顺手清掉过期条目。
fn session_cookie_valid(headers: &HeaderMap, access: &Access) -> bool {
    let Some(id) = cookie_value(headers, SESSION_COOKIE) else {
        return false;
    };
    let Ok(mut sessions) = access.sessions.lock() else {
        return false;
    };
    let now = Instant::now();
    sessions.retain(|_, expires| *expires > now);
    sessions.contains_key(id)
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

fn with_cookie(location: &str, token: &str, secure: bool) -> Response {
    let cookie = format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={COOKIE_MAX_AGE}{}",
        if secure { "; Secure" } else { "" }
    );
    cookie_redirect(location, &cookie)
}

/// 登录成功后换发随机 session id（网关 token 不进浏览器 cookie）。
fn with_session_cookie(location: &str, access: &Access) -> Response {
    let id = uuid::Uuid::new_v4().simple().to_string();
    if let Ok(mut sessions) = access.sessions.lock() {
        sessions.insert(
            id.clone(),
            Instant::now() + Duration::from_secs(COOKIE_MAX_AGE),
        );
    }
    let cookie = format!(
        "{SESSION_COOKIE}={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={COOKIE_MAX_AGE}{}",
        if access.secure_cookie { "; Secure" } else { "" }
    );
    cookie_redirect(location, &cookie)
}

fn cookie_redirect(location: &str, cookie: &str) -> Response {
    let headers = [
        (
            header::SET_COOKIE,
            HeaderValue::from_str(cookie)
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
    // 🔴 **不能用 `no-referrer`**：Chromium 在**表单 POST**（导航请求）上会把 Origin 发成
    // 字面量 `null`，于是 guard 的同源校验会把所有动作按钮一起拒掉——现象是"点新会话 /
    // 发送 / 审批全是 403 跨站请求"，而 `fetch()` 却正常（它照常带真实 Origin），极易误判。
    // 实测对照（同一浏览器、同一页面，只差这个响应头）：
    //   no-referrer → Host=127.0.0.1:8791 Origin=null
    //   无该头      → Host=127.0.0.1:8792 Origin=http://127.0.0.1:8792
    // `same-origin` 仍然不会把来源泄露给第三方（跨站请求不带 Referer）。
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
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
        let site = header_str(request.headers(), HeaderName::from_static("sec-fetch-site"));
        if let Some(origin) = header_str(request.headers(), header::ORIGIN) {
            if !write_origin_allowed_for(&origin, &host, site.as_deref(), &access) {
                // 两端都打出来：只报"不一致"时，线上根本猜不到是哪一侧有问题。
                tracing::warn!(
                    method = %method,
                    path = %path,
                    host = %host,
                    origin = %origin,
                    sec_fetch_site = site.as_deref().unwrap_or(""),
                    "拒绝跨站写请求：Origin 与 Host 不一致"
                );
                return forbidden("拒绝访问：跨站请求（Origin 与 Host 不一致）。");
            }
        }
        if let Some(value) = site.as_deref() {
            if value.eq_ignore_ascii_case("cross-site") {
                return forbidden("拒绝访问：跨站请求被拒绝。");
            }
        }
    }

    let needs_auth = path != "/__auth" && !path.starts_with("/static/");

    if needs_auth && access.has_login() {
        let static_ok = access
            .token
            .as_deref()
            .map(|expected| cookie_token(request.headers(), expected))
            .unwrap_or(false);
        if !static_ok && !session_cookie_valid(request.headers(), &access) {
            if method == Method::GET {
                if let Some(given) = query_token(request.uri().query()) {
                    if let Some(expected) = access.token.as_deref() {
                        if token_eq(given, expected) {
                            return with_cookie(
                                &strip_token(request.uri()),
                                expected,
                                access.secure_cookie,
                            );
                        }
                    }
                    if let Some(verifier) = access.verifier.as_deref() {
                        if verifier.verify(given).is_ok() {
                            return with_session_cookie(&strip_token(request.uri()), &access);
                        }
                    }
                }
            }
            return unauthorized(None);
        }
    } else if needs_auth
        && method == Method::GET
        && access.token.is_none()
        && query_token(request.uri().query()).is_some()
    {
        // 没有配置任何凭据时，仍把 URL 上的 `?token=` 摘掉：
        // 桌面端的「浏览器打开」拼的是网关 token（SSR 不需要它），
        // 让它留在地址栏/历史记录里是纯粹的泄漏面。
        let location = strip_token(request.uri());
        return finish(
            (
                StatusCode::SEE_OTHER,
                [(
                    header::LOCATION,
                    HeaderValue::from_str(&location).unwrap_or(HeaderValue::from_static("/")),
                )],
                (),
            )
                .into_response(),
        );
    }

    finish(next.run(request).await)
}

/// `GET /__auth`：手动输入令牌的入口（没有 cookie 时）。
pub async fn auth_form(State(state): State<AppState>) -> Response {
    if state.access.has_login() {
        finish((StatusCode::OK, auth_page(None)).into_response())
    } else {
        finish(
            (
                StatusCode::SEE_OTHER,
                [(header::LOCATION, HeaderValue::from_static("/"))],
                (),
            )
                .into_response(),
        )
    }
}

#[derive(Debug, Deserialize)]
pub struct AuthForm {
    token: String,
}

/// `POST /__auth`：校验令牌并下发 cookie。
pub async fn auth_submit(State(state): State<AppState>, Form(form): Form<AuthForm>) -> Response {
    let credential = form.token.trim();
    if let Some(expected) = state.access.token.as_deref() {
        if token_eq(credential, expected) {
            return with_cookie("/", expected, state.access.secure_cookie);
        }
    }
    if let Some(verifier) = state.access.verifier.as_deref() {
        return match verifier.verify(credential) {
            Ok(()) => with_session_cookie("/", &state.access),
            Err(message) => unauthorized(Some(&message)),
        };
    }
    if state.access.token.is_some() {
        return unauthorized(Some("令牌不正确，请重试。"));
    }
    finish(
        (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, HeaderValue::from_static("/"))],
            (),
        )
            .into_response(),
    )
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
    } else if access.verifier.is_some() {
        "网关凭据登录已开启（token / 配对码）"
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
    fn null_origin_only_passes_for_same_origin_navigation() {
        // Chromium 在 Referrer-Policy: no-referrer 下会给表单 POST 发 Origin: null，
        // 但 Sec-Fetch-Site 仍是 same-origin——这种必须放行，否则站内动作全 403。
        assert!(write_origin_allowed(
            "null",
            "127.0.0.1:8788",
            Some("same-origin")
        ));
        assert!(write_origin_allowed(
            "NULL",
            "127.0.0.1:8788",
            Some("SAME-ORIGIN")
        ));
        // 不透明来源（sandbox iframe）与伪造的跨站来源一律拒绝。
        assert!(!write_origin_allowed(
            "null",
            "127.0.0.1:8788",
            Some("cross-site")
        ));
        assert!(!write_origin_allowed("null", "127.0.0.1:8788", None));
        assert!(!write_origin_allowed(
            "http://evil.example",
            "127.0.0.1:8788",
            Some("same-origin")
        ));
        // 正常同源请求不依赖 Sec-Fetch-Site。
        assert!(write_origin_allowed(
            "http://127.0.0.1:8788",
            "127.0.0.1:8788",
            None
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

    #[test]
    fn public_url_allows_proxy_host_and_origin_only() {
        let access = access().with_public_url("https://example.com").unwrap();
        // 代理常见形态：Host 无端口 / 带 443；本机回环仍可用。
        assert!(host_allowed("example.com", &access));
        assert!(host_allowed("example.com:443", &access));
        assert!(host_allowed("127.0.0.1:8788", &access));
        assert!(!host_allowed("example.com:8443", &access));
        assert!(!host_allowed("evil.example", &access));

        assert!(origin_matches_public("https://example.com", &access));
        assert!(origin_matches_public("https://example.com:443", &access));
        assert!(!origin_matches_public("http://example.com", &access));
        assert!(!origin_matches_public("https://evil.example", &access));
        assert!(write_origin_allowed_for(
            "https://example.com",
            "127.0.0.1:8788",
            None,
            &access
        ));
        assert!(!write_origin_allowed_for(
            "https://evil.example",
            "127.0.0.1:8788",
            None,
            &access
        ));
    }

    #[test]
    fn public_url_https_marks_cookies_secure() {
        let https = access().with_public_url("https://example.com").unwrap();
        let response = with_session_cookie("/", &https);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(cookie.contains("; Secure"), "{cookie}");

        let http = access().with_public_url("http://example.com").unwrap();
        let response = with_session_cookie("/", &http);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(!cookie.contains("; Secure"), "{cookie}");
    }

    #[test]
    fn public_url_rejects_malformed_values() {
        assert!(access().with_public_url("example.com").is_err());
        assert!(access().with_public_url("ftp://example.com").is_err());
        assert!(access().with_public_url("https://").is_err());
        assert!(access().with_public_url("https://example.com:abc").is_err());
        assert!(access().with_public_url("https://user@example.com").is_err());
    }
}
