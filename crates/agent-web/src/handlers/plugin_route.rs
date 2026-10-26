//! 动态后端路由：`/ext/{id}/route/*` → `plugin.route` → TS 插件自己的路由表。
//!
//! 这是“后端能力只往 TS 里加”的兜底入口：宿主不认识具体接口，只负责
//! 收请求、做同源/大小/头校验、把响应构造出来。插件用 `ada.route({...})`
//! 声明 `GET /foo/:id` 之类的路由，新增接口不再需要改 Rust。
//!
//! 边界：
//! - 只有启用的统一包（声明了 `backend`）才有这个入口；
//! - 请求体 ≤1MB，响应体由 agent 侧再卡 2MB；
//! - `cookie` / `authorization` 等敏感头**不传给插件**（插件拿不到会话凭据）；
//! - 返回头只放行常见内容头、`Location`、`Set-Cookie` 与 `x-*`。

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::{Map, Value};

use super::{public_reason, AppState};

const ROUTE_BODY_LIMIT: usize = 1024 * 1024;
const ROUTE_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_QUERY_KEYS: usize = 64;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const DROPPED_REQUEST_HEADERS: &[&str] = &[
    "cookie",
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "set-cookie",
    "host",
    "content-length",
    "connection",
    "transfer-encoding",
    "upgrade",
    "te",
    "trailer",
    "proxy-connection",
    "keep-alive",
];

/// `/ext/{id}/route`（根路径）。
pub async fn plugin_route_root(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    method: Method,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    route_inner(
        state,
        plugin_id,
        "/".to_string(),
        method,
        headers,
        query,
        body,
    )
    .await
}

/// `/ext/{id}/route/{*path}`。
pub async fn plugin_route(
    State(state): State<AppState>,
    Path((plugin_id, path)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    route_inner(
        state,
        plugin_id,
        normalize_path(&path),
        method,
        headers,
        query,
        body,
    )
    .await
}

async fn route_inner(
    state: AppState,
    plugin_id: String,
    path: String,
    method: Method,
    headers: HeaderMap,
    query: Option<String>,
    body: Bytes,
) -> Response {
    let Some(plugin) = state.frontend.find(&plugin_id) else {
        return json_error(StatusCode::NOT_FOUND, "插件不存在");
    };
    if !plugin.is_active() {
        return json_error(StatusCode::NOT_FOUND, "插件未启用或加载失败");
    }
    let Some(backend_id) = plugin.backend_plugin_id.clone() else {
        return json_error(StatusCode::NOT_FOUND, "插件没有后端入口");
    };

    let method_name = method.as_str().to_ascii_uppercase();
    if !matches!(
        method_name.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    ) {
        return with_allow(
            json_error(StatusCode::METHOD_NOT_ALLOWED, "不支持的 HTTP 方法"),
            "GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS",
        );
    }
    if body.len() > ROUTE_BODY_LIMIT {
        return json_error(StatusCode::PAYLOAD_TOO_LARGE, "请求体超过 1MB 上限");
    }
    if path.len() > 2048 || path.split('/').any(|part| part == "." || part == "..") {
        return json_error(StatusCode::BAD_REQUEST, "路由路径不合法");
    }

    let pairs = match parse_query(query.as_deref()) {
        Ok(pairs) => pairs,
        Err(message) => return json_error(StatusCode::BAD_REQUEST, &message),
    };
    let thread_id = pairs
        .iter()
        .find(|(key, _)| key == "threadId")
        .map(|(_, value)| value.clone());
    let (body_value, body_kind) = match parse_request_body(&headers, &body) {
        Ok(parsed) => parsed,
        Err(message) => return json_error(StatusCode::BAD_REQUEST, &message),
    };

    let call = state.agent.plugin_route(
        &backend_id,
        &method_name,
        &path,
        query_to_json(&pairs),
        sanitize_request_headers(&headers),
        body_value,
        body_kind,
        thread_id.as_deref(),
    );
    let result = match tokio::time::timeout(ROUTE_TIMEOUT, call).await {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                &format!("插件路由调用失败：{}", public_reason(&error)),
            )
        }
        Err(_) => return json_error(StatusCode::GATEWAY_TIMEOUT, "插件路由调用超时"),
    };

    match result.get("ok").and_then(Value::as_bool) {
        Some(true) => match result.get("response") {
            Some(response) => build_route_response(&method_name, response),
            None => json_error(StatusCode::BAD_GATEWAY, "插件路由缺少 response 对象"),
        },
        Some(false) => {
            let status = result
                .get("status")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .and_then(|value| StatusCode::from_u16(value).ok())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let message = result
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("插件路由执行失败");
            json_error(status, message)
        }
        None => json_error(StatusCode::BAD_GATEWAY, "插件路由返回缺少 ok 字段"),
    }
}

fn normalize_path(raw: &str) -> String {
    let trimmed = raw.trim_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn parse_query(raw: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let raw = raw.unwrap_or("");
    let mut pairs: Vec<(String, String)> =
        serde_urlencoded::from_str(raw).map_err(|error| format!("查询串不合法: {error}"))?;
    // 网关 token 只用于 guard 换发会话 cookie；它不该出现在插件拿到的 query 里。
    pairs.retain(|(key, _)| key != "token");
    if pairs.len() > MAX_QUERY_KEYS {
        return Err(format!("查询参数过多（上限 {MAX_QUERY_KEYS}）"));
    }
    Ok(pairs)
}

fn query_to_json(pairs: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        match map.get_mut(key) {
            Some(Value::String(previous)) => {
                let previous = previous.clone();
                map.insert(
                    key.clone(),
                    Value::Array(vec![Value::String(previous), Value::String(value.clone())]),
                );
            }
            Some(Value::Array(items)) => items.push(Value::String(value.clone())),
            _ => {
                map.insert(key.clone(), Value::String(value.clone()));
            }
        }
    }
    Value::Object(map)
}

/// 请求头白名单：凭据类头一律不交给插件。
fn sanitize_request_headers(headers: &HeaderMap) -> Value {
    let mut map = Map::new();
    let mut total = 0usize;
    for (name, value) in headers.iter() {
        let name = name.as_str().to_ascii_lowercase();
        if DROPPED_REQUEST_HEADERS.contains(&name.as_str()) {
            continue;
        }
        if map.len() >= 64 || total >= MAX_HEADER_BYTES {
            break;
        }
        let Ok(text) = value.to_str() else {
            continue;
        };
        total += name.len() + text.len();
        match map.get_mut(&name) {
            Some(Value::String(previous)) => {
                let joined = format!("{previous}, {text}");
                map.insert(name, Value::String(joined));
            }
            _ => {
                map.insert(name, Value::String(text.to_string()));
            }
        }
    }
    Value::Object(map)
}

fn parse_request_body(headers: &HeaderMap, body: &[u8]) -> Result<(Value, &'static str), String> {
    if body.is_empty() {
        return Ok((Value::Null, "empty"));
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.contains("application/json") || content_type.contains("+json") {
        let value: Value = serde_json::from_slice(body)
            .map_err(|error| format!("请求体不是合法 JSON: {error}"))?;
        return Ok((value, "json"));
    }
    if content_type.contains("application/x-www-form-urlencoded") {
        let pairs: Vec<(String, String)> = serde_urlencoded::from_bytes(body)
            .map_err(|error| format!("请求表单不合法: {error}"))?;
        return Ok((query_to_json(&pairs), "form"));
    }
    match std::str::from_utf8(body) {
        Ok(text) => Ok((Value::String(text.to_string()), "text")),
        Err(_) => Ok((
            Value::String(base64::engine::general_purpose::STANDARD.encode(body)),
            "base64",
        )),
    }
}

fn build_route_response(method: &str, response: &Value) -> Response {
    let status = response
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .and_then(|value| StatusCode::from_u16(value).ok())
        .unwrap_or(StatusCode::OK);
    let body_kind = response
        .get("bodyKind")
        .and_then(Value::as_str)
        .unwrap_or("text");
    let body = response
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let bytes = if body_kind == "base64" {
        match base64::engine::general_purpose::STANDARD.decode(body.as_bytes()) {
            Ok(bytes) => bytes,
            Err(_) => return json_error(StatusCode::BAD_GATEWAY, "插件返回了非法 base64 响应体"),
        }
    } else {
        body.into_bytes()
    };

    let mut out = HeaderMap::new();
    if let Some(headers) = response.get("headers").and_then(Value::as_object) {
        for (name, value) in headers {
            if !route_response_header_allowed(name) {
                continue;
            }
            let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                return json_error(StatusCode::BAD_GATEWAY, "插件返回了非法响应头名");
            };
            let values: Vec<&str> = match value {
                Value::String(text) => vec![text.as_str()],
                Value::Array(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        let Some(text) = item.as_str() else {
                            return json_error(
                                StatusCode::BAD_GATEWAY,
                                "插件响应头的值必须是字符串或字符串数组",
                            );
                        };
                        out.push(text);
                    }
                    out
                }
                _ => {
                    return json_error(
                        StatusCode::BAD_GATEWAY,
                        "插件响应头的值必须是字符串或字符串数组",
                    )
                }
            };
            for value in values {
                let Ok(value) = HeaderValue::from_str(value) else {
                    return json_error(StatusCode::BAD_GATEWAY, "插件返回了非法响应头值");
                };
                out.append(name.clone(), value);
            }
        }
    }
    if !out.contains_key(header::CONTENT_TYPE) {
        let fallback = if body_kind == "base64" {
            "application/octet-stream"
        } else {
            "text/plain; charset=utf-8"
        };
        out.insert(header::CONTENT_TYPE, HeaderValue::from_static(fallback));
    }
    if !out.contains_key(header::CACHE_CONTROL) {
        out.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }

    // HEAD 与 204/304 不返回响应体（内容长度由 axum 按实际 body 计算）。
    let suppress_body =
        method == "HEAD" || status == StatusCode::NO_CONTENT || status == StatusCode::NOT_MODIFIED;
    let body = if suppress_body { Vec::new() } else { bytes };
    (status, out, body).into_response()
}

fn route_response_header_allowed(raw: &str) -> bool {
    let name = raw.trim().to_ascii_lowercase();
    name.starts_with("x-")
        || matches!(
            name.as_str(),
            "content-type"
                | "content-language"
                | "content-disposition"
                | "cache-control"
                | "etag"
                | "last-modified"
                | "expires"
                | "location"
                | "set-cookie"
                | "vary"
                | "retry-after"
                | "www-authenticate"
        )
}

fn json_error(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({ "error": message }).to_string();
    (
        status,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        body,
    )
        .into_response()
}

fn with_allow(mut response: Response, allow: &'static str) -> Response {
    if let Ok(value) = HeaderValue::from_str(allow) {
        response.headers_mut().insert(header::ALLOW, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn path_normalization_keeps_one_leading_slash() {
        assert_eq!(normalize_path("a/b"), "/a/b");
        assert_eq!(normalize_path("/a/b/"), "/a/b");
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("/"), "/");
    }

    #[test]
    fn query_repeats_become_arrays_and_are_decoded() {
        let pairs = parse_query(Some("a=1&a=2&b=%E4%BD%A0")).unwrap();
        let value = query_to_json(&pairs);
        assert_eq!(value["a"], serde_json::json!(["1", "2"]));
        assert_eq!(value["b"], "你");
    }

    #[test]
    fn gateway_token_never_reaches_the_plugin_query() {
        let pairs = parse_query(Some("token=SECRET&q=1")).unwrap();
        assert!(pairs.iter().all(|(key, _)| key != "token"));
        assert_eq!(query_to_json(&pairs)["q"], "1");
    }

    #[test]
    fn sensitive_request_headers_are_not_forwarded() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("session=secret"));
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer x"));
        headers.insert("x-trace", HeaderValue::from_static("abc"));
        let value = sanitize_request_headers(&headers);
        assert!(value.get("cookie").is_none());
        assert!(value.get("authorization").is_none());
        assert_eq!(value["x-trace"], "abc");
    }

    #[test]
    fn request_body_kinds_cover_json_form_text_and_binary() {
        let mut json_headers = HeaderMap::new();
        json_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let (value, kind) = parse_request_body(&json_headers, br#"{"a":1}"#).unwrap();
        assert_eq!(kind, "json");
        assert_eq!(value["a"], 1);

        let mut form_headers = HeaderMap::new();
        form_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let (value, kind) = parse_request_body(&form_headers, b"a=1&a=2").unwrap();
        assert_eq!(kind, "form");
        assert_eq!(value["a"], serde_json::json!(["1", "2"]));

        let (value, kind) = parse_request_body(&HeaderMap::new(), b"hello").unwrap();
        assert_eq!(kind, "text");
        assert_eq!(value, "hello");

        let (value, kind) = parse_request_body(&HeaderMap::new(), &[0xff, 0xfe]).unwrap();
        assert_eq!(kind, "base64");
        assert_eq!(value, "//4=");
    }

    #[test]
    fn response_header_allowlist_blocks_hop_by_hop_and_keeps_x_headers() {
        assert!(route_response_header_allowed("content-type"));
        assert!(route_response_header_allowed("set-cookie"));
        assert!(route_response_header_allowed("x-request-id"));
        assert!(!route_response_header_allowed("content-length"));
        assert!(!route_response_header_allowed("connection"));
        assert!(!route_response_header_allowed("proxy-authorization"));
    }
}
