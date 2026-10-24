//! 扩展页：前端插件（导航/页面/插槽）与 agent 插件的查看与启停。

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use super::*;
use crate::plugins::ApiMethod;

fn frontend_rows(state: &AppState) -> Vec<FrontendPluginRow> {
    state
        .frontend
        .list()
        .into_iter()
        .map(|plugin| FrontendPluginRow {
            id: plugin.id,
            name: plugin.name,
            version: plugin.version,
            description: plugin.description,
            nav_count: plugin.nav.len(),
            page_count: plugin.pages.len(),
            slot_count: plugin.slots.len(),
            first_page: plugin.pages.first().map(|page| page.slug.clone()),
            has_dashboard: plugin.dashboard.is_some(),
            has_assets: plugin.assets_dir.is_some(),
            enabled: plugin.enabled,
            status: plugin.status.as_str().to_string(),
            errors: plugin.errors,
        })
        .collect()
}

fn agent_rows(value: &Value) -> Vec<AgentPluginRow> {
    array_at(value, "plugins")
        .iter()
        .map(|plugin| AgentPluginRow {
            id: str_at(plugin, "id").to_string(),
            name: str_at(plugin, "name").to_string(),
            scope: str_at(plugin, "scope").to_string(),
            enabled: plugin
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            status: str_at(plugin, "status").to_string(),
            tools: array_at(plugin, "tools").len(),
            error: str_at(plugin, "error").to_string(),
        })
        .collect()
}

pub async fn extensions(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let frontend = frontend_rows(&state);
    let agent_value = state.agent.plugins().await.unwrap_or(Value::Null);
    let agent = agent_rows(&agent_value);
    let page = ExtensionsPage {
        base: base(&state, "扩展", "extensions", flash_from_query(&query)).await,
        frontend_count: frontend.len(),
        agent_count: agent.len(),
        frontend,
        agent,
    };
    Ok(render(&page)?.into_response())
}

pub async fn reload_frontend(State(state): State<AppState>) -> Result<Response, AppError> {
    let report = match state.frontend.reload() {
        Ok(report) => report,
        Err(error) => {
            tracing::warn!("重载前端插件失败: {error:#}");
            let message = format!("重载前端插件失败：{}", public_reason(&error));
            return Ok(redirect(&format!(
                "/extensions?error={}",
                encode_query(&message)
            )));
        }
    };
    Ok(redirect(&format!(
        "/extensions?notice=frontend_reloaded&count={}&failed={}",
        report.total,
        report.failed.len()
    )))
}

/// 启用插件（热插拔：装载 → 挂载 → 立刻出现在页面上）。
pub async fn enable_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> Result<Response, AppError> {
    if let Err(error) = state.frontend.set_enabled(&plugin_id, true) {
        tracing::warn!("启用前端插件失败: {error:#}");
        let message = format!("启用插件失败：{}", public_reason(&error));
        return Ok(redirect(&format!(
            "/extensions?error={}",
            encode_query(&message)
        )));
    }
    Ok(redirect(&format!(
        "/extensions?notice=plugin_enabled&id={plugin_id}"
    )))
}

/// 停用插件（卸载：贡献消失，界面回到默认实现）。
pub async fn disable_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> Result<Response, AppError> {
    if let Err(error) = state.frontend.set_enabled(&plugin_id, false) {
        tracing::warn!("停用前端插件失败: {error:#}");
        let message = format!("停用插件失败：{}", public_reason(&error));
        return Ok(redirect(&format!(
            "/extensions?error={}",
            encode_query(&message)
        )));
    }
    Ok(redirect(&format!(
        "/extensions?notice=plugin_disabled&id={plugin_id}"
    )))
}

/// 插件状态快照：客户端与服务端看到同一份事实。
pub async fn plugins_api(State(state): State<AppState>) -> Result<Response, AppError> {
    let outputs = crate::plugins::render_slots_off_thread(
        std::sync::Arc::clone(&state.frontend),
        crate::plugins::Slot::all(),
        super::render_context(&state, "extensions"),
    )
    .await;
    let slots: Vec<serde_json::Value> = crate::plugins::Slot::all()
        .iter()
        .zip(outputs)
        .map(|(slot, output)| {
            serde_json::json!({
                "slot": slot.as_str(),
                "contributors": output.contributors,
                "failed": output.failed,
            })
        })
        .collect();
    let body = serde_json::json!({
        "plugins": state.frontend.snapshot(),
        "slots": slots,
    });
    Ok((StatusCode::OK, axum::Json(body)).into_response())
}

pub async fn reload_agent(State(state): State<AppState>) -> Result<Response, AppError> {
    if let Err(error) = state.agent.reload_agent_plugins().await {
        tracing::warn!("agent 重载插件失败: {error:#}");
        let message = format!("agent 重载插件失败：{}", public_reason(&error));
        return Ok(redirect(&format!(
            "/extensions?error={}",
            encode_query(&message)
        )));
    }
    Ok(redirect("/extensions?notice=agent_reloaded"))
}

pub async fn plugin_page(
    State(state): State<AppState>,
    Path((plugin_id, slug)): Path<(String, String)>,
    Query(query): Query<PageQuery>,
) -> Result<Response, AppError> {
    let plugin = state.frontend.find(&plugin_id).ok_or_else(|| {
        AppError::new(
            StatusCode::NOT_FOUND,
            format!("前端插件不存在: {plugin_id}"),
        )
    })?;
    // 插件页可以读 `props.data.tokens`（当前会话上下文用量）。网关不可达时退化成
    // 空快照——插件拿到 percent = null 自己显示"无数据"，页面不要因此 500。
    let snapshot = state.agent.snapshot().await.unwrap_or(Value::Null);
    let mut context = super::render_context_with_tokens(&state, "extensions", &snapshot, None);
    context.route.path = format!("/ext/{plugin_id}/{slug}");
    let mut route_query = Vec::new();
    if let Some(provider) = query.provider.as_deref().filter(|value| !value.is_empty()) {
        route_query.push(format!("provider={}", encode_query(provider)));
    }
    if let Some(tab) = query.tab.as_deref().filter(|value| !value.is_empty()) {
        route_query.push(format!("tab={}", encode_query(tab)));
    }
    context.route.query = route_query.join("&");
    if plugin
        .capabilities
        .iter()
        .any(|capability| capability == "providers.read")
    {
        // snapshot 里的 providers 是精简投影，连接表单需要完整字段（proxy/headers）；
        // 这里额外取 providers RPC，密钥仍只在宿主侧，投影时只输出掩码。
        let providers = state.agent.providers().await.unwrap_or(Value::Null);
        let config = state.agent.config().await.unwrap_or(Value::Null);
        let source = serde_json::json!({
            "activeProviderId": providers
                .get("activeProviderId")
                .cloned()
                .or_else(|| config.get("activeProviderId").cloned())
                .unwrap_or(Value::Null),
            "config": config,
            "providers": providers.get("providers").cloned().unwrap_or(Value::Array(Vec::new())),
        });
        let provider_admin = super::providers::provider_admin_view(
            &source,
            query.provider.as_deref(),
            query.tab.as_deref(),
        );
        if let Some(data) = context.data.as_object_mut() {
            data.insert("providerAdmin".to_string(), provider_admin);
        }
    }
    let (page_title, html) = crate::plugins::read_page_off_thread(
        std::sync::Arc::clone(&state.frontend),
        plugin_id.clone(),
        slug.clone(),
        context,
    )
    .await
    .map_err(|error| AppError::new(StatusCode::NOT_FOUND, format!("{error:#}")))?;
    let page = PluginPage {
        base: base(
            &state,
            &format!("{} · {}", plugin.name, page_title),
            // 侧栏按它点亮「前端插件」链接：ext:{id} 精确到插件，而不是粗到
            // "extensions"（那样亮的是「插件」项，用户点的是哪个插件反而没反馈）
            &format!("ext:{}", plugin.id),
            flash_from_query(&query),
        )
        .await,
        plugin_id: plugin.id,
        plugin_name: plugin.name,
        page_title,
        html,
    };
    Ok(render(&page)?.into_response())
}

/// 插件 API 的请求体上限：JSON 数据面不是文件传输面，超限直接 413。
const API_BODY_LIMIT: usize = 256 * 1024;

fn api_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        )],
        serde_json::json!({ "error": message }).to_string(),
    )
        .into_response()
}

/// `GET/POST /ext/{id}/api/{path}`：插件自带的 JSON API（manifest `api` 声明）。
///
/// 动态扩展的口子是**一条静态分发路由**，而不是重建 Router：axum Router 在 serve
/// 之后不可变，而插件注册表本来就是热换的——端点查找走注册表，`/extensions`
/// 点重载后新端点立即生效（页面 `/ext/{id}/{slug}` 是同一个模式）。
///
/// 请求信息经 `props.api` 进入模板（method / path / query / body，POST 体必须是
/// JSON 且不超限）；响应就是入口函数的返回值（`tsx::render_json`，不走 JSX）。
/// 安全面与页面/插槽同一条规则：插件停用或加载失败，端点立即消失；POST 过
/// guard 的同源校验。
pub async fn plugin_api(
    State(state): State<AppState>,
    method: axum::http::Method,
    Path((plugin_id, path)): Path<(String, String)>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    body: axum::body::Bytes,
) -> Response {
    let api_method = match method {
        axum::http::Method::GET => ApiMethod::Get,
        axum::http::Method::POST => ApiMethod::Post,
        _ => return api_error(StatusCode::METHOD_NOT_ALLOWED, "插件 API 只支持 GET / POST"),
    };
    let Some((entry, template)) = state.frontend.find_api(&plugin_id, api_method, &path) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "插件 API 端点不存在（或插件已停用/加载失败）",
        );
    };

    let body_value = if api_method == ApiMethod::Post {
        if body.len() > API_BODY_LIMIT {
            return api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!("请求体超过 {} KB 上限", API_BODY_LIMIT / 1024),
            );
        }
        match serde_json::from_slice::<Value>(&body) {
            Ok(value) => value,
            Err(error) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    &format!("请求体不是合法 JSON: {error}"),
                )
            }
        }
    } else {
        Value::Null
    };

    let Some(record) = state.frontend.find(&plugin_id) else {
        return api_error(StatusCode::NOT_FOUND, "插件不存在");
    };
    let mut cx = super::render_context(&state, "extensions");
    cx.route.path = format!("/ext/{plugin_id}/api/{}", entry.path);
    let query_value = Value::Object(
        query
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    );
    let props = crate::plugins::api_props(&record, &cx, &entry, query_value, body_value);

    let rendered = tokio::task::spawn_blocking(move || crate::tsx::render_json(&template, &props))
        .await
        .map_err(|error| anyhow::anyhow!("API 任务失败: {error}"))
        .and_then(|inner| inner.map_err(|error| anyhow::anyhow!(error)));
    match rendered {
        Ok(value) => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json; charset=utf-8"),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
            serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
        )
            .into_response(),
        Err(error) => {
            tracing::warn!(
                "插件 {plugin_id} API `{}` 执行失败: {error:#}",
                entry.path
            );
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "插件 API 执行失败（详见服务端日志）",
            )
        }
    }
}

fn scope_plugin_css(plugin_id: &str, css: &str) -> String {
    format!("@scope ([data-plugin-scope=\"{plugin_id}\"]) to ([data-plugin-scope]) {{\n{css}\n}}\n")
}

pub async fn plugin_asset(
    State(state): State<AppState>,
    Path((plugin_id, path)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let (file, mime) = state
        .frontend
        .asset_path(&plugin_id, &path)
        .map_err(|error| AppError::new(StatusCode::NOT_FOUND, format!("{error:#}")))?;
    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|error| AppError::new(StatusCode::NOT_FOUND, format!("读取资源失败: {error}")))?;
    // 插件 CSS 统一包进 @scope：只命中带 data-plugin-scope 的插件内容，
    // 不会污染应用壳或其它插件。浏览器不支持 @scope 时整段样式被忽略（安全降级）。
    let bytes = if mime.starts_with("text/css") {
        let css = String::from_utf8_lossy(&bytes);
        let css = css.strip_prefix('\u{feff}').unwrap_or(&css);
        scope_plugin_css(&plugin_id, css).into_bytes()
    } else {
        bytes
    };
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str(&mime)
                    .unwrap_or(HeaderValue::from_static("application/octet-stream")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_css_is_scoped_to_plugin_content() {
        let css = scope_plugin_css("hello-tsx", ".badge { color: red; }");
        assert!(
            css.starts_with("@scope ([data-plugin-scope=\"hello-tsx\"]) to ([data-plugin-scope])")
        );
        assert!(css.contains(".badge { color: red; }"));
    }
}
