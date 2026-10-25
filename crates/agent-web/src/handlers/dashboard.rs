//! 概览页：统计卡、插件仪表盘卡片、最近会话。

use axum::extract::State;
use axum::response::Response;

use super::*;

async fn dashboard_view(state: &AppState) -> Result<DashboardView, AppError> {
    let snapshot = state.agent.snapshot().await.map_err(|error| {
        AppError::upstream(format!("读取 agent 快照失败: {}", public_reason(&error)))
    })?;
    let running = running_ids(&snapshot);
    let threads: Vec<ThreadRow> = array_at(&snapshot, "threads")
        .iter()
        .map(|value| thread_row(value, &running))
        .collect();
    let recent = threads.iter().take(8).cloned().collect();

    let config = state.agent.config().await.unwrap_or(Value::Null);
    let providers = state.agent.providers().await.unwrap_or(Value::Null);
    let model = config
        .get("saved")
        .and_then(|saved| saved.get("model"))
        .and_then(Value::as_str)
        .or_else(|| config.get("model").and_then(Value::as_str))
        .unwrap_or("未配置");
    let provider_count = array_at(&providers, "providers").len();
    let frontend_count = state.frontend.list().len();

    Ok(DashboardView {
        stats: vec![
            Stat {
                label: "会话".to_string(),
                value: threads.len().to_string(),
                hint: "本地会话总数".to_string(),
                tone: "brand".to_string(),
            },
            Stat {
                label: "运行中".to_string(),
                value: running.len().to_string(),
                hint: "当前有轮次在执行".to_string(),
                tone: if running.is_empty() { "muted" } else { "warn" }.to_string(),
            },
            Stat {
                label: "模型".to_string(),
                value: model.to_string(),
                hint: format!("{} 个供应商", provider_count),
                tone: "muted".to_string(),
            },
            Stat {
                label: "前端插件".to_string(),
                value: frontend_count.to_string(),
                hint: "运行时扫描 frontend-plugins/".to_string(),
                tone: "brand".to_string(),
            },
        ],
        widgets: crate::plugins::dashboard_widgets_off_thread(
            std::sync::Arc::clone(&state.frontend),
            super::render_context_with_tokens(state, "dashboard", &snapshot, None),
        )
        .await,
        recent,
    })
}

pub async fn dashboard(State(state): State<AppState>) -> Result<Response, AppError> {
    let view = match dashboard_view(&state).await {
        Ok(view) => view,
        // 快照读不到 = 网关断线。二级页不再甩裸错误页：保留外壳 + 如实说"未连接"
        // （统计卡不出现，避免 0 值冒充真实数字）。
        Err(error) => {
            let reason = error.message.clone();
            let page = OfflinePage {
                base: offline_page_base(&state, "概览", "dashboard", None).await,
                reason,
            };
            return Ok(render(&page)?.into_response());
        }
    };
    let page = DashboardPage {
        base: base(&state, "概览", "dashboard", None).await,
        view,
    };
    Ok(render(&page)?.into_response())
}

pub async fn dashboard_partial(State(state): State<AppState>) -> Result<Response, AppError> {
    let view = dashboard_view(&state).await?;
    Ok(render(&DashboardPartial {
        view,
        base: base(&state, "概览", "dashboard", None).await,
    })?
    .into_response())
}
