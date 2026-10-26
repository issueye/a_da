//! 供应商与模型管理：Master-Detail 页面 + 全部动作端点。
//!
//! 与 原桌面端 的差异只有一处、且是刻意的：**API Key 不回显**。桌面端把整串密钥
//! 灌进输入框（`type=password` 只是遮住显示），这里只回 `已配置 + sk-…7f3a`，
//! 保存时留空表示"保持原值"、勾选"清空密钥"才真的清空。

use axum::extract::{Form, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::{json, Value};

use super::*;

/// 读取字符串配置；缺失/非字符串时给空串。
fn config_str(config: &Value, key: &str) -> String {
    config
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 页签 key 只有两个，查询参数写错时回落到「供应商信息」（不报错、渲染可预期）。
fn normalize_tab(tab: Option<&str>) -> &'static str {
    match tab.map(str::trim) {
        Some("models") => "models",
        _ => "connection",
    }
}

// ── 供应商管理（对齐 原桌面端 的 Master-Detail）────────────────────────────
//
// 与 原桌面端 的差异只有一处、且是刻意的：**API Key 不回显**。桌面端把整串密钥
// 灌进输入框（`type=password` 只是遮住显示），这里只回 `已配置 + sk-…7f3a`，
// 保存时留空表示"保持原值"、勾选"清空密钥"才真的清空。

fn protocol_label(value: &str) -> &'static str {
    match value {
        "anthropic" => "Anthropic",
        "openai_responses" => "OpenAI Responses",
        _ => "OpenAI Chat",
    }
}

fn protocol_options() -> Vec<ProtocolOption> {
    ["openai_chat", "openai_responses", "anthropic"]
        .iter()
        .map(|value| ProtocolOption {
            value: (*value).to_string(),
            label: protocol_label(value).to_string(),
        })
        .collect()
}

/// 密钥只给"是否已配置 + 首尾各 4 位"，整串绝不回到页面。
fn mask_api_key(key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "已配置".to_string();
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars.iter().skip(chars.len() - 4).collect();
    format!("{head}…{tail}")
}

/// `customHeaders` → `k: v; k2: v2`（与 原桌面端 的编辑框格式一致）。
fn headers_text(value: &Value) -> String {
    value
        .get("customHeaders")
        .and_then(Value::as_object)
        .map(|map| {
            let mut pairs: Vec<String> = map
                .iter()
                .map(|(key, value)| format!("{key}: {}", value.as_str().unwrap_or("")))
                .collect();
            pairs.sort();
            pairs.join("; ")
        })
        .unwrap_or_default()
}

/// `k: v; k2: v2` → `customHeaders` 对象；空文本给 `Null`（调用方据此删字段）。
fn parse_headers(text: &str) -> Value {
    let mut map = serde_json::Map::new();
    for item in text.split([';', '\n']) {
        let Some((key, value)) = item.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        // 只接受 `k: v`（`https://x` 这种冒号不构成请求头名）。
        if key.is_empty() || value.is_empty() || key.contains(char::is_whitespace) {
            continue;
        }
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
    if map.is_empty() {
        Value::Null
    } else {
        Value::Object(map)
    }
}

fn model_text(model: &Value, key: &str) -> String {
    model
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_default()
}

/// 给插件页面的供应商只读投影：**绝不含 apiKey / 自定义请求头原文**。
///
/// 只投影 id / 名称 / 协议 / 端点 / 掩码后的密钥提示 / 模型行；写操作仍走
/// 核心 `/providers/*` 动作端点，插件只负责渲染。
pub(crate) fn provider_admin_view(
    snapshot: &Value,
    selected: Option<&str>,
    tab: Option<&str>,
) -> Value {
    let config = snapshot.get("config").cloned().unwrap_or(Value::Null);
    let active_id = snapshot
        .get("activeProviderId")
        .and_then(Value::as_str)
        .or_else(|| config.get("activeProviderId").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let current_model = config_str(&config, "model");
    let raw = snapshot
        .get("providers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let providers: Vec<Value> = raw
        .iter()
        .map(|provider| {
            let id = str_at(provider, "id").to_string();
            let active = !active_id.is_empty() && active_id == id;
            let api_key = str_at(provider, "apiKey");
            let models: Vec<Value> = provider
                .get("models")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .enumerate()
                .map(|(index, model)| {
                    let model_id = str_at(model, "id").to_string();
                    let name = {
                        let name = str_at(model, "name");
                        if name.is_empty() {
                            model_id.clone()
                        } else {
                            name.to_string()
                        }
                    };
                    serde_json::json!({
                        "index": index,
                        "id": model_id,
                        "name": name,
                        "contextWindow": model_text(model, "contextWindow"),
                        "maxOutputTokens": model_text(model, "maxOutputTokens"),
                        "supportsImages": model.get("supportsImages").and_then(Value::as_bool).unwrap_or(true),
                        "current": active && !current_model.is_empty() && str_at(model, "id") == current_model,
                    })
                })
                .collect();
            serde_json::json!({
                "id": id,
                "name": str_at(provider, "name"),
                "protocol": str_at(provider, "protocol"),
                "protocolLabel": protocol_label(str_at(provider, "protocol")),
                "baseUrl": str_at(provider, "baseUrl"),
                "proxyUrl": str_at(provider, "proxyUrl"),
                "headers": headers_text(provider),
                "apiKeySet": !api_key.trim().is_empty(),
                "apiKeyHint": mask_api_key(api_key),
                "modelCount": models.len(),
                "active": active,
                "models": models,
            })
        })
        .collect();
    let mut current_base_url = config_str(&config, "baseUrl");
    if current_base_url.is_empty() {
        current_base_url = providers
            .iter()
            .find(|provider| provider.get("id").and_then(Value::as_str) == Some(active_id.as_str()))
            .and_then(|provider| provider.get("baseUrl"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
    }
    let requested = selected
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| {
            if active_id.is_empty() {
                providers
                    .first()
                    .and_then(|value| value.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
            } else {
                &active_id
            }
        });
    let selected_id = if providers
        .iter()
        .any(|provider| provider.get("id").and_then(Value::as_str) == Some(requested))
    {
        requested.to_string()
    } else {
        providers
            .first()
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    serde_json::json!({
        "providers": providers,
        "selectedId": selected_id,
        "activeTab": normalize_tab(tab),
        "currentModel": current_model,
        "currentBaseUrl": current_base_url,
        "canDeleteProviders": raw.len() > 1,
        "protocols": protocol_options()
            .into_iter()
            .map(|option| serde_json::json!({ "value": option.value, "label": option.label }))
            .collect::<Vec<_>>(),
    })
}

pub async fn providers_redirect(uri: axum::http::Uri) -> Redirect {
    let query = uri
        .query()
        .filter(|query| !query.is_empty())
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    Redirect::to(&format!("/ext/provider-admin/providers{query}"))
}
async fn provider_entry(state: &AppState, id: &str) -> Result<Value, AppError> {
    let values = state.agent.providers().await.map_err(|error| {
        AppError::upstream(format!("读取供应商失败：{}", public_reason(&error)))
    })?;
    array_at(&values, "providers")
        .iter()
        .find(|value| str_at(value, "id") == id)
        .cloned()
        .ok_or_else(|| AppError::bad(format!("未找到供应商：{id}")))
}

/// 动作成功后回跳。`tab` 必须是发起该动作的面板——否则每次保存都把人弹回
/// 「供应商信息」，改模型的人得重新点回「模型列表」。
fn provider_back(id: &str, tab: &str, notice: &str) -> Response {
    redirect(&format!(
        "/providers?provider={}&tab={}&notice={}",
        encode_query(id),
        normalize_tab(Some(tab)),
        encode_query(notice)
    ))
}

/// 保存后回跳；`back=remote` 时把弹窗一起带回来（弹窗内容由 htmx 重新拉取）。
fn provider_back_from(id: &str, tab: &str, notice: &str, back: Option<&str>) -> Response {
    let dialog = match back.map(str::trim) {
        Some("remote") => "&dialog=remote-models",
        _ => "",
    };
    redirect(&format!(
        "/providers?provider={}&tab={}&notice={}{dialog}",
        encode_query(id),
        normalize_tab(Some(tab)),
        encode_query(notice)
    ))
}

fn provider_failed(id: &str, tab: &str, prefix: &str, error: anyhow::Error) -> Response {
    tracing::warn!("{prefix}: {error:#}");
    let message = format!("{prefix}：{}", public_reason(&error));
    redirect(&format!(
        "/providers?provider={}&tab={}&error={}",
        encode_query(id),
        normalize_tab(Some(tab)),
        encode_query(&message)
    ))
}

/// 只接受插件页面/供应商页的相对回跳，避免开放重定向。
fn safe_plugin_next(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if !raw.starts_with("/")
        || raw.starts_with("//")
        || raw.contains('\\')
        || raw.chars().any(char::is_control)
        || !(raw.starts_with("/ext/") || raw.starts_with("/providers"))
    {
        return None;
    }
    Some(raw.to_string())
}

fn append_query(mut base: String, params: &[(&str, &str)]) -> String {
    for (key, value) in params {
        let separator = if base.contains('?') { '&' } else { '?' };
        base.push(separator);
        base.push_str(&format!("{key}={}", encode_query(value)));
    }
    base
}

fn provider_back_with_next(id: &str, tab: &str, notice: &str, next: Option<&str>) -> Response {
    if let Some(base) = safe_plugin_next(next) {
        return redirect(&append_query(
            base,
            &[
                ("provider", id),
                ("tab", normalize_tab(Some(tab))),
                ("notice", notice),
            ],
        ));
    }
    provider_back(id, tab, notice)
}

fn provider_back_from_with_next(
    id: &str,
    tab: &str,
    notice: &str,
    back: Option<&str>,
    next: Option<&str>,
) -> Response {
    if let Some(base) = safe_plugin_next(next) {
        let mut params = vec![
            ("provider", id.to_string()),
            ("tab", normalize_tab(Some(tab)).to_string()),
            ("notice", notice.to_string()),
        ];
        if back.map(str::trim) == Some("remote") {
            params.push(("dialog", "remote-models".to_string()));
        }
        let rendered: Vec<(&str, &str)> = params
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        return redirect(&append_query(base, &rendered));
    }
    provider_back_from(id, tab, notice, back)
}

fn provider_failed_with_next(
    id: &str,
    tab: &str,
    prefix: &str,
    error: anyhow::Error,
    next: Option<&str>,
) -> Response {
    if let Some(base) = safe_plugin_next(next) {
        tracing::warn!("{prefix}: {error:#}");
        let message = format!("{prefix}：{}", public_reason(&error));
        return redirect(&append_query(
            base,
            &[
                ("provider", id),
                ("tab", normalize_tab(Some(tab))),
                ("error", message.as_str()),
            ],
        ));
    }
    provider_failed(id, tab, prefix, error)
}

#[derive(Debug, Default, Deserialize)]
pub struct NextQuery {
    next: Option<String>,
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// 供应商连接字段（连接表单与"获取模型/测试"两个 htmx 端点共用）。
#[derive(Debug, Default, Deserialize)]
pub struct ProviderForm {
    name: Option<String>,
    protocol: Option<String>,
    #[serde(rename = "baseUrl")]
    base_url: Option<String>,
    #[serde(rename = "apiKey")]
    api_key: Option<String>,
    #[serde(rename = "clearApiKey")]
    clear_api_key: Option<String>,
    #[serde(rename = "proxyUrl")]
    proxy_url: Option<String>,
    headers: Option<String>,
    #[serde(rename = "makeActive")]
    make_active: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ActivateForm {
    model: Option<String>,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ModelForm {
    id: Option<String>,
    name: Option<String>,
    #[serde(rename = "contextWindow")]
    context_window: Option<String>,
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: Option<String>,
    #[serde(rename = "supportsImages")]
    supports_images: Option<String>,
    /// 添加动作的来源：`remote` 表示从「远程模型」弹窗里点的，
    /// 保存后要带着弹窗回跳，否则每加一个模型弹窗就没了。
    #[serde(default)]
    back: Option<String>,
    #[serde(default)]
    next: Option<String>,
}

fn parse_u64(text: Option<&str>, fallback: u64) -> u64 {
    text.and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn model_value(form: &ModelForm, model_id: &str) -> Value {
    json!({
        "id": model_id,
        "name": form
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(model_id),
        "contextWindow": parse_u64(form.context_window.as_deref(), 128_000),
        "maxOutputTokens": parse_u64(form.max_output_tokens.as_deref(), 8_192),
        // 未勾选的 checkbox 不会出现在表单里：只有显式传 "on/true/1" 才算支持。
        "supportsImages": form
            .supports_images
            .as_deref()
            .is_some_and(|value| matches!(value, "on" | "true" | "1")),
    })
}

pub async fn create_provider(
    State(state): State<AppState>,
    Query(query): Query<NextQuery>,
) -> Result<Response, AppError> {
    let id = format!("prov_{}", now_millis());
    let entry = json!({
        "id": id,
        "name": "新建供应商",
        "protocol": "openai_chat",
        "baseUrl": "https://api.openai.com/v1",
        "apiKey": "",
        "models": [{
            "id": "gpt-4o",
            "name": "GPT-4o",
            "contextWindow": 128000,
            "maxOutputTokens": 8192,
            "supportsImages": true
        }]
    });
    match state.agent.save_provider(&entry).await {
        Ok(_) => Ok(provider_back_with_next(
            &id,
            "connection",
            "provider_created",
            query.next.as_deref(),
        )),
        Err(error) => Ok(provider_failed_with_next(
            &id,
            "connection",
            "新建供应商失败",
            error,
            query.next.as_deref(),
        )),
    }
}

pub async fn save_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<NextQuery>,
    Form(form): Form<ProviderForm>,
) -> Result<Response, AppError> {
    let name = form.name.clone().unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return Ok(provider_failed_with_next(
            &id,
            "connection",
            "保存供应商失败",
            anyhow::anyhow!("供应商名称不能为空"),
            query.next.as_deref(),
        ));
    }
    let base_url = form.base_url.clone().unwrap_or_default().trim().to_string();
    if base_url.is_empty() {
        return Ok(provider_failed_with_next(
            &id,
            "connection",
            "保存供应商失败",
            anyhow::anyhow!("API 端点 (Base URL) 不能为空"),
            query.next.as_deref(),
        ));
    }
    let mut entry = provider_entry(&state, &id).await?;
    entry["name"] = json!(name);
    entry["baseUrl"] = json!(base_url);
    entry["protocol"] = json!(form
        .protocol
        .clone()
        .unwrap_or_else(|| "openai_chat".to_string()));
    if form.clear_api_key.is_some() {
        entry["apiKey"] = json!("");
    } else if let Some(key) = form.api_key.clone().filter(|key| !key.trim().is_empty()) {
        entry["apiKey"] = json!(key.trim());
    }
    let proxy_url = form
        .proxy_url
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    match entry.as_object_mut() {
        Some(object) if proxy_url.is_empty() => {
            object.remove("proxyUrl");
        }
        Some(_) => entry["proxyUrl"] = json!(proxy_url),
        None => {}
    }
    match parse_headers(&form.headers.clone().unwrap_or_default()) {
        Value::Null => {
            if let Some(object) = entry.as_object_mut() {
                object.remove("customHeaders");
            }
        }
        headers => entry["customHeaders"] = headers,
    }

    if let Err(error) = state.agent.save_provider(&entry).await {
        return Ok(provider_failed_with_next(
            &id,
            "connection",
            "保存供应商失败",
            error,
            query.next.as_deref(),
        ));
    }
    if form.make_active.is_some() {
        if let Err(error) = state.agent.set_active_provider(&id, None).await {
            return Ok(provider_failed_with_next(
                &id,
                "connection",
                "设为激活供应商失败",
                error,
                query.next.as_deref(),
            ));
        }
        return Ok(provider_back_with_next(
            &id,
            "connection",
            "provider_saved_activated",
            query.next.as_deref(),
        ));
    }
    Ok(provider_back_with_next(
        &id,
        "connection",
        "provider_saved",
        query.next.as_deref(),
    ))
}

pub async fn activate_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ActivateForm>,
) -> Result<Response, AppError> {
    match state
        .agent
        .set_active_provider(&id, form.model.as_deref())
        .await
    {
        Ok(_) => Ok(provider_back_with_next(
            &id,
            "models",
            "provider_activated",
            form.next.as_deref(),
        )),
        Err(error) => Ok(provider_failed_with_next(
            &id,
            "models",
            "切换激活供应商失败",
            error,
            form.next.as_deref(),
        )),
    }
}

pub async fn delete_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<NextQuery>,
) -> Result<Response, AppError> {
    match state.agent.delete_provider(&id).await {
        Ok(_) => {
            if let Some(base) = safe_plugin_next(query.next.as_deref()) {
                Ok(redirect(&append_query(
                    base,
                    &[("notice", "provider_deleted")],
                )))
            } else {
                Ok(redirect("/providers?notice=provider_deleted"))
            }
        }
        Err(error) => Ok(provider_failed_with_next(
            &id,
            "connection",
            "删除供应商失败",
            error,
            query.next.as_deref(),
        )),
    }
}

pub async fn add_provider_model(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ModelForm>,
) -> Result<Response, AppError> {
    let model_id = form.id.clone().unwrap_or_default().trim().to_string();
    if model_id.is_empty() {
        return Ok(provider_failed_with_next(
            &id,
            "models",
            "添加模型失败",
            anyhow::anyhow!("模型 ID 不能为空"),
            form.next.as_deref(),
        ));
    }
    let mut entry = provider_entry(&state, &id).await?;
    let models = entry
        .as_object_mut()
        .map(|object| {
            object
                .entry("models")
                .or_insert_with(|| Value::Array(Vec::new()))
        })
        .and_then(Value::as_array_mut);
    let Some(models) = models else {
        return Ok(provider_failed_with_next(
            &id,
            "models",
            "添加模型失败",
            anyhow::anyhow!("供应商数据格式异常（models 不是数组）"),
            form.next.as_deref(),
        ));
    };
    if models.iter().any(|model| str_at(model, "id") == model_id) {
        return Ok(provider_failed_with_next(
            &id,
            "models",
            "添加模型失败",
            anyhow::anyhow!("模型 {model_id} 已存在"),
            form.next.as_deref(),
        ));
    }
    models.push(model_value(&form, &model_id));
    match state.agent.save_provider(&entry).await {
        Ok(_) => Ok(provider_back_from_with_next(
            &id,
            "models",
            "model_added",
            form.back.as_deref(),
            form.next.as_deref(),
        )),
        Err(error) => Ok(provider_failed_with_next(
            &id,
            "models",
            "添加模型失败",
            error,
            form.next.as_deref(),
        )),
    }
}

pub async fn update_provider_model(
    State(state): State<AppState>,
    Path((id, index)): Path<(String, usize)>,
    Form(form): Form<ModelForm>,
) -> Result<Response, AppError> {
    let model_id = form.id.clone().unwrap_or_default().trim().to_string();
    if model_id.is_empty() {
        return Ok(provider_failed(
            &id,
            "models",
            "保存模型失败",
            anyhow::anyhow!("模型 ID 不能为空"),
        ));
    }
    let mut entry = provider_entry(&state, &id).await?;
    let Some(models) = entry.get_mut("models").and_then(Value::as_array_mut) else {
        return Ok(provider_failed(
            &id,
            "models",
            "保存模型失败",
            anyhow::anyhow!("供应商没有模型列表"),
        ));
    };
    if index >= models.len() {
        return Ok(provider_failed(
            &id,
            "models",
            "保存模型失败",
            anyhow::anyhow!("模型下标越界（列表可能已被其他客户端改动，请刷新）"),
        ));
    }
    if models
        .iter()
        .enumerate()
        .any(|(position, model)| position != index && str_at(model, "id") == model_id)
    {
        return Ok(provider_failed(
            &id,
            "models",
            "保存模型失败",
            anyhow::anyhow!("模型 ID {model_id} 与其他模型重复"),
        ));
    }
    let was_current = str_at(&models[index], "id") == {
        let config = state.agent.config().await.unwrap_or(Value::Null);
        config_str(&config, "model")
    };
    models[index] = model_value(&form, &model_id);
    if let Err(error) = state.agent.save_provider(&entry).await {
        return Ok(provider_failed(&id, "models", "保存模型失败", error));
    }
    // 改的正好是当前生效模型：同步把激活模型指到新 id，否则界面显示的还是旧名字。
    if was_current {
        if let Err(error) = state.agent.set_active_provider(&id, Some(&model_id)).await {
            return Ok(provider_failed(&id, "models", "同步当前模型失败", error));
        }
    }
    Ok(provider_back(&id, "models", "model_saved"))
}

pub async fn delete_provider_model(
    State(state): State<AppState>,
    Path((id, index)): Path<(String, usize)>,
) -> Result<Response, AppError> {
    let mut entry = provider_entry(&state, &id).await?;
    let (removed_id, fallback_id) = {
        let Some(models) = entry.get_mut("models").and_then(Value::as_array_mut) else {
            return Ok(provider_failed(
                &id,
                "models",
                "删除模型失败",
                anyhow::anyhow!("供应商没有模型列表"),
            ));
        };
        if models.len() <= 1 {
            return Ok(provider_failed(
                &id,
                "models",
                "删除模型失败",
                anyhow::anyhow!("每个供应商至少保留一个模型"),
            ));
        }
        if index >= models.len() {
            return Ok(provider_failed(
                &id,
                "models",
                "删除模型失败",
                anyhow::anyhow!("模型下标越界（列表可能已被其他客户端改动，请刷新）"),
            ));
        }
        let removed_id = str_at(&models[index], "id").to_string();
        models.remove(index);
        let fallback_id = models
            .first()
            .map(|model| str_at(model, "id").to_string())
            .unwrap_or_default();
        (removed_id, fallback_id)
    };
    if let Err(error) = state.agent.save_provider(&entry).await {
        return Ok(provider_failed(&id, "models", "删除模型失败", error));
    }
    // 删掉的可能正是当前生效模型：不把激活模型挪走，配置里就会留下一个不存在的 id，
    // 界面顶部会一直显示一个列表里找不到的"当前模型"。
    let current = {
        let config = state.agent.config().await.unwrap_or(Value::Null);
        config_str(&config, "model")
    };
    let is_active_provider = {
        let values = state.agent.providers().await.unwrap_or(Value::Null);
        values
            .get("activeProviderId")
            .and_then(Value::as_str)
            .is_some_and(|active| active == id)
    };
    if is_active_provider
        && !removed_id.is_empty()
        && removed_id == current
        && !fallback_id.is_empty()
    {
        if let Err(error) = state
            .agent
            .set_active_provider(&id, Some(&fallback_id))
            .await
        {
            return Ok(provider_failed(
                &id,
                "models",
                "删除模型失败（同步当前模型时出错）",
                error,
            ));
        }
    }
    Ok(provider_back(&id, "models", "model_deleted"))
}

/// 把表单当前值补成"完整连接参数"：留空的字段回落到已存值。
fn resolve_connection(entry: &Value, form: &ProviderForm) -> Value {
    let protocol = form
        .protocol
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let stored = str_at(entry, "protocol");
            if stored.is_empty() {
                "openai_chat".to_string()
            } else {
                stored.to_string()
            }
        });
    let base_url = form
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| str_at(entry, "baseUrl").to_string());
    let api_key = form
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| str_at(entry, "apiKey").to_string());
    let proxy_url = form
        .proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| str_at(entry, "proxyUrl").to_string());
    let headers = match parse_headers(&form.headers.clone().unwrap_or_default()) {
        Value::Null => entry.get("customHeaders").cloned().unwrap_or(Value::Null),
        parsed => parsed,
    };
    json!({
        "protocol": protocol,
        "baseUrl": base_url,
        "apiKey": api_key,
        "customHeaders": headers,
        "proxyUrl": if proxy_url.is_empty() { Value::Null } else { json!(proxy_url) },
    })
}

fn fragment(html: String) -> Response {
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        Html(html),
    )
        .into_response()
}

/// `POST /providers/{id}/models/fetch`：真的向端点要 `/models`，返回可选列表片段。
pub async fn fetch_provider_models(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ProviderForm>,
) -> Result<Response, AppError> {
    let entry = provider_entry(&state, &id).await?;
    let connection = resolve_connection(&entry, &form);
    let existing: Vec<String> = entry
        .get("models")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .map(|model| str_at(model, "id").to_string())
                .collect()
        })
        .unwrap_or_default();
    match state.agent.fetch_provider_models(connection).await {
        Ok(value) => Ok(fragment(remote_models_html(&value, &id, &existing))),
        Err(error) => Ok(fragment(format!(
            "<p class=\"text-xs text-danger-ink\">获取模型失败：{}</p>",
            escape_html(&public_reason(&error))
        ))),
    }
}

/// 远程模型列表（弹窗内容）：按厂商分组 + 已添加标记。
///
/// 远端动辄返回 90+ 个模型，平铺一列没人翻得动，所以：
/// - 按 `vendor/model` 的厂商前缀分组，组头带数量，大组在前；
/// - 行里只留 id 与一个按钮，上下文/输出长度收进 `title`；
/// - 已在列表里的排到组尾并置灰，避免"点了没反应"。
fn remote_models_html(value: &Value, provider_id: &str, existing: &[String]) -> String {
    let models: Vec<&Value> = array_at(value, "models")
        .iter()
        .filter(|model| !str_at(model, "id").is_empty())
        .collect();
    if models.is_empty() {
        return "<p class=\"ada-remote__empty\">端点已响应，但没有返回可用模型；可以用「新增模型」手动添加。</p>"
            .to_string();
    }
    let mut groups: std::collections::BTreeMap<&str, Vec<&Value>> =
        std::collections::BTreeMap::new();
    for model in &models {
        let id = str_at(model, "id");
        let vendor = id.split('/').next().unwrap_or(id);
        groups.entry(vendor).or_default().push(model);
    }
    let mut ordered: Vec<(&str, Vec<&Value>)> = groups.into_iter().collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));
    // 只有一个模型的厂商不单独开头：94 个模型里三十多个是"独苗"，
    // 每个都占一行组头，列表反而被切得看不出结构。它们统一收进「其它」。
    let mut sections: Vec<(&str, Vec<&Value>)> = Vec::new();
    let mut singletons: Vec<&Value> = Vec::new();
    for (vendor, list) in ordered {
        if list.len() >= 2 {
            sections.push((vendor, list));
        } else {
            singletons.extend(list);
        }
    }
    if !singletons.is_empty() {
        singletons.sort_by(|a, b| str_at(a, "id").cmp(str_at(b, "id")));
        sections.push(("其它", singletons));
    }

    let mut out = String::new();
    out.push_str(&format!(
        "<div class=\"ada-remote__summary\">远端返回 <b>{}</b> 个模型（按厂商分组）</div>",
        models.len()
    ));
    out.push_str("<div class=\"ada-remote__groups\">");
    for (vendor, mut list) in sections {
        let is_added = |model: &Value| existing.iter().any(|known| known == str_at(model, "id"));
        list.sort_by(|a, b| {
            is_added(a)
                .cmp(&is_added(b))
                .then_with(|| str_at(a, "id").cmp(str_at(b, "id")))
        });
        out.push_str(&format!(
            "<section class=\"ada-remote__group\"><h3 class=\"ada-remote__vendor\"><span>{}</span><span class=\"ada-remote__badge\">{}</span></h3><ul class=\"ada-remote__rows\">",
            escape_html(vendor),
            list.len()
        ));
        for model in list {
            out.push_str(&remote_model_row(model, provider_id, existing));
        }
        out.push_str("</ul></section>");
    }
    out.push_str("</div>");
    out
}

fn remote_model_row(model: &Value, provider_id: &str, existing: &[String]) -> String {
    let model_id = str_at(model, "id");
    let raw_name = str_at(model, "name");
    let name = if raw_name.is_empty() {
        model_id
    } else {
        raw_name
    };
    let context_window = model_text(model, "contextWindow");
    let max_output_tokens = model_text(model, "maxOutputTokens");
    let mut meta = Vec::new();
    if !context_window.is_empty() {
        meta.push(format!("上下文 {context_window}"));
    }
    if !max_output_tokens.is_empty() {
        meta.push(format!("最大输出 {max_output_tokens}"));
    }
    let already = existing.iter().any(|known| known == model_id);
    let action = if already {
        "<span class=\"ada-remote__added\">已添加</span>".to_string()
    } else {
        let mut fields = String::new();
        for (field, value) in [
            ("id", model_id),
            ("name", name),
            ("contextWindow", context_window.as_str()),
            ("maxOutputTokens", max_output_tokens.as_str()),
        ] {
            if value.is_empty() {
                continue;
            }
            fields.push_str(&format!(
                "<input type=\"hidden\" name=\"{field}\" value=\"{}\">",
                escape_html(value)
            ));
        }
        format!(
            "<form method=\"post\" action=\"/providers/{provider}/models\">{fields}<input type=\"hidden\" name=\"back\" value=\"remote\"><button class=\"btn btn-secondary btn-xs\" type=\"submit\">添加</button></form>",
            provider = encode_query(provider_id)
        )
    };
    format!(
        "<li class=\"ada-remote__row{added}\" data-remote-row data-name=\"{key}\"><span class=\"ada-remote__id\" title=\"{title}\">{label}</span>{action}</li>",
        added = if already { " is-added" } else { "" },
        key = escape_html(&model_id.to_lowercase()),
        title = escape_html(&meta.join(" · ")),
        label = escape_html(model_id),
    )
}

/// `POST /providers/{id}/test`：调 `config.checkProvider`，返回结果片段。
///
/// 后端目前只做参数校验（不发起真实请求），所以文案如实照录服务端返回，
/// 并由页面注明"想验证可达性请用获取远程模型"。
pub async fn test_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ProviderForm>,
) -> Result<Response, AppError> {
    let entry = provider_entry(&state, &id).await?;
    let connection = resolve_connection(&entry, &form);
    let model = entry
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.first())
        .map(|model| str_at(model, "id").to_string())
        .unwrap_or_default();
    let mut params = connection;
    params["model"] = json!(model);
    match state.agent.check_provider(params).await {
        Ok(value) => {
            let message = str_at(&value, "message");
            let message = if message.is_empty() {
                "配置校验通过"
            } else {
                message
            };
            Ok(fragment(format!(
                "<p class=\"text-xs text-success-ink\">✓ {}</p>",
                escape_html(message)
            )))
        }
        Err(error) => Ok(fragment(format!(
            "<p class=\"text-xs text-danger-ink\">✗ {}</p>",
            escape_html(&public_reason(&error))
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tab_query_param_only_knows_the_two_panels() {
        assert_eq!(normalize_tab(None), "connection");
        assert_eq!(normalize_tab(Some("  ")), "connection");
        assert_eq!(normalize_tab(Some("models")), "models");
        // 手改地址栏写错时回落到默认页签，而不是渲染一块没有选中项的面板。
        assert_eq!(normalize_tab(Some("bogus")), "connection");
    }

    #[test]
    fn remote_models_are_grouped_by_vendor_and_mark_the_added_ones() {
        let value = json!({"models": [
            {"id": "Qwen/Qwen3-8-Max", "contextWindow": 262144},
            {"id": "Qwen/Qwen3-7-Plus"},
            {"id": "MiniMaxAI/MiniMax-M2.5"},
            {"id": "bare-model"}
        ]});
        let html = remote_models_html(&value, "prov 1", &["Qwen/Qwen3-7-Plus".to_string()]);
        // Qwen 有两个模型 → 单独成组；另外两个是"独苗" → 合并进「其它」。
        // 注意别直接数 `ada-remote__group`：容器 `ada-remote__groups` 也会命中。
        assert_eq!(
            html.matches("class=\"ada-remote__group\"").count(),
            2,
            "{html}"
        );
        assert!(html.contains(">其它<"), "独苗厂商应合并成「其它」: {html}");
        // 已在列表里：只给「已添加」，不再给按钮
        assert!(html.contains("ada-remote__added"), "{html}");
        assert_eq!(html.matches(">添加</button>").count(), 3, "{html}");
        // 已添加的排在组内末尾
        let fresh = html.find("Qwen/Qwen3-8-Max").expect("fresh model");
        let added = html.find("Qwen/Qwen3-7-Plus").expect("added model");
        assert!(fresh < added, "已添加的应排在后面: {html}");
        // 元信息收进 title，不占行
        assert!(html.contains("上下文 262144"), "{html}");
    }

    #[test]
    fn back_from_remote_reopens_the_dialog() {
        let response = provider_back_from("p1", "models", "model_added", Some("remote"));
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/providers?provider=p1&tab=models&notice=model_added&dialog=remote-models"
        );
        let plain = provider_back_from("p1", "models", "model_added", None);
        assert_eq!(
            plain.headers().get(header::LOCATION).unwrap(),
            "/providers?provider=p1&tab=models&notice=model_added"
        );
    }

    #[test]
    fn provider_back_keeps_the_calling_tab() {
        let response = provider_back("prov 1", "models", "model added");
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/providers?provider=prov+1&tab=models&notice=model+added"
        );
    }

    #[test]
    fn provider_admin_projection_never_leaks_api_key() {
        let snapshot = json!({
            "activeProviderId": "p1",
            "config": { "model": "m1", "baseUrl": "https://example.test/v1" },
            "providers": [{
                "id": "p1", "name": "Provider", "protocol": "openai_chat",
                "baseUrl": "https://example.test/v1", "apiKey": "sk-secret-value-1234",
                "models": [{ "id": "m1", "name": "Model", "contextWindow": 1000 }]
            }]
        });
        let view = provider_admin_view(&snapshot, None, Some("models"));
        let text = serde_json::to_string(&view).unwrap();
        assert!(!text.contains("sk-secret-value-1234"), "{text}");
        assert!(text.contains("apiKeyHint"), "{text}");
        assert_eq!(view["selectedId"], "p1");
        assert_eq!(view["activeTab"], "models");
        assert_eq!(view["providers"][0]["models"][0]["current"], true);
    }

    #[test]
    fn api_key_is_masked_and_never_renderable_whole() {
        assert_eq!(mask_api_key(""), "");
        assert_eq!(mask_api_key("short"), "已配置");
        let masked = mask_api_key("sk-test-1234567890");
        assert_eq!(masked, "sk-t…7890");
        assert!(!masked.contains("123456"));
    }

    #[test]
    fn custom_headers_round_trip_as_text() {
        let value = json!({"customHeaders": {"x-b": "2", "x-a": "1"}});
        assert_eq!(headers_text(&value), "x-a: 1; x-b: 2");
        let parsed = parse_headers("x-a: 1; x-b: 2\nx-c: 3");
        assert_eq!(parsed["x-a"], json!("1"));
        assert_eq!(parsed["x-c"], json!("3"));
        // 没有合法 k: v 时给 Null，调用方据此把字段从实体里删掉（而不是写成空对象）。
        assert!(parse_headers("").is_null());
        assert!(parse_headers("no-colon-here").is_null());
        assert!(parse_headers("bad key: v").is_null());
    }

    #[test]
    fn model_form_falls_back_to_sane_defaults() {
        let form = ModelForm {
            id: Some("m1".to_string()),
            name: None,
            context_window: Some("64000".to_string()),
            max_output_tokens: None,
            supports_images: None,
            back: None,
            next: None,
        };
        let value = model_value(&form, "m1");
        assert_eq!(value["name"], json!("m1"));
        assert_eq!(value["contextWindow"], json!(64000));
        assert_eq!(value["maxOutputTokens"], json!(8192));
        // 未勾选的 checkbox 不会进表单：必须落成 false，而不是序列化失败/缺字段。
        assert_eq!(value["supportsImages"], json!(false));

        let checked = ModelForm {
            supports_images: Some("on".to_string()),
            context_window: Some("0".to_string()),
            ..ModelForm::default()
        };
        let value = model_value(&checked, "m2");
        assert_eq!(value["supportsImages"], json!(true));
        // 0 不是合法上下文长度 → 回落默认值。
        assert_eq!(value["contextWindow"], json!(128000));
    }
}




