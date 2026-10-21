//! 供应商与模型管理：Master-Detail 页面 + 全部动作端点。
//!
//! 与 desktop-ui 的差异只有一处、且是刻意的：**API Key 不回显**。桌面端把整串密钥
//! 灌进输入框（`type=password` 只是遮住显示），这里只回 `已配置 + sk-…7f3a`，
//! 保存时留空表示"保持原值"、勾选"清空密钥"才真的清空。

use axum::extract::{Form, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use serde_json::{json, Value};

use super::*;
use crate::ui::{
    Button, ButtonKind, ButtonVariant, IconButton, Modal, Render, Select, Size, TabItem, Tabs,
};

/// 详情区页签的 DOM id 前缀：`Tabs` 按它生成 `…-tab-{key}` / `…-panel-{key}`，
/// 模板里的两个面板 id 必须与它保持一致（改了这里也要改模板）。
const DETAIL_TABS: &str = "provider-detail";

/// 页签 key 只有两个，查询参数写错时回落到「供应商信息」（不报错、渲染可预期）。
fn normalize_tab(tab: Option<&str>) -> &'static str {
    match tab.map(str::trim) {
        Some("models") => "models",
        _ => "connection",
    }
}

// ── 供应商管理（对齐 desktop-ui 的 Master-Detail）────────────────────────────
//
// 与 desktop-ui 的差异只有一处、且是刻意的：**API Key 不回显**。桌面端把整串密钥
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

/// `customHeaders` → `k: v; k2: v2`（与 desktop-ui 的编辑框格式一致）。
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

fn provider_cards(values: &Value, active_id: &str, current_model: &str) -> Vec<ProviderCard> {
    array_at(values, "providers")
        .iter()
        .map(|value| {
            let id = str_at(value, "id").to_string();
            let active = !active_id.is_empty() && active_id == id;
            let api_key = str_at(value, "apiKey");
            let models: Vec<ProviderModelRow> = value
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
                    ProviderModelRow {
                        current: active && !current_model.is_empty() && model_id == current_model,
                        index,
                        id: model_id,
                        name,
                        context_window: model_text(model, "contextWindow"),
                        max_output_tokens: model_text(model, "maxOutputTokens"),
                        supports_images: model
                            .get("supportsImages")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                    }
                })
                .collect();
            ProviderCard {
                href: format!("/providers?provider={}", encode_query(&id)),
                id,
                name: str_at(value, "name").to_string(),
                protocol: {
                    let protocol = str_at(value, "protocol");
                    if protocol.is_empty() {
                        "openai_chat".to_string()
                    } else {
                        protocol.to_string()
                    }
                },
                protocol_label: protocol_label(str_at(value, "protocol")).to_string(),
                base_url: str_at(value, "baseUrl").to_string(),
                proxy_url: str_at(value, "proxyUrl").to_string(),
                headers: headers_text(value),
                api_key_set: !api_key.trim().is_empty(),
                api_key_hint: mask_api_key(api_key),
                model_count: models.len(),
                active,
                models,
                modals: String::new(),
                delete_form: String::new(),
            }
        })
        .collect()
}

/// 列表行右侧的「删除供应商」动作：整行是链接，删除必须另起一个表单（不能塞进 `<a>` 里）。
fn provider_delete_form(id: &str, name: &str, can_delete: bool) -> String {
    if !can_delete {
        return IconButton::new("i-trash", "至少保留一个供应商")
            .attrs("disabled")
            .render();
    }
    IconButton::new("i-trash", format!("删除供应商「{name}」"))
        .action(format!("/providers/{}/delete", encode_query(id)))
        .attrs(format!(
            "data-confirm=\"{}\"",
            escape_html(&format!("删除供应商「{name}」？该操作不可恢复。"))
        ))
        .render()
}

/// 模型编辑/新增弹窗：两者共用一张表单，只有 action、标题与预填值不同。
fn model_modal(provider_id: &str, model: Option<&ProviderModelRow>) -> String {
    let provider_id = encode_query(provider_id);
    let (modal_id, title, action, submit_label) = match model {
        Some(row) => (
            format!("model-{}", row.index),
            "编辑模型".to_string(),
            format!("/providers/{provider_id}/models/{}", row.index),
            "保存模型",
        ),
        None => (
            "model-new".to_string(),
            "新增模型".to_string(),
            format!("/providers/{provider_id}/models"),
            "添加到列表",
        ),
    };
    let (id_value, name, context_window, max_output, supports_images) = match model {
        Some(row) => (
            escape_html(&row.id),
            escape_html(&row.name),
            escape_html(&row.context_window),
            escape_html(&row.max_output_tokens),
            row.supports_images,
        ),
        None => (
            String::new(),
            String::new(),
            "128000".to_string(),
            "8192".to_string(),
            true,
        ),
    };
    let form = format!(
        r#"<form method="post" action="{action}" class="ada-modal__form">
  <div class="ada-form-grid">
    <label class="field"><span class="label">模型 ID</span>
      <input class="input font-mono" name="id" value="{id_value}" placeholder="gpt-4o-mini" required data-modal-autofocus>
    </label>
    <label class="field"><span class="label">显示名</span>
      <input class="input" name="name" value="{name}" placeholder="留空则用模型 ID">
    </label>
    <label class="field"><span class="label">上下文窗口</span>
      <input class="input" name="contextWindow" value="{context_window}" inputmode="numeric" placeholder="128000">
    </label>
    <label class="field"><span class="label">最大输出</span>
      <input class="input" name="maxOutputTokens" value="{max_output}" inputmode="numeric" placeholder="8192">
    </label>
  </div>
  <label class="checkbox-row"><input type="checkbox" name="supportsImages" value="on"{checked}> 支持图片输入</label>
  <div class="ada-modal__actions">{cancel}{submit}</div>
</form>"#,
        checked = if supports_images { " checked" } else { "" },
        cancel = Button::new("取消")
            .variant(ButtonVariant::Secondary)
            .size(Size::Sm)
            .kind(ButtonKind::Button)
            .attrs("data-modal-close")
            .render(),
        submit = Button::new(submit_label)
            .variant(ButtonVariant::Primary)
            .size(Size::Sm)
            .kind(ButtonKind::Submit)
            .render(),
    );
    Modal::new(modal_id, title).wide().body(form).render()
}

fn config_str(config: &Value, key: &str) -> String {
    config
        .get("saved")
        .and_then(|saved| saved.get(key))
        .and_then(Value::as_str)
        .or_else(|| config.get(key).and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

pub async fn providers(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let config = state.agent.config().await.unwrap_or(Value::Null);
    let values = state.agent.providers().await.unwrap_or(Value::Null);
    let active_id = values
        .get("activeProviderId")
        .and_then(Value::as_str)
        .or_else(|| config.get("activeProviderId").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let current_model = config_str(&config, "model");
    let cards = provider_cards(&values, &active_id, &current_model);
    let wanted = query
        .provider
        .clone()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| {
            if active_id.is_empty() {
                cards
                    .first()
                    .map(|card| card.id.clone())
                    .unwrap_or_default()
            } else {
                active_id.clone()
            }
        });
    let mut selected = cards
        .iter()
        .find(|card| card.id == wanted)
        .cloned()
        .or_else(|| cards.first().cloned());
    // 每个模型的编辑弹窗 + 一个"新增模型"弹窗（都是组件库渲染的 HTML）。
    let model_new_modal = model_modal(
        &selected
            .as_ref()
            .map(|card| card.id.clone())
            .unwrap_or_default(),
        None,
    );
    if let Some(card) = selected.as_mut() {
        let provider_id = card.id.clone();
        let modals: String = card
            .models
            .iter()
            .map(|model| model_modal(&provider_id, Some(model)))
            .collect();
        card.modals = modals;
    }
    let can_delete = cards.len() > 1;
    // 「删除供应商」放到列表行右侧：整行是链接，删除按钮必须自带表单。
    let mut cards = cards;
    for card in cards.iter_mut() {
        card.delete_form = provider_delete_form(&card.id, &card.name, can_delete);
    }
    let protocols = protocol_options();
    // 协议下拉走组件库的 Select（原生 <select> 的弹层不受控，深色主题下尤其扎眼）。
    let protocol_select = Select::new("protocol")
        .options(
            protocols
                .iter()
                .map(|option| (option.value.clone(), option.label.clone())),
        )
        .value(
            selected
                .as_ref()
                .map(|card| card.protocol.clone())
                .unwrap_or_default(),
        )
        .title("协议")
        .render();
    let active_tab = normalize_tab(query.tab.as_deref());
    // 页签导航是**真实链接**：没有 JS 时点一下就是整页刷新到该页签；地址里带着
    // provider 与 tab，刷新/收藏/从网关跳回来都落在同一处。JS 只把同页跳转变成就地切换。
    let tabs = match selected.as_ref() {
        Some(card) => Tabs::new(DETAIL_TABS, "供应商详情")
            .active(active_tab)
            .item(TabItem::new(
                "connection",
                "供应商信息",
                format!(
                    "/providers?provider={}&tab=connection",
                    encode_query(&card.id)
                ),
            ))
            .item(
                TabItem::new(
                    "models",
                    "模型列表",
                    format!("/providers?provider={}&tab=models", encode_query(&card.id)),
                )
                .count(card.model_count),
            )
            .render(),
        None => String::new(),
    };
    let page = ProvidersPage {
        base: base(&state, "供应商", "providers", flash_from_query(&query)),
        providers: cards,
        selected,
        current_model,
        current_base_url: config_str(&config, "baseUrl"),
        protocol_select,
        tabs,
        active_tab: active_tab.to_string(),
        model_new_modal,
    };
    Ok(render(&page)?.into_response())
}

/// 读一条供应商实体：**所有动作都先读整条、改字段、整体写回**。
///
/// 后端 `provider.save` 是"存在即替换"，只发改动的字段会把模型列表写成空。
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

pub async fn create_provider(State(state): State<AppState>) -> Result<Response, AppError> {
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
        Ok(_) => Ok(provider_back(&id, "connection", "provider_created")),
        Err(error) => Ok(provider_failed(&id, "connection", "新建供应商失败", error)),
    }
}

pub async fn save_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ProviderForm>,
) -> Result<Response, AppError> {
    let name = form.name.unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return Ok(provider_failed(
            &id,
            "connection",
            "保存供应商失败",
            anyhow::anyhow!("供应商名称不能为空"),
        ));
    }
    let base_url = form.base_url.unwrap_or_default().trim().to_string();
    if base_url.is_empty() {
        return Ok(provider_failed(
            &id,
            "connection",
            "保存供应商失败",
            anyhow::anyhow!("API 端点 (Base URL) 不能为空"),
        ));
    }
    let mut entry = provider_entry(&state, &id).await?;
    entry["name"] = json!(name);
    entry["baseUrl"] = json!(base_url);
    entry["protocol"] = json!(form.protocol.unwrap_or_else(|| "openai_chat".to_string()));
    if form.clear_api_key.is_some() {
        entry["apiKey"] = json!("");
    } else if let Some(key) = form.api_key.filter(|key| !key.trim().is_empty()) {
        entry["apiKey"] = json!(key.trim());
    }
    let proxy_url = form.proxy_url.unwrap_or_default().trim().to_string();
    match entry.as_object_mut() {
        Some(object) if proxy_url.is_empty() => {
            object.remove("proxyUrl");
        }
        Some(_) => entry["proxyUrl"] = json!(proxy_url),
        None => {}
    }
    match parse_headers(&form.headers.unwrap_or_default()) {
        Value::Null => {
            if let Some(object) = entry.as_object_mut() {
                object.remove("customHeaders");
            }
        }
        headers => entry["customHeaders"] = headers,
    }

    if let Err(error) = state.agent.save_provider(&entry).await {
        return Ok(provider_failed(&id, "connection", "保存供应商失败", error));
    }
    if form.make_active.is_some() {
        if let Err(error) = state.agent.set_active_provider(&id, None).await {
            return Ok(provider_failed(&id, "connection", "设为激活供应商失败", error));
        }
        return Ok(provider_back(&id, "connection", "provider_saved_activated"));
    }
    Ok(provider_back(&id, "connection", "provider_saved"))
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
        Ok(_) => Ok(provider_back(&id, "models", "provider_activated")),
        Err(error) => Ok(provider_failed(&id, "models", "切换激活供应商失败", error)),
    }
}

pub async fn delete_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    match state.agent.delete_provider(&id).await {
        Ok(_) => Ok(redirect("/providers?notice=provider_deleted")),
        Err(error) => Ok(provider_failed(&id, "connection", "删除供应商失败", error)),
    }
}

pub async fn add_provider_model(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ModelForm>,
) -> Result<Response, AppError> {
    let model_id = form.id.clone().unwrap_or_default().trim().to_string();
    if model_id.is_empty() {
        return Ok(provider_failed(
            &id,
            "models",
            "添加模型失败",
            anyhow::anyhow!("模型 ID 不能为空"),
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
        return Ok(provider_failed(
            &id,
            "models",
            "添加模型失败",
            anyhow::anyhow!("供应商数据格式异常（models 不是数组）"),
        ));
    };
    if models.iter().any(|model| str_at(model, "id") == model_id) {
        return Ok(provider_failed(
            &id,
            "models",
            "添加模型失败",
            anyhow::anyhow!("模型 {model_id} 已存在"),
        ));
    }
    models.push(model_value(&form, &model_id));
    match state.agent.save_provider(&entry).await {
        Ok(_) => Ok(provider_back(&id, "models", "model_added")),
        Err(error) => Ok(provider_failed(&id, "models", "添加模型失败", error)),
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

fn remote_models_html(value: &Value, provider_id: &str, existing: &[String]) -> String {
    let models = array_at(value, "models");
    if models.is_empty() {
        return "<p class=\"text-xs text-faint\">端点已响应，但没有返回可用模型；可以在下面手动添加。</p>"
            .to_string();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "<p class=\"text-xs text-muted\">远端返回 {} 个模型，点“添加”加入当前供应商：</p>",
        models.len()
    ));
    out.push_str("<div class=\"mt-2 max-h-64 space-y-1 overflow-y-auto\">");
    for model in models {
        let model_id = str_at(model, "id");
        if model_id.is_empty() {
            continue;
        }
        let already = existing.iter().any(|known| known == model_id);
        let name = {
            let name = str_at(model, "name");
            if name.is_empty() {
                model_id.to_string()
            } else {
                name.to_string()
            }
        };
        let context_window = model_text(model, "contextWindow");
        let max_output_tokens = model_text(model, "maxOutputTokens");
        if already {
            out.push_str(&format!(
                "<div class=\"flex flex-wrap items-center justify-between gap-2 rounded-lg border border-line bg-surface px-2.5 py-1.5\">\
                   <span class=\"font-mono text-[11px] text-muted\">{}</span>\
                   <span class=\"badge badge-neutral\">已在列表</span></div>",
                escape_html(model_id)
            ));
            continue;
        }
        out.push_str(&format!(
            r#"<form method="post" action="/providers/{provider}/models" class="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-line bg-surface px-2.5 py-1.5">"#,
            provider = encode_query(provider_id)
        ));
        for (field, value) in [
            ("id", model_id),
            ("name", name.as_str()),
            ("contextWindow", context_window.as_str()),
            ("maxOutputTokens", max_output_tokens.as_str()),
        ] {
            if value.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "<input type=\"hidden\" name=\"{field}\" value=\"{}\">",
                escape_html(value)
            ));
        }
        out.push_str(
            "<input type=\"hidden\" name=\"supportsImages\" value=\"on\">\
             <span class=\"font-mono text-[11px] text-ink\">",
        );
        out.push_str(&escape_html(model_id));
        out.push_str("</span><button class=\"btn btn-secondary btn-xs\" type=\"submit\">添加</button></form>");
    }
    out.push_str("</div>");
    out
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
    fn provider_back_keeps_the_calling_tab() {
        let response = provider_back("prov 1", "models", "model added");
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/providers?provider=prov+1&tab=models&notice=model+added"
        );
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
