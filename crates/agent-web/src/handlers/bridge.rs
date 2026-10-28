//! 统一插件的 SSR 桥接：前端插槽 → 核心路由 → `plugin.invoke`。
//!
//! 设计边界：
//! - 浏览器不接触网关 token，也不直接连 `/rpc`；
//! - `GET` 只允许 `read=true` 的绑定，`POST` 才允许写；
//! - 真正的 read/write 判定在后端动作的 `mode`，这里只负责把 `intent` 传下去；
//! - 普通表单 POST 走 303 回跳，fetch/HTMX 走 JSON。

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use super::{
    encode_query, public_reason, redirect, wants_partial, AppError, AppState,
};

const FORM_BODY_LIMIT: usize = 64 * 1024;

/// `GET/POST /ext/{plugin_id}/backend/{binding}`。
pub async fn backend_action(
    State(state): State<AppState>,
    Path((plugin_id, binding_name)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Response, AppError> {
    let plugin = state.frontend.find(&plugin_id).ok_or_else(|| {
        AppError::new(
            StatusCode::NOT_FOUND,
            format!("前端插件不存在: {plugin_id}"),
        )
    })?;
    if !plugin.is_active() {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            format!("前端插件未启用: {plugin_id}"),
        ));
    }
    let binding = plugin.action(&binding_name).ok_or_else(|| {
        AppError::new(
            StatusCode::NOT_FOUND,
            format!("动作绑定不存在: {}/{}", plugin_id, binding_name),
        )
    })?;
    if binding.backend_plugin_id.trim().is_empty() {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            format!("动作没有后端入口: {}/{}", plugin_id, binding_name),
        ));
    }

    let is_get = method == Method::GET;
    if is_get && !binding.read {
        return Err(AppError::new(
            StatusCode::METHOD_NOT_ALLOWED,
            format!("动作 {binding_name} 不允许 GET；写入动作必须用 POST"),
        ));
    }

    let (input, intent) = if is_get {
        let mut object = serde_json::Map::new();
        for (key, value) in query {
            object.insert(key, Value::String(value));
        }
        (Value::Object(object), "read")
    } else if method == Method::POST {
        if body.len() > FORM_BODY_LIMIT {
            return Err(AppError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "动作请求体超过 64KB",
            ));
        }
        (parse_post_body(&headers, &body)?, "write")
    } else {
        return Err(AppError::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "动作只接受 GET / POST",
        ));
    };

    let result = state
        .agent
        .invoke_plugin_action(
            &binding.backend_plugin_id,
            &binding.action,
            input,
            intent,
        )
        .await;

    // fetch / htmx 要 JSON；普通表单要 303 回跳。GET 永远 JSON。
    if is_get
        || wants_partial(&headers)
        || headers
            .get("hx-request")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    {
        return Ok(match result {
            Ok(value) => {
                let body = value
                    .as_object()
                    .cloned()
                    .map(Value::Object)
                    .unwrap_or_else(|| json!({ "ok": true, "data": value }));
                (StatusCode::OK, axum::Json(body)).into_response()
            }
            Err(error) => (
                StatusCode::BAD_GATEWAY,
                axum::Json(json!({
                    "ok": false,
                    "error": public_reason(&error),
                })),
            )
                .into_response(),
        });
    }

    let target = redirect_target(&headers, &plugin);
    let location = match result {
        Ok(_) => append_query(&target, "notice=backend_action_ok"),
        Err(error) => append_query(
            &target,
            &format!("error={}", encode_query(&public_reason(&error))),
        ),
    };
    Ok(redirect(&location))
}

/// POST 体：JSON 或普通表单。两者都归一化成 JSON 对象交给动作。
fn parse_post_body(headers: &HeaderMap, body: &Bytes) -> Result<Value, AppError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.contains("application/json") {
        let value: Value = serde_json::from_slice(body).map_err(|error| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                format!("动作 JSON 体不合法: {error}"),
            )
        })?;
        return Ok(value);
    }
    if content_type.contains("application/x-www-form-urlencoded") {
        let mut object = serde_json::Map::new();
        for (key, value) in form_urlencoded::parse(&body) {
            let key = key.into_owned();
            let value = Value::String(value.into_owned());
            match object.get_mut(&key) {
                None => {
                    object.insert(key, value);
                }
                Some(existing) => {
                    // 多选（checkbox 同名多值）必须保住全部值，否则“勾了两个只收到一个”。
                    if let Some(list) = existing.as_array_mut() {
                        list.push(value);
                    } else {
                        let first = existing.take();
                        *existing = Value::Array(vec![first, value]);
                    }
                }
            }
        }
        return Ok(Value::Object(object));
    }
    Err(AppError::new(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "动作 POST 只接受 application/json 或表单",
    ))
}

/// 回跳目标：只接受同源相对路径，避免开放重定向。
///
/// 插槽可能渲染在 `/`、`/threads/...` 或插件页，所以不能只限 `/ext/{id}`；
/// 但必须拒绝 `//evil`、反斜杠与 `..`。
fn redirect_target(headers: &HeaderMap, plugin: &crate::plugins::PluginRecord) -> String {
    let from_referer = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.parse::<axum::http::Uri>().ok())
        .map(|uri| {
            uri.path_and_query()
                .map(|path| path.as_str().to_string())
                .unwrap_or_else(|| uri.path().to_string())
        })
        .filter(|path| {
            path.starts_with('/')
                && !path.starts_with("//")
                && !path.contains('\\')
                && !path.split('/').any(|part| part == "..")
        });
    if let Some(path) = from_referer {
        return path;
    }
    plugin
        .pages
        .first()
        .map(|page| format!("/ext/{}/{}", plugin.id, page.slug))
        .unwrap_or_else(|| "/".to_string())
}

fn append_query(path: &str, query: &str) -> String {
    if path.contains('?') {
        format!("{path}&{query}")
    } else {
        format!("{path}?{query}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(content_type: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap());
        headers
    }

    #[test]
    fn post_body_accepts_json_and_form() {
        let json = parse_post_body(&headers("application/json"), &Bytes::from_static(br#"{"q":"hi"}"#))
            .unwrap();
        assert_eq!(json["q"], "hi");

        let form = parse_post_body(
            &headers("application/x-www-form-urlencoded"),
            &Bytes::from_static(b"q=hi&n=2"),
        )
        .unwrap();
        assert_eq!(form["q"], "hi");
        assert_eq!(form["n"], "2");
    }

    #[test]
    fn post_body_rejects_unknown_content_type() {
        let error = parse_post_body(&headers("text/plain"), &Bytes::from_static(b"hi")).unwrap_err();
        assert_eq!(error.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[test]
    fn query_append_keeps_existing_params() {
        assert_eq!(append_query("/", "a=1"), "/?a=1");
        assert_eq!(append_query("/?x=1", "a=1"), "/?x=1&a=1");
    }
}
