//! 扩展页：前端插件（导航/页面/插槽）与 agent 插件的查看与启停。

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use super::*;

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
    let context = super::render_context_with_tokens(&state, "extensions", &snapshot, None);
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
            "extensions",
            None,
        )
        .await,
        plugin_name: plugin.name,
        page_title,
        html,
    };
    Ok(render(&page)?.into_response())
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
